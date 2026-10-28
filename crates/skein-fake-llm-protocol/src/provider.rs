//! A real HTTP/SSE fake provider service. HTTP documents enter the neutral
//! fake domain; its answers leave through the dialect server and SSE writer.
//!
//! Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, sections 4.4 and 6.3.

use crate::documents;

mod credentials;
mod failure;
mod inflate;
pub use inflate::inflate;
pub(crate) mod public;
pub use credentials::{MAX_CREDENTIALS, Presented, route};
#[path = "pieces.rs"]
mod pieces;
use alloc::boxed::Box;
use core::mem::size_of;
use failure::response_error;
pub use failure::scripts;
use skein_fake_llm_domain::{self as domain, api};
use skein_http::{Header, Method, server as http, sse::writer as sse};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Env, Id, List, Queue, ReplyTo, Slab, Token, bytes};
use skein_llm::anthropic;
use skein_llm::openai;

const ROUTES: u32 = 8;

/// Immutable ownership and document caps supplied by the caller to every entry point.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    /// Maximum provider calls held at once.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub calls: u32,
    /// HTTP server bounds supplied by the byte world.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub http: http::Limits,
    /// SSE framing bounds supplied by the byte world.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub sse: sse::Limits,
    /// Document codec bounds for the selected fake provider.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub documents: documents::Limits,
}

/// Immutable scripted peer configuration supplied by the byte world; it contains no live IO.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[expect(missing_debug_implementations, reason = "configured header values must not enter traces")]
pub struct Config {
    /// Accepts requests with no credential field on public or Anthropic routes.
    pub without_credential: bool,
    /// Codex metadata echoes; off unless the world explicitly selects them.
    pub echo: openai::Echo,
    /// Usage fields this dialect reports; absent selections are omitted.
    pub usage_fields: documents::UsageFields,
    /// Provider dialect selected by this scripted service.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub provider: documents::Provider,
    /// Exact configured HTTP request target accepted by this peer.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub path: Box<[u8]>,
    /// Verified compatibility data chosen by the world/deployment; this fake
    /// does not certify historical identities as current provider identities.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub headers: Box<[Header]>,
}

/// Typed refusal or terminal failure of the fake peer, independent of the agent's policy.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// Configuration or constructor ownership caps cannot be honored.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Limits,
    /// Input has invalid syntax or violates the codec's structural contract.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Malformed,
    /// A configured ownership, count or encoded-byte cap would be exceeded.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    TooLarge,
}

/// Decoded provider or peer event; the enclosing entry point checks order and correlation.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Request fields without credentials, and what was presented, before its body/query.
    Head { headers: Box<[Header]>, presented: Presented },
    /// Typed request forwarded to the neutral fake provider domain.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Domain(domain::Event),
    /// The owner requests connection closure; the lower layer still supplies Closed.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Close,
    /// The lower layer has completed connection closure.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    New,
    Reading,
    Body,
    Waiting,
    Streaming,
    Pieces { written: usize },
    Erring,
    Closing,
    Closed,
}

struct Call {
    server: Token,
    active: bool,
}

/// Bounded fake-provider service owning routing and pending calls across its connections.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[expect(missing_debug_implementations, reason = "configured headers remain protocol-only")]
pub struct Service {
    config: Config,
    calls: Slab<Call>,
    count: u64,
}

impl Service {
    /// Constructs empty bounded state under the supplied immutable limits; no IO or clocks are consulted.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub fn new(config: Config, limits: &Limits) -> Result<Service, Error> {
        if worst_case(limits).is_none()
            || config.path.first() != Some(&b'/')
            || config.echo.attribution_bytes > limits.documents.openai.retained
        {
            return Err(Error::Limits);
        }
        if config.path.len() > usize::try_from(limits.http.head).expect("u32 fits usize")
            || config.headers.len() > usize::try_from(limits.http.headers).expect("u32 fits usize")
        {
            return Err(Error::Limits);
        }
        for &byte in &config.path {
            if !(0x21..=0x7e).contains(&byte) {
                return Err(Error::Limits);
            }
        }
        let mut encoded = config.path.len();
        for (index, header) in config.headers.iter().enumerate() {
            if header.name.is_empty()
                || header.is(b"authorization")
                || header.is(b"x-api-key")
                || header.is(b"chatgpt-account-id")
                || (config.provider == documents::Provider::OpenAi
                    && config.path.as_ref() == b"/v1/responses"
                    && public::reserved(&header.name))
            {
                return Err(Error::Limits);
            }
            for &byte in &header.name {
                if !byte.is_ascii_alphanumeric() && !b"!#$%&'*+-.^_`|~".contains(&byte) {
                    return Err(Error::Limits);
                }
            }
            for &byte in &header.value {
                if !(0x20..=0x7e).contains(&byte) && byte != b'\t' {
                    return Err(Error::Limits);
                }
            }
            for prior in config.headers.get(..index).ok_or(Error::Limits)? {
                if prior.is(&header.name) {
                    return Err(Error::Limits);
                }
            }
            encoded = encoded
                .checked_add(header.name.len())
                .ok_or(Error::Limits)?
                .checked_add(header.value.len())
                .ok_or(Error::Limits)?
                .checked_add(4)
                .ok_or(Error::Limits)?;
        }
        if encoded > usize::try_from(limits.http.head).expect("u32 fits usize") {
            return Err(Error::Limits);
        }
        Ok(Service { config, calls: Slab::with_capacity(limits.calls), count: 0 })
    }

    /// Validates a credential list against the configured route and scripts before serving.
    ///
    /// # Errors
    /// Returns `Limits` after the first request or for a list the route cannot serve.
    pub fn validate_credentials(
        &self,
        credentials: &[skein_llm::Credential],
        scripts: &[api::Script],
    ) -> Result<(), Error> {
        if self.count != 0 {
            return Err(Error::Limits);
        }
        self::scripts(&self.config, credentials, scripts)
    }

    /// Number of fake provider requests observed since service construction.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// Routes still owned by the Service, including retired slots until
    /// iteration-end reclamation. This content-free count changes no state.
    /// Contract: docs/design/fake-llm.md, sections 2–5;
    /// programming-model.md, sections 5.1 and 6.3.
    #[must_use]
    pub const fn calls(&self) -> u32 {
        self.calls.len()
    }

    /// Routes one owned fake-domain update to its pending HTTP connection.
    /// Returns the update alongside its target, preserving a terminal's reply right
    /// for `down`; an unmatched or closed route returns the owned update as `Err`.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub fn target(&self, request: domain::Request) -> Result<(Token, domain::Request), domain::Request> {
        let (token, request) = match request {
            domain::Request::Reply { to, result } => {
                let token = to.into_token();
                (token, domain::Request::Reply { to: ReplyTo::new(token), result })
            }
            domain::Request::Piece { call, usage, text } => (call, domain::Request::Piece { call, usage, text }),
            domain::Request::Hold { call, usage } => (call, domain::Request::Hold { call, usage }),
        };
        let Some(call) = self.calls.get(Id::<Call>::from_token(token)) else {
            return Err(request);
        };
        if call.active { Ok((call.server, request)) } else { Err(request) }
    }

    /// Iteration-end reclamation of retired entries; it must follow delivery of owned outputs.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
    }
}

/// Bounded per-connection HTTP/SSE state driven by start, input, resume and close events.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[expect(missing_debug_implementations, reason = "HTTP input queues temporarily hold authorization values")]
pub struct Server {
    owner: Token,
    state: State,
    http: http::Server,
    writer: sse::Writer,
    events: Queue<http::Event>,
    writer_events: Queue<sse::Event>,
    requests: Queue<http::Request>,
    body: List<u8>,
    // A gzip body's inflated bytes, swapped into `body` once read back.
    inflated: List<u8>,
    // Whether the request's body came with `Content-Encoding: gzip`.
    gzip: bool,
    // Remaining announced body bytes; None for chunked framing.
    remaining_body: Option<u64>,
    call: Option<Id<Call>>,
    answer: Option<api::Answer>,
    pieces: pieces::Stream,
    sequence: u32,
    response_id: u64,
    reply: Option<Box<[u8]>>,
    reply_offset: usize,
    refused: Option<api::Error>,
    credential: Option<u32>,
    route: skein_llm::Provider,
    session: Option<[u8; 16]>,
    thread: Option<[u8; 16]>,
    writer_closed: bool,
}

impl Server {
    /// Constructs empty bounded state under the supplied immutable limits; no IO or clocks are consulted.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub fn new(owner: Token, limits: &Limits) -> Result<Server, Error> {
        if worst_case(limits).is_none() {
            return Err(Error::Limits);
        }
        let request = room(limits).ok_or(Error::Limits)?;
        Ok(Server {
            owner,
            state: State::New,
            http: http::Server::new(&limits.http),
            writer: sse::Writer::new(&limits.sse),
            events: Queue::uncapped(),
            writer_events: Queue::uncapped(),
            requests: Queue::uncapped(),
            body: List::with_capacity(request),
            inflated: List::with_capacity(request),
            gzip: false,
            remaining_body: None,
            call: None,
            answer: None,
            pieces: pieces::Stream::new(),
            sequence: 0,
            response_id: 0,
            reply: None,
            reply_offset: 0,
            refused: None,
            credential: None,
            route: skein_llm::Provider::OpenAi,
            session: None,
            thread: None,
            writer_closed: false,
        })
    }

    /// Whether this peer can progress from its queued input or injected time without fresh transport input.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub fn has_work(&self) -> bool {
        !self.events.is_empty() || !self.writer_events.is_empty() || !self.requests.is_empty() || pieces::ready(self)
    }
}

/// Starts this connection's HTTP state with uncapped output queues.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn start(
    server: &mut Server,
    service: &mut Service,
    credentials: &[skein_llm::Credential],
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state != State::New {
        return;
    }
    server.state = State::Reading;
    http_down(server, env, http::Request::Next, below);
    resume(server, service, credentials, env, above, below);
}

/// Consumes one lower transport event and routes bounded HTTP progress or terminals to the owner.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn up(
    server: &mut Server,
    service: &mut Service,
    credentials: &[skein_llm::Credential],
    env: &Env<Limits>,
    event: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match server.state {
        State::New | State::Closing | State::Closed => return,
        State::Reading | State::Body | State::Waiting | State::Streaming | State::Pieces { .. } | State::Erring => {}
    }
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.http };
    http::up(&mut server.http, &child, event, &mut server.events, below);
    resume(server, service, credentials, env, above, below);
}

/// Accepts one neutral fake-domain update and schedules its bounded HTTP/SSE response.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn down(
    server: &mut Server,
    service: &mut Service,
    credentials: &[skein_llm::Credential],
    env: &Env<Limits>,
    request: domain::Request,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    let (to, result) = match request {
        domain::Request::Reply { to, result } => (to, result),
        domain::Request::Piece { call, usage, text } => {
            pieces::receive(server, service, env, (call, usage, Some(text)), above, below);
            resume(server, service, credentials, env, above, below);
            return;
        }
        domain::Request::Hold { call, usage } => {
            pieces::receive(server, service, env, (call, usage, None), above, below);
            resume(server, service, credentials, env, above, below);
            return;
        }
    };
    let id = Id::<Call>::from_token(to.into_token());
    if server.call != Some(id) || !matches_waiting(server.state) {
        return;
    }
    let Some(call) = service.calls.get_mut(id) else {
        return;
    };
    if !call.active || call.server != server.owner {
        return;
    }
    call.active = false;
    service.calls.retire(id);
    server.call = None;
    match result {
        Ok(answer) => {
            if answer_fits(&answer, &env.limits.documents) {
                server.answer = Some(answer);
                if server.state == State::Waiting {
                    server.state = State::Streaming;
                    server.sequence = 0;
                    pieces::head(server);
                }
                next_event(server, service, env, above, below);
            } else if let State::Pieces { .. } = server.state {
                close(server, service, env, above, below);
            } else {
                response_error(server, service, env, api::Error::InvalidRequest.into());
            }
        }
        Err(mut failure) => {
            if failure.parts.is_empty() && server.state == State::Waiting {
                response_error(server, service, env, failure);
            } else {
                let parts = core::mem::replace(&mut failure.parts, Box::new([]));
                let answer = api::Answer { parts, finish: api::Finish::Stop, usage: api::Usage::NONE };
                if !answer_fits(&answer, &env.limits.documents) {
                    close(server, service, env, above, below);
                    return;
                }
                server.answer = Some(answer);
                server.pieces.fail(failure);
                if server.state == State::Waiting {
                    server.state = match &server.answer.as_ref().expect("stored failed answer").parts.first() {
                        Some(api::Part::Text { .. }) => State::Pieces { written: 0 },
                        Some(api::Part::Opaque { .. } | api::Part::ToolCall { .. } | api::Part::ToolOutput { .. })
                        | None => State::Streaming,
                    };
                    server.sequence = 0;
                    pieces::head(server);
                }
                next_event(server, service, env, above, below);
            }
        }
    }
    resume(server, service, credentials, env, above, below);
}

/// Performs bounded deferred HTTP/SSE handoffs through uncapped output queues.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn resume(
    server: &mut Server,
    service: &mut Service,
    credentials: &[skein_llm::Credential],
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    for _route in 0..ROUTES {
        if let Some(event) = server.writer_events.pop() {
            match event {
                sse::Event::Sent => {
                    server.pieces.sent();
                    next_event(server, service, env, above, below);
                }
                sse::Event::Refused(_) | sse::Event::Failed(_) => close(server, service, env, above, below),
                sse::Event::Closed => {}
            }
        } else if let Some(event) = server.events.pop() {
            http_event(server, service, credentials, env, event, above, below);
        } else if let Some(request) = server.requests.pop() {
            http_down(server, env, request, below);
        } else if pieces::ready(server) {
            next_event(server, service, env, above, below);
        } else {
            break;
        }
    }
}

/// Starts orderly connection closure; outstanding lower work still requires its terminal.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn close(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closing || server.state == State::Closed {
        return;
    }
    if let Some(id) = server.call.take()
        && let Some(call) = service.calls.get_mut(id)
        && call.active
    {
        call.active = false;
        service.calls.retire(id);
    }
    for _slot in 0..server.events.len() {
        if server.events.pop().is_none() {
            break;
        }
    }
    for _slot in 0..server.requests.len() {
        if server.requests.pop().is_none() {
            break;
        }
    }
    server.answer = None;
    server.pieces.clear();
    server.reply = None;
    server.body.clear();
    if !server.writer_closed {
        writer_down(server, env, sse::Request::Close);
        server.writer_closed = true;
    }
    for _slot in 0..server.requests.len() {
        if server.requests.pop().is_none() {
            break;
        }
    }
    http_down(server, env, http::Request::Close, below);
    server.state = State::Closing;
    above.push(Event::Close);
}

/// Consumes the lower layer's close terminal and emits the owner's single Closed observation.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn closed(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closed {
        return;
    }
    if server.state != State::Closing {
        close(server, service, env, above, below);
    }
    server.state = State::Closed;
    above.push(Event::Closed);
}

fn entrance(
    server: &mut Server,
    service: &mut Service,
    credentials: &[skein_llm::Credential],
    limits: &Limits,
    call: http::Call,
    above: &mut Queue<Event>,
) -> Result<(), api::Error> {
    server.route = match route(&service.config, credentials) {
        Ok(route) => route,
        Err(Error::Limits | Error::Malformed | Error::TooLarge) => return Err(api::Error::InvalidRequest),
    };
    server.credential = None;
    let identity = next_response(&mut service.count);
    server.response_id = identity.unwrap_or(0);
    server.refused = if identity.is_some() && call.method == Method::Post && call.target == service.config.path {
        None
    } else {
        Some(api::Error::InvalidRequest)
    };
    let mut fields = credentials::Fields::new();
    server.session = None;
    server.thread = None;
    server.gzip = false;
    let mut content_type = 0_u32;
    for header in &call.headers {
        // A body comes as it is or in gzip; any other coding, or two, is refused.
        if header.is(b"content-encoding") {
            if server.gzip || !header.value.eq_ignore_ascii_case(b"gzip") {
                server.refused = Some(api::Error::InvalidRequest);
            }
            server.gzip = true;
        }
        if server.route == skein_llm::Provider::OpenAi && public::reserved(&header.name) {
            server.refused = Some(api::Error::InvalidRequest);
        }
        if service.config.provider == documents::Provider::OpenAi && header.is(b"session-id") {
            if server.session.is_some() {
                server.refused = Some(api::Error::InvalidRequest);
            }
            server.session = documents::parse_uuid(&header.value);
            if server.session.is_none() {
                server.refused = Some(api::Error::InvalidRequest);
            }
        }
        if service.config.provider == documents::Provider::OpenAi && header.is(b"thread-id") {
            if server.thread.is_some() {
                server.refused = Some(api::Error::InvalidRequest);
            }
            server.thread = documents::parse_uuid(&header.value);
            if server.thread.is_none() {
                server.refused = Some(api::Error::InvalidRequest);
            }
        }
        fields.read(header);
        if header.is(b"content-type") {
            content_type = content_type.saturating_add(1);
            if header.value.as_ref() != b"application/json" {
                server.refused = Some(api::Error::InvalidRequest);
            }
        }
    }
    if content_type != 1 {
        server.refused = Some(api::Error::InvalidRequest);
    }
    let auth = credentials::authorize(server.route, credentials, &fields, service.config.without_credential);
    server.credential = match auth {
        credentials::Auth::Matched(index) => Some(index),
        credentials::Auth::Open | credentials::Auth::Unauthorized => None,
    };
    for expected in &service.config.headers {
        let mut count = 0_u32;
        for header in &call.headers {
            if header.is(&expected.name) {
                count = count.saturating_add(1);
                if header.value != expected.value {
                    server.refused = Some(api::Error::InvalidRequest);
                }
            }
        }
        if count != 1 {
            server.refused = Some(api::Error::InvalidRequest);
        }
    }
    if auth == credentials::Auth::Unauthorized {
        server.refused = Some(api::Error::Unauthorized);
    }
    server.state = State::Body;
    server.remaining_body = match call.body {
        http::Body::None => Some(0),
        http::Body::Length(length) => Some(length),
        http::Body::Chunked => None,
    };
    let presented = fields.presented(server.route, credentials, auth);
    above.push(Event::Head { headers: credentials::observed_headers(call.headers), presented });
    demand_body(server, limits);
    match server.refused {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

// Known lengths use bounded exact reads; unknown bodies read what has
// arrived. After zero, a read probes End rather than assuming a terminal.
fn body_read(remaining: Option<u64>, cap: u32) -> Read {
    match remaining {
        Some(remaining) => Read::Fill(u32::try_from(remaining.min(u64::from(cap))).expect("bounded HTTP read").max(1)),
        None => Read::Arrived { max: cap.max(1) },
    }
}

fn demand_body(server: &mut Server, limits: &Limits) {
    let cap = match server.remaining_body {
        Some(_) => limits.http.read,
        None => server.body.room(),
    };
    server.requests.push(http::Request::Body(Down::Demand { read: body_read(server.remaining_body, cap), room: 0 }));
}

fn next_response(count: &mut u64) -> Option<u64> {
    let next = count.checked_add(1)?;
    *count = next;
    Some(next)
}

fn http_down(server: &mut Server, env: &Env<Limits>, request: http::Request, below: &mut Queue<Down>) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.http };
    http::down(&mut server.http, &child, request, &mut server.events, below);
}

fn writer_down(server: &mut Server, env: &Env<Limits>, request: sse::Request) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.sse };
    let mut down = Queue::uncapped();
    sse::down(&mut server.writer, &child, request, &mut server.writer_events, &mut down);
    for _slot in 0..down.len() {
        let down = down.pop().expect("writer output present at the start");
        server.requests.push(http::Request::Reply(down));
    }
}

// HTTP Done has already settled this response's downstream demand. Close the
// old writer explicitly before replacing it; its withdrawal belongs to that
// completed response and must not be sent into HTTP's WaitingNext state.
fn reset_writer(server: &mut Server, env: &Env<Limits>) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.sse };
    let mut down = Queue::uncapped();
    if !server.writer_closed {
        sse::down(&mut server.writer, &child, sse::Request::Close, &mut server.writer_events, &mut down);
    }
    for _slot in 0..server.writer_events.len() {
        if server.writer_events.pop().is_none() {
            break;
        }
    }
    server.writer = sse::Writer::new(&env.limits.sse);
    server.writer_closed = false;
}

fn http_event(
    server: &mut Server,
    service: &mut Service,
    credentials: &[skein_llm::Credential],
    env: &Env<Limits>,
    event: http::Event,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closing || server.state == State::Closed {
        return;
    }
    match event {
        http::Event::Body(Up::Bytes(data)) => {
            if data.len() > usize::try_from(server.body.room()).expect("u32 fits usize") {
                close(server, service, env, above, below);
                return;
            }
            if let Some(remaining) = server.remaining_body {
                server.remaining_body = Some(
                    remaining
                        .checked_sub(u64::try_from(data.len()).expect("bounded body delivery"))
                        .expect("HTTP delivers within the announced body"),
                );
            }
            for byte in data {
                server.body.push(byte).expect("checked body cap");
            }
            demand_body(server, &env.limits);
        }
        http::Event::Body(Up::End) => requested(server, service, env, above),
        http::Event::Reply(event) => match server.state {
            State::Streaming | State::Pieces { .. } => {
                let child = Env { now: env.now, wall: env.wall, limits: env.limits.sse };
                let mut down = Queue::uncapped();
                sse::up(&mut server.writer, &child, event, &mut server.writer_events, &mut down);
                for _slot in 0..down.len() {
                    if let Some(down) = down.pop() {
                        server.requests.push(http::Request::Reply(down));
                    }
                }
            }
            State::Erring => match event {
                Up::Room => send_error(server, env),
                Up::Failed(_) | Up::Bytes(_) | Up::End => close(server, service, env, above, below),
            },
            State::New | State::Reading | State::Body | State::Waiting | State::Closing | State::Closed => {}
        },
        http::Event::Done(reuse) => {
            server.answer = None;
            server.pieces.clear();
            server.body.clear();
            server.reply = None;
            match reuse {
                http::Reuse::Keep => {
                    reset_writer(server, env);
                    server.state = State::Reading;
                    server.requests.push(http::Request::Next);
                }
                http::Reuse::Close => close(server, service, env, above, below),
            }
        }
        http::Event::Call(call) => match entrance(server, service, credentials, &env.limits, call, above) {
            Ok(()) => {}
            Err(error) => server.refused = Some(error),
        },
        http::Event::Ended
        | http::Event::Failed(_)
        | http::Event::Refused(_)
        | http::Event::Body(Up::Failed(_) | Up::Room) => close(server, service, env, above, below),
        http::Event::Closed => {}
    }
}

fn requested(server: &mut Server, service: &mut Service, env: &Env<Limits>, above: &mut Queue<Event>) {
    if let Some(error) = server.refused.take() {
        server.body.clear();
        response_error(server, service, env, error.into());
        return;
    }
    if server.gzip {
        server.gzip = false;
        let inflated = inflate(server.body.as_slice(), &mut server.inflated);
        core::mem::swap(&mut server.body, &mut server.inflated);
        server.inflated.clear();
        if inflated.is_none() {
            server.body.clear();
            response_error(server, service, env, api::Error::InvalidRequest.into());
            return;
        }
    }
    let query = documents::request_on(server.route, server.body.as_slice(), &env.limits.documents);
    if service.config.provider != documents::Provider::OpenAi || !service.config.echo.enabled() {
        server.body.clear();
    }
    match query {
        Ok(mut query) => {
            query.credential = server.credential;
            let scope = match query.caching {
                api::Caching::Scope(key) => Some(key),
                api::Caching::Unscoped | api::Caching::Marks(_) => None,
            };
            if server.route == skein_llm::Provider::OpenAiCodex
                && (scope != server.session || scope.is_some() != server.thread.is_some())
            {
                server.body.clear();
                response_error(server, service, env, api::Error::InvalidRequest.into());
                return;
            }
            let Ok(id) = service.calls.insert(Call { server: server.owner, active: true }) else {
                response_error(server, service, env, api::Error::Overloaded.into());
                return;
            };
            server.call = Some(id);
            server.state = State::Waiting;
            above.push(Event::Domain(domain::Event::Call {
                reply_to: ReplyTo::new(id.token()),
                call: id.token(),
                query,
            }));
        }
        Err(documents::Error::Refused(field)) => {
            let mut failure: api::Failure = api::Error::Unsupported.into();
            failure.report.field = bytes::copy_of(field);
            response_error(server, service, env, failure);
        }
        Err(documents::Error::Malformed | documents::Error::TooLarge) => {
            response_error(server, service, env, api::Error::InvalidRequest.into());
        }
    }
}

fn matches_waiting(state: State) -> bool {
    match state {
        State::Waiting | State::Pieces { .. } => true,
        State::New
        | State::Reading
        | State::Body
        | State::Streaming
        | State::Erring
        | State::Closing
        | State::Closed => false,
    }
}

fn next_event(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if let State::Pieces { .. } = server.state {
        match pieces::next(server, service, env) {
            Ok(Some(event)) => writer_down(server, env, sse::Request::Event(event)),
            Ok(None) => {}
            Err(documents::Error::Malformed | documents::Error::TooLarge | documents::Error::Refused(_)) => {
                close(server, service, env, above, below);
            }
        }
        return;
    }
    if server.state != State::Streaming {
        return;
    }
    match pieces::terminal(server, env) {
        Ok(Some(event)) => {
            writer_down(server, env, sse::Request::Event(event));
            return;
        }
        Ok(None) => {}
        Err(documents::Error::Malformed | documents::Error::TooLarge | documents::Error::Refused(_)) => {
            close(server, service, env, above, below);
            return;
        }
    }
    let Some(answer) = &server.answer else {
        writer_down(server, env, sse::Request::Finish);
        return;
    };
    match documents::event_with_options(
        service.config.provider,
        answer,
        server.sequence,
        server.response_id,
        server.body.as_slice(),
        documents::Options { echo: service.config.echo, usage_fields: service.config.usage_fields },
        &env.limits.documents,
    ) {
        Ok(Some(event)) => {
            let Some(next) = server.sequence.checked_add(1) else {
                close(server, service, env, above, below);
                return;
            };
            server.sequence = next;
            writer_down(server, env, sse::Request::Event(event));
        }
        Ok(None) => writer_down(server, env, sse::Request::Finish),
        Err(documents::Error::Malformed | documents::Error::TooLarge | documents::Error::Refused(_)) => {
            close(server, service, env, above, below);
        }
    }
}

fn send_error(server: &mut Server, env: &Env<Limits>) {
    let Some(body) = &server.reply else {
        return;
    };
    let end = server
        .reply_offset
        .saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits usize"))
        .min(body.len());
    let Some(piece) = body.get(server.reply_offset..end) else {
        return;
    };
    server.requests.push(http::Request::Reply(Down::Send(bytes::copy_of(piece))));
    server.reply_offset = end;
    if end == body.len() {
        server.requests.push(http::Request::Reply(Down::Finish));
    } else {
        server.requests.push(http::Request::Reply(Down::Demand { read: Read::Nothing, room: env.limits.http.send }));
    }
}

fn answer_fits(answer: &api::Answer, limits: &documents::Limits) -> bool {
    if answer.parts.len()
        > usize::try_from(limits.openai.output_items.min(limits.anthropic.output_items)).expect("u32 fits usize")
    {
        return false;
    }
    let mut length = Some(0_usize);
    for part in &answer.parts {
        let size = match part {
            api::Part::Text { text } | api::Part::Opaque { bytes: text } => Some(text.len()),
            api::Part::ToolCall { id, name, arguments } => match id.len().checked_add(name.len()) {
                Some(length) => length.checked_add(arguments.len()),
                None => None,
            },
            api::Part::ToolOutput { .. } => return false,
        };
        length = match length {
            Some(length) => match size {
                Some(size) => length.checked_add(size),
                None => None,
            },
            None => None,
        };
    }
    match length {
        Some(length) => {
            length <= usize::try_from(limits.openai.answer.max(limits.anthropic.answer)).expect("u32 fits usize")
        }
        None => false,
    }
}

/// Counts all owned field wrappers, names and values in an actual request head.
#[must_use]
pub fn head_bytes(headers: &[Header]) -> Option<u64> {
    let mut bytes = u64::try_from(headers.len().checked_mul(size_of::<Header>())?).ok()?;
    for header in headers {
        bytes = bytes
            .checked_add(u64::try_from(header.name.len()).ok()?)?
            .checked_add(u64::try_from(header.value.len()).ok()?)?;
    }
    Some(bytes)
}

/// Checked maximum ownership of one admitted HTTP request head.
#[must_use]
pub fn head_worst_case(limits: &Limits) -> Option<u64> {
    u64::from(limits.http.headers)
        .checked_mul(u64::try_from(size_of::<Header>()).ok()?)?
        .checked_add(u64::from(limits.http.head))
}

/// The room a request body takes: the larger document cap, or the gzip
/// member a body at that cap can be sent as, which a stored block makes a
/// few bytes larger.
fn room(limits: &Limits) -> Option<u32> {
    let request = limits.documents.anthropic.request.max(limits.documents.openai.request);
    u32::try_from(skein_http::gzip::bound(usize::try_from(request).ok()?)?).ok()
}

/// Checked maximum owned bytes under limits, or `None` when counters or containers cannot be bounded.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let request = limits.documents.openai.request.max(limits.documents.anthropic.request);
    if limits.http.body < u64::from(request)
        || limits.sse.chunk > limits.http.send
        || limits.documents.model_ceiling == 0
    {
        return None;
    }
    u64::try_from(size_of::<Server>())
        .ok()?
        .checked_add(u64::try_from(size_of::<Service>()).ok()?)?
        .checked_add(u64::from(limits.http.headers).checked_mul(u64::try_from(size_of::<Header>()).ok()?)?)?
        .checked_add(Slab::<Call>::worst_case(limits.calls)?)?
        .checked_add(http::worst_case(&limits.http)?)?
        .checked_add(sse::worst_case(&limits.sse)?)?
        .checked_add(pieces::worst_case(&limits.documents))?
        .checked_add(openai::worst_case(&limits.documents.openai)?)?
        .checked_add(anthropic::worst_case(&limits.documents.anthropic)?)?
        // Configured echoes retain this raw request in the existing body List;
        // native codec worst_case above prices transient decoded metadata and entry scratch.
        .checked_add(List::<u8>::worst_case(room(limits)?)?)?
        // A gzip body inflates beside its member, into a second List; the two
        // trade places, so each holds the larger room.
        .checked_add(List::<u8>::worst_case(room(limits)?)?)?
        .checked_add(u64::from(limits.documents.openai.answer.max(limits.documents.anthropic.answer)).checked_mul(8)?)?
        .checked_add(u64::from(limits.http.head).checked_mul(3)?)?
        .checked_add(4_u64.checked_mul(u64::try_from(size_of::<Header>()).ok()?)?)?
        .checked_add(192)?
        .checked_add(u64::from(limits.documents.openai.answer.max(limits.documents.anthropic.answer)))?
        // The current entrance delivery owns its batch separately from HTTP's
        // intake and the accumulated document.
        .checked_add(u64::from(request.max(limits.http.read)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn length_framed_body_demands_bounded_batches_and_probes_actual_end() {
        use skein_lib::stream::Read;

        for (remaining, cap, expected) in [
            (None, 256, Read::Arrived { max: 256 }),
            (None, 0, Read::Arrived { max: 1 }),
            (Some(0), 256, Read::Fill(1)),
            (Some(1), 256, Read::Fill(1)),
            (Some(257), 256, Read::Fill(256)),
            (Some(255), 256, Read::Fill(255)),
            (Some(u64::MAX), u32::MAX, Read::Fill(u32::MAX)),
            (Some(8192), 1, Read::Fill(1)),
        ] {
            assert_eq!(super::body_read(remaining, cap), expected);
        }
    }

    #[test]
    fn final_identity_is_unique_and_exhaustion_refuses_further_identities() {
        let mut count = u64::MAX - 1;
        assert_eq!(super::next_response(&mut count), Some(u64::MAX));
        assert_eq!(super::next_response(&mut count), None);
        assert_eq!(super::next_response(&mut count), None);
        assert_eq!(count, u64::MAX);
    }
}
