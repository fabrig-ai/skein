//! Hosted issuer HTTP face (oauth.md, section 5). The independent issuer
//! checks registration, PKCE, code reuse and refresh rotation. This face
//! owns bounded HTTP connections and redirects, never a client's state or
//! credential durability. `new` listens; `queue` installs bounded response
//! plans; `Host::iterate` drives io; `shutdown` settles connections and
//! keeps an already accepted delayed issuer response until its terminal.

use std::mem::size_of;
use std::net::SocketAddr;

use skein_fake_oauth as issuer;
use skein_http::{Header, server as http};
use skein_io::{self as io, kernel};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Decimal, Duration, Env, List, Queue, Time, Token, Wall, bytes};
use skein_oauth::HttpRequest;
use skein_shell::Host;

mod route;

use route::{Paths, Route, route};

use crate::face::{self, Face, LISTENER};
use crate::transport::{self, Wire};
use crate::{Error, Limits, Transport};

/// The issuer's outside records for a world's referee, without credential values.
#[derive(PartialEq, Eq)]
pub enum Observation {
    /// The server accepted this connection from a client or scripted browser.
    Accepted { connection: Token },
    /// The browser sent this authorization target to the registered endpoint.
    Authorize { connection: Token, target: Box<[u8]> },
    /// The token endpoint received a complete POST.
    Post { connection: Token },
    /// The browser requested this issuer document target.
    Document { connection: Token, target: Box<[u8]> },
    /// The revocation endpoint received a complete POST.
    Revoke { connection: Token },
    /// The issuer approved sign-in and the server sent its redirect.
    Redirect { connection: Token },
    /// The issuer emitted a token or revocation terminal, even if disconnected.
    Answered { connection: Option<Token>, id: u64, status: u16 },
    /// io settled the connection's close.
    Closed { connection: Token },
}

struct Response {
    body: Box<[u8]>,
    offset: usize,
}

struct Connection {
    owner: Token,
    wire: Wire,
    http: http::Server,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    below: Queue<Down>,
    plain: Queue<Up>,
    body: List<u8>,
    content_type: Option<&'static [u8]>,
    post: Option<Route>,
    active: Option<u64>,
    response: Option<Response>,
    closing: bool,
}

/// A world's fake issuer process, serving authorization and token endpoints.
pub struct Peer {
    face: Face,
    transport: Transport,
    issuer: issuer::Issuer,
    issuer_limits: issuer::Limits,
    http_limits: http::Limits,
    authorization_url: Box<[u8]>,
    token_endpoint: Box<[u8]>,
    paths: Paths,
    revoke_endpoint: Option<Box<[u8]>>,
    metadata_url: Option<Box<[u8]>>,
    key_set_url: Option<Box<[u8]>>,
    outgoing: Queue<issuer::Request>,
    connections: Vec<Connection>,
    next: u64,
    exchange: u64,
    observations: Vec<Observation>,
    observed_bytes: u64,
    recording: bool,
    bound: u64,
}

impl Peer {
    /// Adopts a held listener before the first drive, so the face's queued
    /// Listen claims its address. On refusal the caller keeps the descriptor.
    /// The peer owns an accepted descriptor through ordinary shutdown.
    pub fn adopt_listener(&mut self, fd: kernel::Fd, addr: SocketAddr) -> Result<(), kernel::Fd> {
        self.face.io.adopt_listener(fd, addr).map(|_| ())
    }

    /// Starts a loopback issuer with explicit registration and immutable machine bounds.
    pub fn new(
        address: SocketAddr,
        transport: Transport,
        limits: Limits,
        config: issuer::Config,
        issuer_limits: issuer::Limits,
        http_limits: http::Limits,
    ) -> Result<Self, Error> {
        transport::validate(&limits, transport)?;
        let bound = worst_case(&limits, &issuer_limits, &http_limits, transport).ok_or(Error::Limits)?;
        let terms = config.terms.as_ref();
        let openid = terms.and_then(|terms| terms.openid.as_ref());
        let revoke_endpoint = terms.and_then(|terms| terms.revocation_endpoint.clone());
        let metadata_url = openid.map(|openid| {
            let mut url = openid.issuer.to_vec();
            url.extend_from_slice(b"/.well-known/openid-configuration");
            url.into_boxed_slice()
        });
        let key_set_url = openid.map(|openid| openid.key_set_url.clone());
        let paths = Paths {
            authorize: path(&config.authorization_url, transport)?,
            token: path(&config.token_endpoint, transport)?,
            revoke: revoke_endpoint.as_deref().map(|url| path(url, transport)).transpose()?,
            metadata: metadata_url.as_deref().map(|url| path(url, transport)).transpose()?,
            key_set: key_set_url.as_deref().map(|url| path(url, transport)).transpose()?,
        };
        let authorization_url = config.authorization_url.clone();
        let token_endpoint = config.token_endpoint.clone();
        Ok(Self {
            face: Face::new(address, limits)?,
            transport,
            issuer: issuer::Issuer::new(config, issuer_limits).map_err(|_| Error::Limits)?,
            issuer_limits,
            http_limits,
            authorization_url,
            token_endpoint,
            paths,
            revoke_endpoint,
            metadata_url,
            key_set_url,
            outgoing: Queue::uncapped(),
            connections: Vec::with_capacity(usize::try_from(limits.connections).expect("u32 fits")),
            next: 1,
            exchange: 1,
            observations: Vec::with_capacity(usize::try_from(limits.observations).expect("u32 fits")),
            observed_bytes: 0,
            recording: true,
            bound,
        })
    }

    /// Adds one actual issuer response plan; its domain enforces count and byte limits.
    pub fn queue(&mut self, plan: issuer::Plan) -> Result<(), issuer::Error> {
        self.issuer.queue(plan)
    }

    /// The scripted access tokens, for the LLM peer's list at start.
    #[must_use]
    pub fn granted(&self) -> Box<[Box<[u8]>]> {
        self.issuer.granted()
    }

    /// The actual listener address after io announces Listening.
    #[must_use]
    pub fn address(&self) -> Option<SocketAddr> {
        self.face.address
    }

    /// Keeps no outside records. Call on a fresh peer before its first pass.
    #[must_use]
    pub fn without_records(mut self) -> Self {
        let slots = u64::from(self.face.limits.observations)
            .checked_mul(u64::try_from(core::mem::size_of::<Observation>()).expect("record size"))
            .expect("validated record slots");
        let records = slots.checked_add(u64::from(self.face.limits.observation_bytes)).expect("validated records");
        self.bound = self.bound.checked_sub(records).expect("bound includes records");
        self.face.limits.observations = 0;
        self.face.limits.observation_bytes = 0;
        self.observations = Vec::new();
        self.observed_bytes = 0;
        self.recording = false;
        self
    }
    /// Frozen outside records, retained under observation bounds supplied by the world.
    #[must_use]
    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }

    /// Ends a peer whose clients have settled, retaining accepted work’s terminals.
    /// A caller ending it earlier names its scenario about peer hang-up.
    pub fn shutdown(&mut self) {
        self.face.shutdown();
    }

    fn observe(&mut self, record: Observation, payload_bytes: u64) {
        if !self.recording {
            return;
        }
        self.reserve_observation(payload_bytes);
        self.observed_bytes = self.observed_bytes.checked_add(payload_bytes).expect("bounded observations");
        self.observations.push(record);
    }

    fn reserve_observation(&self, payload_bytes: u64) {
        assert!(
            self.observations.len() < usize::try_from(self.face.limits.observations).expect("u32 fits"),
            "world observation count cap"
        );
        assert!(
            self.observed_bytes
                .checked_add(payload_bytes)
                .is_some_and(|bytes| bytes <= u64::from(self.face.limits.observation_bytes)),
            "world observation byte cap before copying"
        );
    }

    fn io_event(&mut self, event: io::Event) {
        match event {
            io::Event::Listening { owner, listener, addr } => {
                assert_eq!(owner, LISTENER, "one issuer listener");
                self.face.listener = Some(listener);
                let addr = addr.socket().expect("fake peers listen on socket addresses");
                self.face.address = Some(addr);
            }
            io::Event::Accepted { owner, socket, .. } => {
                assert_eq!(owner, LISTENER, "one issuer listener accepts");
                if self.face.stopping
                    || self.connections.len() == usize::try_from(self.face.limits.connections).expect("u32 fits")
                {
                    self.face.requests.push(io::Request::Reject { socket });
                    return;
                }
                let owner = Token::new(self.next);
                self.next =
                    self.next.checked_add(1).filter(|next| *next < u64::MAX).expect("finite world connection ids");
                self.face.requests.push(io::Request::Bind { socket, owner });
                let limits = self.face.limits;
                let mut requests = Queue::uncapped();
                requests.push(http::Request::Next);
                self.connections.push(Connection {
                    owner,
                    wire: Wire::new(socket, self.transport, limits),
                    http: http::Server::new(&self.http_limits),
                    events: Queue::uncapped(),
                    requests,
                    below: Queue::uncapped(),
                    plain: Queue::uncapped(),
                    body: List::with_capacity(self.issuer_limits.request_bytes),
                    content_type: None,
                    post: None,
                    active: None,
                    response: None,
                    closing: false,
                });
                self.observe(Observation::Accepted { connection: owner }, 0);
            }
            io::Event::Output { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.output(up, &mut connection.plain, &mut self.face.requests);
                }
            }
            io::Event::Stream { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.up(up, &mut connection.plain, &mut self.face.requests);
                }
            }
            io::Event::Closed { owner } => {
                if owner == LISTENER {
                    return;
                }
                if let Some(index) = self.connections.iter().position(|connection| connection.owner == owner) {
                    self.connections.remove(index);
                    self.observe(Observation::Closed { connection: owner }, 0);
                }
            }
            io::Event::Failed { owner, .. } => {
                assert_ne!(owner, LISTENER, "issuer listener must bind");
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.close(&mut self.face.requests);
                    connection.closing = true;
                }
            }
            io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => {
                panic!("issuer only listens and serves classic byte streams");
            }
        }
    }

    fn http_event(&mut self, index: usize, event: http::Event, now: Time, wall: Wall) {
        match event {
            http::Event::Call(call) => self.call(index, call, now, wall),
            http::Event::Body(Up::Bytes(bytes)) => {
                let connection = &mut self.connections[index];
                if bytes.len() > usize::try_from(connection.body.room()).expect("u32 fits") {
                    self.close(index);
                    return;
                }
                for byte in bytes {
                    connection.body.push(byte).expect("admitted request body");
                }
                connection.requests.push(http::Request::Body(Down::Demand {
                    read: Read::Arrived { max: connection.body.room().max(1) },
                    room: 0,
                }));
            }
            http::Event::Body(Up::End) => self.post(index, now, wall),
            http::Event::Reply(Up::Room) => {
                let connection = &mut self.connections[index];
                let response = connection.response.as_mut().expect("one HTTP response body");
                let end = response
                    .offset
                    .checked_add(usize::try_from(self.http_limits.send).expect("u32 fits"))
                    .expect("bounded response")
                    .min(response.body.len());
                connection
                    .requests
                    .push(http::Request::Reply(Down::Send(bytes::copy_of(&response.body[response.offset..end]))));
                response.offset = end;
                if end == response.body.len() {
                    connection.requests.push(http::Request::Reply(Down::Finish));
                } else {
                    connection
                        .requests
                        .push(http::Request::Reply(Down::Demand { read: Read::Nothing, room: self.http_limits.send }));
                }
            }
            http::Event::Done(http::Reuse::Keep) => {
                let connection = &mut self.connections[index];
                connection.response = None;
                connection.active = None;
                connection.requests.push(http::Request::Next);
            }
            http::Event::Done(http::Reuse::Close)
            | http::Event::Ended
            | http::Event::Failed(_)
            | http::Event::Refused(_)
            | http::Event::Reply(Up::Bytes(_) | Up::End | Up::Failed(_))
            | http::Event::Body(Up::Room | Up::Failed(_)) => self.close(index),
            http::Event::Closed => {}
        }
    }

    fn call(&mut self, index: usize, call: http::Call, now: Time, wall: Wall) {
        let route = route(&self.paths, call.method, &call.target);
        if route == Route::Authorize {
            let target_base = call.target.split(|byte| *byte == b'?').next().expect("target has a base");
            let owner = self.connections[index].owner;
            if self
                .authorization_url
                .len()
                .checked_add(call.target.len().checked_sub(target_base.len()).expect("target suffix"))
                .is_none_or(|length| length > usize::try_from(self.issuer_limits.request_bytes).expect("u32 fits"))
            {
                self.respond(index, 400, Box::new([]), Box::new([]), Duration::ZERO);
                return;
            }
            let mut url = Vec::with_capacity(
                self.authorization_url
                    .len()
                    .checked_add(call.target.len().checked_sub(target_base.len()).expect("suffix"))
                    .expect("bounded URL"),
            );
            url.extend_from_slice(&self.authorization_url);
            if let Some(query) = call.target.get(target_base.len()..) {
                url.extend_from_slice(query);
            }
            if url.len() > usize::try_from(self.issuer_limits.request_bytes).expect("u32 fits") {
                self.respond(index, 400, Box::new([]), Box::new([]), Duration::ZERO);
                return;
            }
            if self.recording {
                let target = route::observed_target(&call.target);
                let payload_bytes = u64::try_from(target.len()).expect("bounded target");
                self.observe(Observation::Authorize { connection: owner, target }, payload_bytes);
            }
            self.issuer.step(issuer::Event::Authorize { url: url.into_boxed_slice(), now }, &mut self.outgoing);
            self.deliver(Some(index));
        } else if route == Route::Token || route == Route::Revoke {
            let connection = &mut self.connections[index];
            connection.body.clear();
            connection.content_type = None;
            connection.post = Some(route);
            let mut count = 0;
            for header in &call.headers {
                if header.is(b"content-type") {
                    count += 1;
                    connection.content_type = match header.value.as_ref() {
                        b"application/json" => Some(b"application/json"),
                        b"application/x-www-form-urlencoded" => Some(b"application/x-www-form-urlencoded"),
                        _ => None,
                    };
                }
            }
            if count != 1 {
                connection.content_type = None;
            }
            connection.requests.push(http::Request::Body(Down::Demand {
                read: Read::Arrived { max: connection.body.room().max(1) },
                room: 0,
            }));
        } else if route == Route::Metadata || route == Route::KeySet {
            let connection = &mut self.connections[index];
            connection.post = Some(route);
            let id = self.exchange;
            self.exchange = self.exchange.checked_add(1).expect("finite world HTTP identities");
            connection.active = Some(id);
            let endpoint = if route == Route::Metadata { &self.metadata_url } else { &self.key_set_url };
            let endpoint = endpoint.clone().expect("configured document route");
            let owner = connection.owner;
            if self.recording {
                let payload_bytes = u64::try_from(call.target.len()).expect("bounded target");
                self.observe(Observation::Document { connection: owner, target: call.target }, payload_bytes);
            }
            self.issuer.step(issuer::Event::Get { id, endpoint, now, wall }, &mut self.outgoing);
            self.deliver(Some(index));
        } else {
            self.respond(index, 404, Box::new([]), Box::new([]), Duration::ZERO);
        }
    }

    fn post(&mut self, index: usize, now: Time, wall: Wall) {
        let connection = &mut self.connections[index];
        let Some(route @ (Route::Token | Route::Revoke)) = connection.post else { return };
        let Some(content_type) = connection.content_type.take() else {
            self.respond(index, 400, Box::new([]), Box::new([]), Duration::ZERO);
            return;
        };
        let body = bytes::copy_of(connection.body.as_slice());
        connection.body.clear();
        let owner = connection.owner;
        let id = self.exchange;
        self.exchange = self.exchange.checked_add(1).expect("finite world HTTP identities");
        connection.active = Some(id);
        let endpoint = if route == Route::Revoke {
            self.revoke_endpoint.clone().expect("configured revocation route")
        } else {
            self.token_endpoint.clone()
        };
        let observation = if route == Route::Revoke {
            Observation::Revoke { connection: owner }
        } else {
            Observation::Post { connection: owner }
        };
        self.observe(observation, 0);
        self.issuer.step(
            issuer::Event::Post {
                request: HttpRequest {
                    method: skein_oauth::Method::Post,
                    id,
                    endpoint,
                    content_type,
                    body,
                    deadline: now.saturating_add(Duration::from_secs(60)),
                },
                now,
                wall,
            },
            &mut self.outgoing,
        );
        self.deliver(Some(index));
    }

    fn deliver(&mut self, origin: Option<usize>) {
        if let Some(output) = self.outgoing.pop() {
            match output {
                issuer::Request::Redirect { uri, state, code, client_id } => {
                    let index = origin.expect("authorization emits synchronously");
                    let location = redirect(&uri, &state, &code, client_id.as_deref(), self.http_limits.response)
                        .expect("configured redirect response cap");
                    self.observe(Observation::Redirect { connection: self.connections[index].owner }, 0);
                    self.respond(
                        index,
                        302,
                        Box::new([Header { name: bytes::copy_of(b"location"), value: location }]),
                        Box::new([]),
                        Duration::ZERO,
                    );
                }
                issuer::Request::Http(response) => {
                    let index = self.connections.iter().position(|connection| connection.active == Some(response.id));
                    let route = index.and_then(|index| self.connections[index].post.take());
                    if route != Some(Route::Metadata) && route != Some(Route::KeySet) {
                        self.observe(
                            Observation::Answered {
                                connection: index.map(|index| self.connections[index].owner),
                                id: response.id,
                                status: response.status,
                            },
                            0,
                        );
                    }
                    if let Some(index) = index {
                        self.respond(
                            index,
                            response.status,
                            Box::new([Header {
                                name: bytes::copy_of(b"content-type"),
                                value: bytes::copy_of(b"application/json"),
                            }]),
                            response.body,
                            response.retry_after,
                        );
                    }
                }
                issuer::Request::Refused => {
                    if let Some(index) = origin {
                        self.respond(index, 400, Box::new([]), Box::new([]), Duration::ZERO);
                    }
                }
            }
        }
    }

    fn respond(&mut self, index: usize, status: u16, headers: Box<[Header]>, body: Box<[u8]>, retry_after: Duration) {
        let connection = &mut self.connections[index];
        let mut headers = headers.into_vec();
        if retry_after != Duration::ZERO {
            let seconds = Decimal::of(retry_after.as_nanos().div_euclid(1_000_000_000));
            headers.push(Header { name: bytes::copy_of(b"retry-after"), value: bytes::copy_of(seconds.as_bytes()) });
        }
        let empty = body.is_empty();
        connection.requests.push(http::Request::Discard);
        connection.requests.push(http::Request::Respond(http::Response {
            status,
            headers: headers.into_boxed_slice(),
            body: if empty {
                http::Body::None
            } else {
                http::Body::Length(u64::try_from(body.len()).expect("bounded token response"))
            },
            close: false,
        }));
        connection.response = Some(Response { body, offset: 0 });
        if !empty {
            connection
                .requests
                .push(http::Request::Reply(Down::Demand { read: Read::Nothing, room: self.http_limits.send }));
        }
    }

    fn close(&mut self, index: usize) {
        let connection = &mut self.connections[index];
        if !connection.closing {
            connection.closing = true;
            connection.requests.push(http::Request::Close);
            connection.wire.close(&mut self.face.requests);
        }
    }

    fn progress(&mut self, now: Time, wall: Wall) {
        let env = Env { now, wall, limits: self.http_limits };
        for index in 0..self.connections.len() {
            if self.face.stopping {
                self.close(index);
            }
            let connection = &mut self.connections[index];
            if let Some(up) = connection.plain.pop() {
                http::up(&mut connection.http, &env, up, &mut connection.events, &mut connection.below);
            }
            if let Some(event) = connection.events.pop() {
                self.http_event(index, event, now, wall);
            }
            let connection = &mut self.connections[index];
            if let Some(request) = connection.requests.pop() {
                http::down(&mut connection.http, &env, request, &mut connection.events, &mut connection.below);
            }
            if let Some(down) = connection.below.pop() {
                connection.wire.down(down, &mut connection.plain, &mut self.face.requests);
            }
            connection.wire.pump(&mut connection.plain, &mut self.face.requests);
        }
        if self.issuer.next_deadline().is_some_and(|deadline| deadline <= now) {
            self.issuer.step(issuer::Event::Tick { now, wall }, &mut self.outgoing);
            self.deliver(None);
        }
    }
}

impl Host for Peer {
    fn iterate(&mut self, now: Time, wall: Wall) {
        self.face.up(now, wall);
        for _ in 0..self.face.limits.queue {
            let Some(event) = self.face.events.pop() else { break };
            self.io_event(event);
        }
        self.face.close_listener();
        self.progress(now, wall);
        self.face.down(now, wall);
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        &mut self.face.completions
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        &mut self.face.submissions
    }
    fn work_pending(&self, now: Time) -> bool {
        self.face.work_pending()
            || !self.outgoing.is_empty()
            || self.issuer.next_deadline().is_some_and(|due| due <= now)
            || self.connections.iter().any(|connection| {
                !connection.events.is_empty()
                    || !connection.requests.is_empty()
                    || !connection.below.is_empty()
                    || !connection.plain.is_empty()
                    || connection.wire.work_pending()
            })
    }
    fn next_deadline(&self) -> Option<Time> {
        [self.face.io.next_deadline(), self.issuer.next_deadline()].into_iter().flatten().min()
    }

    fn next_policy_deadline(&self) -> Option<Time> {
        self.issuer.next_deadline()
    }

    fn is_empty(&self) -> bool {
        self.face.is_empty()
            && self.connections.is_empty()
            && self.outgoing.is_empty()
            && self.issuer.next_deadline().is_none()
    }
    fn worst_case(&self) -> u64 {
        self.bound
    }
    fn operations(&self) -> u32 {
        io::operations(&self.face.limits.io).expect("validated io limits")
    }
}

/// The least [`http::Limits::response`] the issuer's answers fit: its URI,
/// two document strings percent-encoded at three bytes per byte, and the
/// 267 fixed bytes of its largest answer's framing.
#[must_use]
pub fn least_response(issuer: &issuer::Limits) -> Option<u32> {
    issuer
        .uri_bytes
        .checked_add(issuer.document.string_bytes.checked_mul(3)?)?
        .checked_add(issuer.document.string_bytes.checked_mul(3)?)?
        .checked_add(267)
}

/// Checked complete process envelope, including issuer plans and observations.
#[must_use]
pub fn worst_case(limits: &Limits, issuer: &issuer::Limits, http: &http::Limits, transport: Transport) -> Option<u64> {
    if http::largest_read(http) > limits.plaintext
        || http::largest_room(http) > limits.ciphertext.checked_sub(1024)?
        || http.body < u64::from(issuer.request_bytes)
        || http.read == 0
        || http.send == 0
        || http.headers < 2
        || http.response < least_response(issuer)?
    {
        return None;
    }
    let connection = transport::worst_case(limits, transport)?
        .checked_add(http::worst_case(http)?)?
        .checked_add(u64::try_from(size_of::<Connection>()).ok()?)?
        .checked_add(Queue::<http::Event>::worst_case(limits.queue)?)?
        .checked_add(Queue::<http::Request>::worst_case(limits.queue)?)?
        .checked_add(Queue::<Down>::worst_case(limits.queue)?)?
        .checked_add(Queue::<Up>::worst_case(limits.queue)?)?
        .checked_add(u64::from(issuer.request_bytes).checked_mul(4)?)?
        .checked_add(u64::from(issuer.document.document_bytes).checked_mul(4)?)?
        .checked_add(
            u64::from(limits.queue).checked_mul(u64::from(limits.plaintext).checked_add(u64::from(http.response))?)?,
        )?;
    face::worst_case(limits)?
        .checked_add(issuer::worst_case(issuer)?)?
        .checked_add(connection.checked_mul(u64::from(limits.connections))?)?
        .checked_add(Queue::<issuer::Request>::worst_case(limits.queue)?)?
        .checked_add(u64::from(issuer.uri_bytes).checked_mul(10)?)?
        .checked_add(2_u64.checked_mul(u64::try_from(b"/.well-known/openid-configuration".len()).ok()?)?)?
        .checked_add(u64::from(limits.observations).checked_mul(u64::try_from(size_of::<Observation>()).ok()?)?)?
        .checked_add(u64::from(limits.observation_bytes))
}

fn path(url: &[u8], transport: Transport) -> Result<Box<[u8]>, Error> {
    let remainder = match transport {
        Transport::Plaintext => url.strip_prefix(b"http://"),
        Transport::Tls => url.strip_prefix(b"https://"),
    }
    .ok_or(Error::Limits)?;
    let at = remainder.iter().position(|byte| *byte == b'/').ok_or(Error::Limits)?;
    let target = &remainder[at..];
    if target.contains(&b'?') || target.iter().any(|byte| !(0x21..=0x7e).contains(byte)) {
        return Err(Error::Limits);
    }
    Ok(bytes::copy_of(target))
}

fn redirect(uri: &[u8], state: &[u8], code: &[u8], client_id: Option<&[u8]>, cap: u32) -> Option<Box<[u8]>> {
    let size = uri
        .len()
        .checked_add(state.len().checked_mul(3)?)?
        .checked_add(code.len().checked_mul(3)?)?
        .checked_add(13)?
        .checked_add(client_id.map_or(0, <[u8]>::len).checked_mul(3)?)?
        .checked_add(11)?;
    if size > usize::try_from(cap).ok()? {
        return None;
    }
    let mut location = Vec::with_capacity(size);
    location.extend_from_slice(uri);
    location.extend_from_slice(if uri.contains(&b'?') { b"&code=" } else { b"?code=" });
    percent(code, &mut location);
    location.extend_from_slice(b"&state=");
    percent(state, &mut location);
    if let Some(client_id) = client_id {
        location.extend_from_slice(b"&client_id=");
        percent(client_id, &mut location);
    }
    Some(location.into_boxed_slice())
}

fn percent(value: &[u8], out: &mut Vec<u8>) {
    const HEX: &[u8] = b"0123456789ABCDEF";
    for &byte in value {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte);
        } else {
            out.extend_from_slice(&[b'%', HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 15)]]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Limits, Transport, http, issuer, least_response, redirect, worst_case};
    use skein_lib::Duration;

    #[test]
    fn redirect_preserves_registered_uri_and_escapes_code_and_state() {
        assert_eq!(
            redirect(b"http://127.0.0.1:1234/callback", b"state with +&%", b"code?=#", None, 1024).unwrap().as_ref(),
            b"http://127.0.0.1:1234/callback?code=code%3F%3D%23&state=state%20with%20%2B%26%25"
        );
        assert_eq!(
            redirect(b"http://127.0.0.1:1234/callback?kept=yes", b"0123456789abcdef", b"code", None, 1024)
                .unwrap()
                .as_ref(),
            b"http://127.0.0.1:1234/callback?kept=yes&code=code&state=0123456789abcdef"
        );
        assert_eq!(
            redirect(b"http://127.0.0.1:1234/callback", b"state", b"code", Some(b"client +"), 1024).unwrap().as_ref(),
            b"http://127.0.0.1:1234/callback?code=code&state=state&client_id=client%20%2B"
        );
        assert!(redirect(b"http://127.0.0.1:1234/callback", b"state", b"code", None, 4).is_none());
    }

    #[test]
    fn response_floor_admits_exactly_the_bound() {
        let limits = Limits {
            io: skein_io::Limits {
                sockets: 3,
                refusals: 1,
                intake: 32_768,
                receive: 1024,
                output: 32_768,
                sends: 2,
                accepts: 1,
                backlog: 1,
                close_timeout: Duration::from_secs(1),
                names: 0,
                retry: Duration::from_millis(1),
            },
            connections: 2,
            queue: 64,
            plaintext: 32_768,
            ciphertext: 32_768,
            observations: 16,
            observation_bytes: 4096,
        };
        let issuer = issuer::Limits {
            document: skein_oauth::Limits {
                document_bytes: 1024,
                string_bytes: 256,
                token_bytes: 256,
                client_bytes: 64,
                detail_bytes: 64,
                record_bytes: 2048,
                depth: 8,
                tokens: 64,
                issued_client_bytes: 0,
                granted_bytes: 64,
                subject_bytes: 0,
                id_token_bytes: 1024,
            },
            uri_bytes: 256,
            request_bytes: 1024,
            codes: 2,
            rotations: 2,
            plans: 2,
            carried: 1,
        };
        let floor = least_response(&issuer).expect("the fixture's response floor fits u32");
        for (response, admitted) in [(floor, true), (floor - 1, false)] {
            let http = http::Limits { head: 2048, headers: 16, body: 1024, read: 256, response, send: 512 };
            assert_eq!(worst_case(&limits, &issuer, &http, Transport::Plaintext).is_some(), admitted, "{response}");
        }
    }
}
