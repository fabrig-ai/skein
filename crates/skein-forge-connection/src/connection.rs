//! Forge connection (forge-connection.md, sections 3 and 8).
use alloc::boxed::Box;
use skein_forge::forgejo::plans::{self, Fields, plan::Range};
use skein_forge::forgejo::{decode, header};
use skein_forge::github;

/// A forge connection's state; the component arms its deadline.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    /// Nothing held.
    Closed,
    /// io's connect, until the call's deadline (or a start read's).
    Connecting,
    /// TLS's handshake, until the same deadline.
    Handshaking,
    /// The request going down; `written` once its first byte went.
    Sending { written: bool },
    /// The head, then the body as the decoder demands.
    Reading,
    /// Kept for its forge until `idle`.
    Idle,
}

use crate::{
    grow::{Grow, Tail},
    limits::{self, Limits, Shared},
    settings::{Forge, Kind, Transport},
};
use skein_forge::forgejo::{client as documents, request};
use skein_http::client as http;
use skein_json::tokenizer;
use skein_lib::{
    Env, Intake, List, Queue, Secret, Time, Token, Writer, bytes,
    stream::{Down, Read, Up},
};
/// A component-selected request and its complete Authorization value.
#[derive(Debug)]
pub(crate) enum Exchange {
    Forgejo { request: request::Request, range: Option<Range>, authorization: Secret },
    GitHub { exchange: github::plans::plan::Exchange, authorization: Option<Secret> },
}
/// Exchange events belong to the connection's dialect.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Heard {
    Forgejo(plans::Heard),
    GitHub(github::plans::Heard),
}
/// The selected encoder's refusal.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Refusal {
    Forgejo(documents::Refusal),
    GitHub(github::request::Refusal),
}
/// Whether any request byte may have reached the peer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Failure {
    Unseen,
    Seen,
}
#[derive(Debug)]
enum Request {
    Forgejo(request::Request),
    GitHub(Box<github::plans::plan::Exchange>),
}
#[derive(Debug)]
enum Decoder {
    Forgejo(Box<documents::Decoder>),
    GitHub(Box<github::decode::Decoder>),
}
#[derive(Debug)]
enum Body {
    Forgejo(plans::Body),
    GitHub(github::plans::Body),
}
impl Body {
    fn closes(&self) -> bool {
        match self {
            Self::Forgejo(body) => match body {
                plans::Body::Malformed(_) | plans::Body::Raw { more: true, .. } => true,
                plans::Body::Document(_) | plans::Body::Raw { more: false, .. } | plans::Body::Empty => false,
            },
            Self::GitHub(body) => match body {
                github::plans::Body::Malformed(_) | github::plans::Body::Raw { more: true, .. } => true,
                github::plans::Body::Document(_)
                | github::plans::Body::Raw { more: false, .. }
                | github::plans::Body::Empty => false,
            },
        }
    }
}
enum JsonStep {
    Demand(tokenizer::Request),
    Row(Heard, tokenizer::Request),
    Done(Body),
    Malformed(decode::Malformed),
}
/// One exchange's response body state.
#[derive(Debug)]
enum Receiving {
    Json(Decoder),
    Raw { grow: Grow, max: u32, remaining: Option<u64> },
    Tail { tail: Tail, read: u64, remaining: Option<u64> },
    Empty,
}
// Host, User-Agent, Accept, pin, Authorization and Content-Type;
// HTTP adds Content-Length outside this list.
const GITHUB_HEADERS: u32 = 6;
/// The body stream below the tokenizer, bounded by the existing read cap.
struct JsonBody {
    intake: Intake,
    read: Option<Read>,
    below: bool,
    ended: bool,
}
/// One connection's machines, socket identity, request and pending output.
pub(crate) struct Connection {
    pub(crate) forge: u16,
    pub(crate) origin: Option<u32>,
    pub(crate) draining: bool,
    kind: Kind,
    pub(crate) phase: Phase,
    pub(crate) deadline: Option<Time>,
    pub(crate) socket: Option<Token>,
    pub(crate) closing: bool,
    pub(crate) written: bool,
    http: http::Client,
    tls: Option<skein_tls::client::Client>,
    json: tokenizer::Tokenizer,
    shared: Shared,
    http_up: Queue<http::Event>,
    http_down: Queue<Down>,
    tls_up: Queue<skein_tls::client::Event>,
    tls_down: Queue<Down>,
    json_up: Queue<tokenizer::Event>,
    json_down: Queue<Down>,
    json_next: Option<tokenizer::Request>,
    json_body: JsonBody,
    heard: Queue<Heard>,
    request: Option<Request>,
    receive: Receiving,
    done: Option<Body>,
    after: u64,
    upload: Option<Box<[u8]>>,
    uploaded: u32,
    raw_range: Range,
    raw_cap: u32,
    json_limits: tokenizer::Limits,
}
impl Connection {
    #[cfg(test)]
    pub fn new(forge: &Forge, l: &Limits, shared: Shared, deadline: Time) -> Self {
        Self::at_origin(forge, None, l, shared, deadline)
    }
    #[expect(clippy::manual_map, reason = "the programming model excludes closures")]
    pub fn at_origin(forge: &Forge, origin: Option<u32>, l: &Limits, shared: Shared, deadline: Time) -> Self {
        let configured = match origin {
            Some(at) => Some(forge.log_origins.get(at).expect("configured log origin")),
            None => None,
        };
        let tls = match (&forge.transport, configured) {
            (Transport::Plaintext { .. } | Transport::Tls { .. }, Some(configured)) if !configured.origin.tls => None,
            (Transport::Tls { trust, .. }, Some(configured)) => Some(skein_tls::client::Client::new(
                trust,
                crate::settings::log_name(&configured.origin).expect("checked DNS name"),
                &l.tls,
            )),
            (Transport::Plaintext { .. }, Some(_)) => unreachable!("HTTPS origin requires trust"),
            (Transport::Plaintext { .. }, None) => None,
            (Transport::Tls { trust, server_name }, None) => {
                Some(skein_tls::client::Client::new(trust, server_name.clone(), &l.tls))
            }
        };
        Self {
            forge: forge.number,
            origin,
            draining: false,
            kind: forge.kind,
            phase: Phase::Connecting,
            deadline: Some(deadline),
            socket: None,
            closing: false,
            written: false,
            http: http::Client::new(&limits::http(l, tls.is_some(), &shared.io)),
            tls,
            json: tokenizer::Tokenizer::new(&limits::documents(l).json),
            shared,
            http_up: Queue::with_capacity(8),
            http_down: Queue::with_capacity(8),
            tls_up: Queue::with_capacity(8),
            tls_down: Queue::with_capacity(8),
            json_up: Queue::with_capacity(8),
            json_down: Queue::with_capacity(8),
            json_next: None,
            json_body: JsonBody {
                intake: Intake::with_capacity(l.chunk.max(5)),
                read: None,
                below: false,
                ended: false,
            },
            heard: Queue::with_capacity(16),
            request: None,
            receive: Receiving::Empty,
            done: None,
            after: 0,
            upload: None,
            uploaded: 0,
            raw_range: Range::Head(0),
            raw_cap: shared.answer_bytes,
            json_limits: limits::documents(l).json,
        }
    }
    fn host(&self, forge: &Forge) -> Box<[u8]> {
        match self.origin {
            Some(at) => crate::settings::log_host(forge.log_origins.get(at).expect("configured origin")),
            None => crate::settings::host(forge).expect("checked forge host"),
        }
    }
    pub fn start(
        &mut self,
        env: &Env<Limits>,
        forge: &Forge,
        user_agent: &[u8],
        exchange: Exchange,
    ) -> Result<(), Refusal> {
        assert_eq!(self.kind, forge.kind, "connection belongs to its forge dialect");
        let (request, method, target, upload, mut headers, range) = match exchange {
            Exchange::Forgejo { request, range, authorization } => {
                assert_eq!(self.kind, Kind::Forgejo, "exchange belongs to its connection dialect");
                let encoded = match documents::encode(&request, &limits::documents(&env.limits)) {
                    Ok(encoded) => encoded,
                    Err(error) => return Err(Refusal::Forgejo(error)),
                };
                // Host, Authorization, User-Agent, Content-Type and Range; HTTP adds Content-Length.
                let mut headers = List::with_capacity(5);
                headers.push(field(b"Host", &self.host(forge))).expect("five Forgejo header fields");
                headers.push(field(b"Authorization", authorization.expose())).expect("five Forgejo header fields");
                headers.push(field(b"User-Agent", user_agent)).expect("five Forgejo header fields");
                let method = match encoded.method {
                    request::Method::Get => skein_http::Method::Get,
                    request::Method::Post => skein_http::Method::Post,
                    request::Method::Put => skein_http::Method::Put,
                    request::Method::Patch => skein_http::Method::Patch,
                    request::Method::Delete => skein_http::Method::Delete,
                };
                (Request::Forgejo(request), method, encoded.target, encoded.body, headers, range)
            }
            Exchange::GitHub { exchange, authorization } => {
                let request = &exchange.request;
                assert_eq!(self.kind, Kind::GitHub, "exchange belongs to its connection dialect");
                let encoded = match github::request::encode(request, &limits::github_documents(&env.limits)) {
                    Ok(encoded) => encoded,
                    Err(error) => return Err(Refusal::GitHub(error)),
                };
                match (encoded.auth, &authorization) {
                    (github::request::Auth::None, None)
                    | (github::request::Auth::App | github::request::Auth::Installation, Some(_)) => {}
                    (github::request::Auth::None, Some(_))
                    | (github::request::Auth::App | github::request::Auth::Installation, None) => {
                        unreachable!("component selects Authorization according to Encoded.auth");
                    }
                }
                let mut headers = List::with_capacity(GITHUB_HEADERS);
                headers.push(field(b"Host", &self.host(forge))).expect("six GitHub header fields");
                headers.push(field(b"User-Agent", user_agent)).expect("six GitHub header fields");
                for (name, value) in github::request::fields(&encoded) {
                    headers.push(field(name, value)).expect("six GitHub header fields");
                }
                if let Some(authorization) = authorization {
                    headers.push(field(b"Authorization", authorization.expose())).expect("six GitHub header fields");
                }
                let method = match encoded.method {
                    github::request::Method::Get => skein_http::Method::Get,
                    github::request::Method::Post => skein_http::Method::Post,
                    github::request::Method::Put => skein_http::Method::Put,
                    github::request::Method::Patch => skein_http::Method::Patch,
                    github::request::Method::Delete => skein_http::Method::Delete,
                };
                (Request::GitHub(Box::new(exchange)), method, encoded.target, encoded.body, headers, None)
            }
        };
        let body = match &upload {
            Some(body) => {
                headers.push(field(b"Content-Type", b"application/json")).expect("dialect's header capacity");
                http::Body::Length(u64::try_from(body.len()).expect("bounded body"))
            }
            None => http::Body::None,
        };
        if let Some(range) = range {
            let (prefix, max) = match range {
                Range::Head(n) => (b"bytes=0-".as_slice(), n),
                Range::Tail(n) => (b"bytes=-".as_slice(), n),
            };
            let decimal = skein_lib::Decimal::of(u64::from(max));
            let mut value = Writer::new(decimal.as_bytes().len().saturating_add(prefix.len()));
            value.put(prefix).expect("range prefix measured");
            value.put(decimal.as_bytes()).expect("range measured");
            headers
                .push(skein_http::Header { name: Box::from(&b"Range"[..]), value: value.finish() })
                .expect("five Forgejo header fields");
        }
        self.clear_json_body();
        self.json_limits = limits::documents(&env.limits).json;
        self.request = Some(request);
        self.upload = upload;
        self.uploaded = 0;
        self.raw_range = range.unwrap_or(Range::Head(self.raw_cap));
        self.done = None;
        self.written = false;
        self.after = 0;
        self.phase = Phase::Sending { written: false };
        self.http_down(
            env,
            http::Request::Call(http::Call { method, target, headers: headers.into_boxed(), body, close: false }),
        );
        if self.upload.is_some() {
            self.http_down(
                env,
                http::Request::Upload(Down::Demand { read: Read::Nothing, room: self.http_env(env).limits.send }),
            );
        }
        Ok(())
    }
    pub fn drain_redirect(&mut self, env: &Env<Limits>, io: &mut Queue<skein_io::Request>, budget: u32) -> bool {
        if !self.draining {
            return false;
        }
        if self.deadline <= Some(env.now) {
            self.timeout();
        }
        self.pump(env, io, budget);
        drop(self.heard());
        if self.phase == Phase::Idle || self.phase == Phase::Closed {
            self.draining = false;
        }
        self.ready()
    }
    pub fn discard_redirect(&mut self, env: &Env<Limits>) {
        if self.phase == Phase::Reading {
            self.clear_json_body();
            self.draining = true;
            self.receive = Receiving::Empty;
            self.json_next = None;
            for _ in 0..self.json_up.len() {
                drop(self.json_up.pop());
            }
            for _ in 0..self.json_down.len() {
                drop(self.json_down.pop());
            }
            self.http_down(env, http::Request::Discard);
        }
    }
    pub fn connect(&mut self, env: &Env<Limits>) {
        match &mut self.tls {
            Some(tls) => {
                self.phase = Phase::Handshaking;
                let tls_env = Env { now: env.now, wall: env.wall, limits: env.limits.tls };
                skein_tls::client::down(
                    tls,
                    &tls_env,
                    skein_tls::client::Request::Handshake,
                    &mut self.tls_up,
                    &mut self.tls_down,
                );
            }
            None => {
                self.phase = Phase::Idle;
            }
        }
    }
    pub fn stream(&mut self, env: &Env<Limits>, event: Up) {
        if self.closing || self.phase == Phase::Closed {
            return;
        }
        match &mut self.tls {
            Some(tls) => {
                let tls_env = Env { now: env.now, wall: env.wall, limits: env.limits.tls };
                skein_tls::client::up(tls, &tls_env, event, &mut self.tls_up, &mut self.tls_down);
            }
            None => {
                let h = self.http_env(env);
                http::up(&mut self.http, &h, event, &mut self.http_up, &mut self.http_down);
            }
        }
    }
    fn http_env(&self, env: &Env<Limits>) -> Env<http::Limits> {
        Env { now: env.now, wall: env.wall, limits: limits::http(&env.limits, self.tls.is_some(), &self.shared.io) }
    }
    fn http_down(&mut self, env: &Env<Limits>, request: http::Request) {
        let h = self.http_env(env);
        http::down(&mut self.http, &h, request, &mut self.http_up, &mut self.http_down);
    }
    fn json_down(&mut self, env: &Env<Limits>, request: tokenizer::Request) {
        let j = Env { now: env.now, wall: env.wall, limits: self.json_limits };
        tokenizer::down(&mut self.json, &j, request, &mut self.json_up, &mut self.json_down);
    }
    /// Routes at most a fixed number of child-machine movements. Remaining work stays owned.
    pub fn pump(&mut self, env: &Env<Limits>, io: &mut Queue<skein_io::Request>, budget: u32) {
        let mut emitted = 0_u32;
        for _ in 0_u32..64 {
            if emitted >= budget || !self.heard.is_empty() || self.closing || self.phase == Phase::Closed {
                break;
            }
            if let Some(down) = self.tls_down.pop() {
                self.send(down, io);
                emitted = emitted.saturating_add(1);
                continue;
            }
            if let Some(event) = self.tls_up.pop() {
                self.tls_event(env, event);
                continue;
            }
            if let Some(down) = self.http_down.pop() {
                match &mut self.tls {
                    Some(tls) => {
                        let e = Env { now: env.now, wall: env.wall, limits: env.limits.tls };
                        skein_tls::client::down(
                            tls,
                            &e,
                            skein_tls::client::Request::Stream(down),
                            &mut self.tls_up,
                            &mut self.tls_down,
                        );
                    }
                    None => {
                        self.send(down, io);
                        emitted = emitted.saturating_add(1);
                    }
                }
                continue;
            }
            if let Some(down) = self.json_down.pop() {
                self.json_demand(env, down);
                continue;
            }
            if let Some(request) = self.json_next.take() {
                self.json_down(env, request);
                continue;
            }
            if let Some(event) = self.json_up.pop() {
                self.json_event(env, event);
                continue;
            }
            if self.pump_json_body(env) {
                continue;
            }
            if let Some(event) = self.http_up.pop() {
                self.http_event(env, event);
                continue;
            }
            break;
        }
    }
    fn clear_json_body(&mut self) {
        self.json_body.intake.clear();
        self.json_body.read = None;
        self.json_body.below = false;
        self.json_body.ended = false;
    }
    fn json_demand(&mut self, env: &Env<Limits>, down: Down) {
        match down {
            Down::Demand { read, room } => {
                assert_eq!(room, 0, "JSON bodies only read");
                if read == Read::Nothing {
                    self.json_body.read = None;
                    self.json_body.intake.clear();
                    if self.json_body.below {
                        self.json_body.below = false;
                        self.http_down(env, http::Request::Body(Down::Demand { read, room: 0 }));
                    }
                } else {
                    assert!(self.json_body.read.is_none(), "one tokenizer demand at a time");
                    self.json_body.read = Some(read);
                }
            }
            Down::Send(_) | Down::Finish => unreachable!("JSON bodies only read"),
        }
    }
    /// One local answer or refill is one of the pump's bounded movements.
    fn pump_json_body(&mut self, env: &Env<Limits>) -> bool {
        let Some(read) = self.json_body.read else { return false };
        let event = if let Some(bytes) = self.json_body.intake.meet(read) {
            Up::Bytes(bytes)
        } else if self.json_body.ended {
            self.json_body.intake.clear();
            Up::End
        } else {
            // HTTP may already have its final Bytes, End and Done queued.
            // Consume those before asking its body stream for another read.
            if self.json_body.below || !self.http_up.is_empty() {
                return false;
            }
            let max = self.json_body.intake.room();
            assert!(max > 0, "a full intake meets every tokenizer demand");
            self.json_body.below = true;
            self.http_down(env, http::Request::Body(Down::Demand { read: Read::Arrived { max }, room: 0 }));
            return true;
        };
        self.json_body.read = None;
        let j = Env { now: env.now, wall: env.wall, limits: self.json_limits };
        tokenizer::up(&mut self.json, &j, event, &mut self.json_up, &mut self.json_down);
        true
    }
    fn send(&mut self, down: Down, io: &mut Queue<skein_io::Request>) {
        if sending(&down) && self.request.is_some() {
            self.written = true;
            self.phase = if self.upload.is_some() { Phase::Sending { written: true } } else { Phase::Reading };
        }
        if let Some(socket) = self.socket {
            io.push(skein_io::Request::Stream { stream: socket, down });
        }
    }
    fn tls_event(&mut self, env: &Env<Limits>, event: skein_tls::client::Event) {
        match event {
            skein_tls::client::Event::Ready(_) => {
                self.phase = Phase::Idle;
            }
            skein_tls::client::Event::Stream(event) => {
                let h = self.http_env(env);
                http::up(&mut self.http, &h, event, &mut self.http_up, &mut self.http_down);
            }
            skein_tls::client::Event::Failed(_) => {
                self.failed(if self.written { Failure::Seen } else { Failure::Unseen });
            }
            skein_tls::client::Event::Closed => {}
        }
    }
    fn json_body_event(&mut self, event: Up) {
        match event {
            Up::Bytes(bytes) => {
                assert!(self.json_body.below, "one answer to a JSON body refill");
                self.json_body.below = false;
                self.json_body.intake.append(&bytes).expect("refill fits the remaining intake room");
            }
            Up::End => {
                self.json_body.below = false;
                self.json_body.ended = true;
            }
            Up::Failed(_) => self.failed(Failure::Seen),
            Up::Room => unreachable!("JSON bodies only read"),
        }
    }
    fn http_event(&mut self, env: &Env<Limits>, event: http::Event) {
        match event {
            http::Event::Response(response) => self.response(env, response),
            http::Event::Body(event) => match &mut self.receive {
                Receiving::Json(_) => self.json_body_event(event),
                Receiving::Empty => {
                    drop(event);
                }
                Receiving::Tail { tail, read, remaining } => match event {
                    Up::Bytes(piece) => {
                        let length = u64::try_from(piece.len()).expect("Bounded piece");
                        *read = read.saturating_add(length);
                        if let Some(left) = remaining {
                            *left = left.saturating_sub(length);
                        }
                        if *read > u64::from(env.limits.body_bytes_in) {
                            self.receive = Receiving::Empty;
                            self.malformed(decode::Malformed::Bound { field: b"log" });
                            self.phase = Phase::Closed;
                            self.deadline = None;
                        } else {
                            tail.write(&piece);
                            self.raw_demand(env);
                        }
                    }
                    Up::End => {}
                    Up::Failed(_) => self.failed(Failure::Seen),
                    Up::Room => unreachable!("Tail bodies only read"),
                },
                Receiving::Raw { grow, max, remaining } => match event {
                    Up::Bytes(piece) => {
                        if let Some(left) = remaining {
                            *left = left.saturating_sub(u64::try_from(piece.len()).expect("bounded piece"));
                        }
                        let more = grow.write(&piece).is_err() || grow.len() > *max;
                        if more {
                            let grow = match core::mem::replace(&mut self.receive, Receiving::Empty) {
                                Receiving::Raw { grow, .. } => grow,
                                Receiving::Json(_) | Receiving::Tail { .. } | Receiving::Empty => {
                                    unreachable!("raw body")
                                }
                            };
                            let done = self.raw(grow.finish(), true);
                            self.finished(done);
                            self.phase = Phase::Closed;
                            self.deadline = None;
                        } else {
                            self.raw_demand(env);
                        }
                    }
                    Up::End => {}
                    Up::Failed(_) => self.failed(Failure::Seen),
                    Up::Room => unreachable!("raw bodies only read"),
                },
            },
            http::Event::Upload(event) => self.upload(env, event),
            http::Event::Done(reuse) => {
                let done = match core::mem::replace(&mut self.receive, Receiving::Empty) {
                    Receiving::Json(_) => {
                        self.done.take().unwrap_or(self.malformed_body(decode::Malformed::Missing { field: b"" }))
                    }
                    Receiving::Empty => self.done.take().unwrap_or(self.empty()),
                    Receiving::Raw { grow, max, .. } => {
                        let more = grow.len() > max;
                        self.raw(grow.finish(), more)
                    }
                    Receiving::Tail { tail, .. } => {
                        let more = tail.dropped();
                        self.raw(tail.finish(), more)
                    }
                };
                let close = reuse == http::Reuse::Close || done.closes();
                self.clear_json_body();
                self.finished(done);
                self.request = None;
                self.upload = None;
                self.phase = if close { Phase::Closed } else { Phase::Idle };
                self.deadline = if close { None } else { Some(env.now.saturating_add(env.limits.idle)) };
            }
            http::Event::Failed(error) => self.failed(match error {
                http::Error::Closed(_) | http::Error::Refused(_) => Failure::Unseen,
                http::Error::Truncated { .. }
                | http::Error::Stream(_)
                | http::Error::Status
                | http::Error::Version
                | http::Error::Header
                | http::Error::HeadTooLong
                | http::Error::TooManyHeaders
                | http::Error::Framing
                | http::Error::ChunkSize
                | http::Error::Chunk
                | http::Error::Trailer
                | http::Error::Upgrade => Failure::Seen,
            }),
            http::Event::Closed => {}
        }
    }
    fn response(&mut self, env: &Env<Limits>, response: http::Response) {
        match self.request.as_ref().expect("head belongs to a request") {
            Request::Forgejo(_) => self.forgejo_response(env, response),
            Request::GitHub(exchange) => {
                let request = &exchange.request;
                let mut l = limits::github_documents(&env.limits);
                if github::decode::kind(request) == Some(github::document::Kind::Minted) && response.status == 201 {
                    l = limits::github_mint_documents(&env.limits);
                }
                if github::decode::kind(request) == Some(github::document::Kind::LogDelivery) {
                    l.json.length = env.limits.delivery_bytes;
                }
                self.json_limits = l.json;
                let mut head = github::header::Head::new();
                for field in &response.headers {
                    github::header::field(&mut head, &field.name, &field.value, &l);
                }
                self.heard.push(Heard::GitHub(github::plans::Heard::Head { status: response.status, head }));
                let decoder = if response.status >= 300 {
                    Some(github::decode::decoder(github::document::Kind::Error, &l))
                } else {
                    exchange.decoder(&l)
                };
                match decoder {
                    Some(decoder) => {
                        self.receive = Receiving::Json(Decoder::GitHub(Box::new(decoder)));
                        self.json = tokenizer::Tokenizer::new(&l.json);
                        self.json_down(env, tokenizer::Request::Next);
                    }
                    None => {
                        if github_raw_request(request) {
                            let retention = exchange
                                .retention
                                .unwrap_or(github::plans::plan::Retention::Head { bytes: self.shared.answer_bytes });
                            let (range, cap) = match retention {
                                github::plans::plan::Retention::Head { bytes }
                                | github::plans::plan::Retention::Whole { bytes } => (Range::Head(bytes), bytes),
                                github::plans::plan::Retention::Tail { bytes } => (Range::Tail(bytes), bytes),
                            };
                            self.raw_range = range;
                            self.raw_cap = cap.min(self.shared.answer_bytes);
                            self.begin_raw(env, response.framing);
                        } else {
                            self.receive = Receiving::Empty;
                            self.http_down(env, http::Request::Discard);
                        }
                    }
                }
            }
        }
    }
    fn forgejo_response(&mut self, env: &Env<Limits>, response: http::Response) {
        let date = match response.header(b"Date") {
            Some(date) => skein_forge::forgejo::time::http_date(date),
            None => None,
        };
        let retry_after = match response.header(b"Retry-After") {
            Some(value) => header::retry_after(value, date.unwrap_or(env.wall)),
            None => None,
        };
        let has_more = match response.header(b"X-HasMore") {
            Some(value) => header::has_more(value),
            None => None,
        };
        let content_range = match response.header(b"Content-Range") {
            Some(value) => header::content_range(value),
            None => None,
        };
        let close = match response.header(b"Connection") {
            Some(value) => value.eq_ignore_ascii_case(b"close"),
            None => false,
        };
        self.heard.push(Heard::Forgejo(plans::Heard::Head {
            status: response.status,
            fields: Fields { date, retry_after, has_more, content_range, close },
        }));
        let l = limits::documents(&env.limits);
        let request = match self.request.as_ref().expect("head belongs to a request") {
            Request::Forgejo(request) => request,
            Request::GitHub(_) => unreachable!("Forgejo response belongs to a Forgejo request"),
        };
        if response.status == 401 || response.status == 403 || response.status == 404 {
            self.receive = Receiving::Empty;
            self.http_down(env, http::Request::Discard);
            return;
        }
        let decoder =
            if response.status >= 300 { Some(documents::error_decoder(&l)) } else { documents::decoder(request, &l) };
        match decoder {
            Some(mut decoder) => {
                documents::keep_after(&mut decoder, self.after);
                self.receive = Receiving::Json(Decoder::Forgejo(Box::new(decoder)));
                self.json = tokenizer::Tokenizer::new(&l.json);
                self.json_down(env, tokenizer::Request::Next);
            }
            None => {
                if raw_request(request) {
                    self.begin_raw(env, response.framing);
                } else {
                    self.receive = Receiving::Empty;
                    self.http_down(env, http::Request::Discard);
                }
            }
        }
    }
    fn begin_raw(&mut self, env: &Env<Limits>, framing: http::Framing) {
        let remaining = match framing {
            http::Framing::Length(length) => Some(length),
            http::Framing::Empty => Some(0),
            http::Framing::Chunked | http::Framing::UntilEnd => None,
        };
        self.receive = match self.raw_range {
            Range::Head(max) => Receiving::Raw {
                grow: Grow::new(env.limits.chunk, max.saturating_add(1).min(self.raw_cap)),
                max,
                remaining,
            },
            Range::Tail(max) => {
                Receiving::Tail { tail: Tail::new(env.limits.chunk, max.min(self.raw_cap)), read: 0, remaining }
            }
        };
        self.raw_demand(env);
    }
    fn raw_demand(&mut self, env: &Env<Limits>) {
        let count = match &self.receive {
            Receiving::Raw { grow, remaining, .. } => match remaining {
                Some(left) => grow.room().min(env.limits.chunk).min(u32::try_from(*left).unwrap_or(u32::MAX)).max(1),
                None => 1,
            },
            Receiving::Tail { remaining, .. } => match remaining {
                Some(left) => env.limits.chunk.min(u32::try_from(*left).unwrap_or(u32::MAX)).max(1),
                None => 1,
            },
            Receiving::Json(_) | Receiving::Empty => return,
        };
        self.http_down(env, http::Request::Body(Down::Demand { read: Read::Fill(count), room: 0 }));
    }
    fn upload(&mut self, env: &Env<Limits>, event: Up) {
        match event {
            Up::Room => {
                let Some(body) = &self.upload else {
                    return;
                };
                let start = usize::try_from(self.uploaded).expect("bounded position");
                let end = body
                    .len()
                    .min(start.saturating_add(usize::try_from(self.http_env(env).limits.send).expect("send fits")));
                let piece = bytes::copy_of(body.get(start..end).expect("bounded position"));
                self.uploaded = u32::try_from(end).expect("bounded body");
                let finished = end == body.len();
                self.http_down(env, http::Request::Upload(Down::Send(piece)));
                if finished {
                    self.upload = None;
                    self.phase = Phase::Reading;
                    self.http_down(env, http::Request::Upload(Down::Finish));
                } else {
                    self.http_down(
                        env,
                        http::Request::Upload(Down::Demand {
                            read: Read::Nothing,
                            room: self.http_env(env).limits.send,
                        }),
                    );
                }
            }
            Up::Failed(_) | Up::End => {
                self.upload = None;
            }
            Up::Bytes(_) => unreachable!("uploads only write"),
        }
    }
    fn json_event(&mut self, env: &Env<Limits>, event: tokenizer::Event) {
        let Some(decoder) = self.decoder() else {
            return;
        };
        let step = match decoder {
            Decoder::Forgejo(decoder) => match documents::step(decoder, &limits::documents(&env.limits), &event) {
                decode::Step::Demand(request) => JsonStep::Demand(request),
                decode::Step::Row(row, request) => JsonStep::Row(Heard::Forgejo(plans::Heard::Row(row)), request),
                decode::Step::Done(document) => JsonStep::Done(Body::Forgejo(plans::Body::Document(document))),
                decode::Step::Malformed(error) => JsonStep::Malformed(error),
            },
            Decoder::GitHub(decoder) => {
                let (request, step) = decoder.step(event);
                match step {
                    github::decode::Step::Demand => JsonStep::Demand(request),
                    github::decode::Step::Row(row) => {
                        JsonStep::Row(Heard::GitHub(github::plans::Heard::Row(row)), request)
                    }
                    github::decode::Step::Done(github::document::Document::Minted(minted))
                        if minted.token.len() > usize::try_from(env.limits.token_bytes).expect("token bound") =>
                    {
                        JsonStep::Malformed(decode::Malformed::Bound { field: b"token" })
                    }
                    github::decode::Step::Done(document) => {
                        JsonStep::Done(Body::GitHub(github::plans::Body::Document(document)))
                    }
                    github::decode::Step::Malformed(error) => JsonStep::Malformed(error),
                }
            }
        };
        match step {
            JsonStep::Demand(request) => self.json_down(env, request),
            JsonStep::Row(heard, request) => {
                self.heard.push(heard);
                self.json_next = Some(request);
            }
            JsonStep::Done(body) => self.done = Some(body),
            JsonStep::Malformed(decode::Malformed::Json(tokenizer::Error::Stream(_))) => self.failed(Failure::Seen),
            JsonStep::Malformed(error) => {
                self.clear_json_body();
                self.receive = Receiving::Empty;
                self.phase = Phase::Closed;
                self.deadline = None;
                self.malformed(error);
            }
        }
    }
    fn raw(&self, bytes: Box<[u8]>, more: bool) -> Body {
        match self.kind {
            Kind::Forgejo => Body::Forgejo(plans::Body::Raw { bytes, more }),
            Kind::GitHub => Body::GitHub(github::plans::Body::Raw { bytes, more }),
        }
    }
    fn empty(&self) -> Body {
        match self.kind {
            Kind::Forgejo => Body::Forgejo(plans::Body::Empty),
            Kind::GitHub => Body::GitHub(github::plans::Body::Empty),
        }
    }
    fn malformed_body(&self, error: decode::Malformed) -> Body {
        match self.kind {
            Kind::Forgejo => Body::Forgejo(plans::Body::Malformed(error)),
            Kind::GitHub => Body::GitHub(github::plans::Body::Malformed(error)),
        }
    }
    fn malformed(&mut self, error: decode::Malformed) {
        let done = self.malformed_body(error);
        self.finished(done);
    }
    fn finished(&mut self, done: Body) {
        self.request = None;
        let heard = match done {
            Body::Forgejo(body) => Heard::Forgejo(plans::Heard::Done(body)),
            Body::GitHub(body) => Heard::GitHub(github::plans::Heard::Done(body)),
        };
        self.heard.push(heard);
    }
    pub fn failed(&mut self, failure: Failure) {
        if self.phase == Phase::Closed {
            return;
        }
        self.clear_json_body();
        self.phase = Phase::Closed;
        self.deadline = None;
        self.request = None;
        let heard = match self.kind {
            Kind::Forgejo => Heard::Forgejo(plans::Heard::Failed(match failure {
                Failure::Unseen => plans::Failure::Unseen,
                Failure::Seen => plans::Failure::Seen,
            })),
            Kind::GitHub => Heard::GitHub(github::plans::Heard::Failed(match failure {
                Failure::Unseen => github::plans::Failure::Unseen,
                Failure::Seen => github::plans::Failure::Seen,
            })),
        };
        self.heard.push(heard);
    }
    pub fn timeout(&mut self) {
        self.failed(if self.written { Failure::Seen } else { Failure::Unseen });
    }
    pub fn in_flight(&self) -> bool {
        self.request.is_some()
            || match self.phase {
                Phase::Connecting | Phase::Handshaking => true,
                Phase::Closed | Phase::Sending { .. } | Phase::Reading | Phase::Idle => false,
            }
    }
    pub fn close(&mut self, abort: bool, io: &mut Queue<skein_io::Request>) {
        if self.closing {
            return;
        }
        self.clear_json_body();
        self.phase = Phase::Closed;
        self.deadline = None;
        self.request = None;
        if let Some(socket) = self.socket {
            self.closing = true;
            io.push(if abort {
                skein_io::Request::Abort { entity: socket }
            } else {
                skein_io::Request::Close { entity: socket }
            });
        }
    }
    pub fn heard(&mut self) -> Option<Heard> {
        self.heard.pop()
    }
    pub fn ready(&self) -> bool {
        if self.phase == Phase::Closed {
            return !self.heard.is_empty();
        }
        !self.http_up.is_empty()
            || !self.http_down.is_empty()
            || !self.tls_up.is_empty()
            || !self.tls_down.is_empty()
            || !self.json_up.is_empty()
            || !self.json_down.is_empty()
            || !self.heard.is_empty()
            || self.json_next.is_some()
            || (self.json_body.read.is_some() && !self.json_body.below)
    }
    pub fn keep_after(&mut self, after: u64) {
        self.after = after;
        if let Some(decoder) = self.decoder() {
            match decoder {
                Decoder::Forgejo(decoder) => documents::keep_after(decoder, after),
                Decoder::GitHub(_) => unreachable!("keep_after belongs to Forgejo plans"),
            }
        }
    }
    /// The answer's remaining payload room, set by the holding plan before its raw read.
    pub fn raw_room(&mut self, room: u32) {
        self.raw_cap = room;
    }
    pub fn cut(&mut self, _env: &Env<Limits>) {
        if let Some(decoder) = self.decoder() {
            let request = match decoder {
                Decoder::Forgejo(decoder) => documents::cut(decoder),
                Decoder::GitHub(_) => unreachable!("cut belongs to Forgejo plans"),
            };
            self.json_next = Some(request);
        }
    }
}

fn field(name: &[u8], value: &[u8]) -> skein_http::Header {
    skein_http::Header { name: bytes::copy_of(name), value: bytes::copy_of(value) }
}

impl core::fmt::Debug for Connection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Connection")
            .field("forge", &self.forge)
            .field("phase", &self.phase)
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}
/// The fixed routing queues, allocated once per connection.
pub(crate) fn routing_bytes() -> Option<u64> {
    Queue::<http::Event>::worst_case(8)?
        .checked_add(Queue::<Down>::worst_case(8)?.checked_mul(3)?)?
        .checked_add(Queue::<skein_tls::client::Event>::worst_case(8)?)?
        .checked_add(Queue::<tokenizer::Event>::worst_case(8)?)?
        .checked_add(Queue::<Heard>::worst_case(16)?)
}
impl Connection {
    fn decoder(&mut self) -> Option<&mut Decoder> {
        match &mut self.receive {
            Receiving::Json(decoder) => Some(decoder),
            Receiving::Raw { .. } | Receiving::Tail { .. } | Receiving::Empty => None,
        }
    }
}
fn sending(down: &Down) -> bool {
    match down {
        Down::Send(_) => true,
        Down::Demand { .. } | Down::Finish => false,
    }
}
fn raw_request(request: &request::Request) -> bool {
    match request {
        request::Request::Raw { .. } | request::Request::JobLog { .. } => true,
        request::Request::User
        | request::Request::ApiSettings
        | request::Request::RepositoryById { .. }
        | request::Request::Repository { .. }
        // user/repos is used by the setup host, not the connection.
        | request::Request::UserRepos { .. }
        // repos/search is used by the setup host, not the connection.
        | request::Request::RepoSearch { .. }
        | request::Request::UserSearch { .. }
        | request::Request::Issues { .. }
        | request::Request::Issue { .. }
        | request::Request::Comments { .. }
        | request::Request::Pull { .. }
        | request::Request::PullFor { .. }
        | request::Request::CombinedStatus { .. }
        | request::Request::Reviews { .. }
        | request::Request::ReviewComments { .. }
        | request::Request::Branch { .. }
        | request::Request::Branches { .. }
        | request::Request::Protection { .. }
        | request::Request::Protections { .. }
        | request::Request::Collaborators { .. }
        | request::Request::Permission { .. }
        | request::Request::Pulls { .. }
        | request::Request::PullFiles { .. }
        | request::Request::Compare { .. }
        | request::Request::Tree { .. }
        | request::Request::WorkflowRuns { .. }
        | request::Request::Dispatch { .. }
        | request::Request::Rerun { .. }
        | request::Request::Runs { .. }
        | request::Request::Run { .. }
        | request::Request::Job { .. }
        | request::Request::Jobs { .. }
        | request::Request::CreateIssue { .. }
        | request::Request::OpenPull { .. }
        | request::Request::PostComment { .. }
        | request::Request::PostReview { .. }
        | request::Request::EditIssue { .. }
        | request::Request::RequestReviewers { .. }
        | request::Request::RemoveReviewers { .. }
        | request::Request::Merge { .. }
        | request::Request::Update { .. }
        | request::Request::PostStatus { .. }
        | request::Request::CreateBranch { .. }
        | request::Request::Tokens { .. }
        | request::Request::CreateToken { .. }
        | request::Request::DeleteToken { .. }
        | request::Request::UserOf { .. }
        | request::Request::Org { .. }
        | request::Request::CreateOrg { .. }
        | request::Request::CreateFile { .. }
        | request::Request::CreateRepository { .. }
        | request::Request::Teams { .. }
        | request::Request::CreateTeam { .. }
        | request::Request::EditTeam { .. }
        | request::Request::TeamMembers { .. }
        | request::Request::OrgHooks { .. }
        | request::Request::CreateOrgHook { .. }
        | request::Request::EditOrgHook { .. }
        | request::Request::DeleteOrgHook { .. }
        | request::Request::AddMember { .. }
        // setup requests are the setup host's, not the connection's.
        | request::Request::AddCollaborator { .. }
        | request::Request::RepoHooks { .. }
        | request::Request::CreateRepoHook { .. }
        | request::Request::EditRepoHook { .. }
        | request::Request::DeleteRepoHook { .. }
        | request::Request::CreateUser { .. }
        | request::Request::DeleteBranch { .. } => false,
    }
}

fn github_raw_request(request: &github::request::Request) -> bool {
    use github::request::Request;
    match request {
        Request::Raw { .. } | Request::JobLog { .. } | Request::Followed { .. } => true,
        Request::LogPage { .. }
        | Request::LogDelivery { .. }
        | Request::Installations { .. }
        | Request::Installation { .. }
        | Request::Mint { .. }
        | Request::Reachable { .. }
        | Request::Enqueue { .. }
        | Request::Merge { .. }
        | Request::PullGraphql { .. }
        | Request::Viewer
        | Request::App
        | Request::HookConfig
        | Request::SetHook { .. }
        | Request::Uninstall { .. }
        | Request::Conversion { .. }
        | Request::BotUser { .. }
        | Request::Repository { .. }
        | Request::Locate { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullsFor { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::CombinedStatus { .. }
        | Request::CheckRuns { .. }
        | Request::Job { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::RulesForBranch { .. }
        | Request::Ruleset { .. }
        | Request::ClassicProtection { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::Reopen { .. }
        | Request::Edit { .. }
        | Request::Status { .. }
        | Request::CreateBranch { .. }
        | Request::Reviewers { .. }
        | Request::Update { .. }
        | Request::Review { .. }
        | Request::Post { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::Close { .. }
        | Request::DeleteBranch { .. }
        | Request::User { .. } => false,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
impl Connection {
    pub(crate) fn test_github_handoff(&self) -> bool {
        match &self.request {
            Some(Request::GitHub(exchange)) => {
                exchange.retention.is_some() || exchange.comparison == github::plans::plan::Comparison::MergeBase
            }
            Some(Request::Forgejo(_)) | None => false,
        }
    }
}
