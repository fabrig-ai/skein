//! The component's small limits and configured forge.
use crate::Limits;
/// Check and construction return the same refusal before any io is begun.
pub fn admission(settings: Settings, limits: &Limits, expected: Result<(), crate::Unusable>, label: &[u8]) {
    let shared = shared();
    assert_eq!(crate::limits::check(limits, &shared, &settings), expected, "{label:?}: check");
    let constructed = match Component::new(settings, limits, &shared) {
        Ok(_) => Ok(()),
        Err(reason) => Err(reason),
    };
    assert_eq!(constructed, expected, "{label:?}: construction");
}

use skein_lib::Duration;
pub const WORD: &[u8] = b"kit";
pub const USER_AGENT: &[u8] = b"kit/0.0.0";
pub const OP_BYTES: u32 = 512;
// Twice the operation bound, the owner's word, and marker framing (forge.md 6).
pub const BODY_ROOM: u32 = 2 * OP_BYTES + 3 + 15;
pub const BODY_ROOM_SHORT: u32 = BODY_ROOM - 1;
pub const MAX_WORD_BODY_ROOM: u32 = 2 * OP_BYTES + 16 + 15;
pub const WRITER: u64 = 2;
pub fn limits() -> Limits {
    Limits {
        forges: 2,
        token_bytes: 64,
        key_bytes: 1200,
        connections: 2,
        head_bytes: 4096,
        headers: 32,
        body_bytes_in: 1_048_576,
        depth: 16,
        chunk: 1024,
        string_bytes: 128,
        body_bytes: BODY_ROOM,
        inner: 8,
        target_bytes: 1024,
        repositories: 2,
        users: 8,
        installations: 3,
        log_origins: 2,
        deadline: Duration::from_secs(10),
        idle: Duration::from_secs(5),
        start_backoff: Duration::from_secs(1),
        start_backoff_max: Duration::from_secs(8),
        renew_before: Duration::from_secs(60),
        // Beyond token-renewal stories; puller stories advance this timer themselves.
        log_poll: Duration::from_secs(86_400),
        log_catch_up: Duration::from_secs(3600),
        log_pages: 10,
        delivery_bytes: 65_536,
        hooks: 4,
        hook_head_bytes: 8192,
        hook_body_bytes: 1_048_576,
        hook_deadline: Duration::from_secs(10),
        hook_secret_bytes: 128,
        tls: skein_tls::client::Limits { read: 16384, send: 16384, records: skein_tls::client::MAX_RECORD },
    }
}

use crate::{
    Answered, Component, Event, Settings, Shared,
    settings::{Forge, Kind, Transport},
    start::Start,
};
use alloc::boxed::Box;
use core::net::{Ipv4Addr, SocketAddr};
use skein_forge::api;
use skein_lib::{Env, List, Queue, Secret, Time, Wall};
pub const FORGE: u16 = 1;
pub const REPO: api::Repository = api::Repository { forge: 1, repository: 1 };
pub const TOKEN: &[u8] = b"0123456789abcdef0123456789abcdef01234567";
pub fn shared() -> Shared {
    Shared {
        calls: 2,
        op_bytes: OP_BYTES,
        answer_bytes: 65_536,
        rows: 8,
        lifetime: Duration::from_secs(10),
        name_bytes: 128,
        repositories: 2,
        io: skein_io::Limits {
            sockets: 8,
            names: 0,
            refusals: 2,
            intake: 32_768,
            receive: 4096,
            output: 32_768,
            sends: 4,
            accepts: 1,
            backlog: 1,
            close_timeout: Duration::from_secs(1),
            retry: Duration::from_millis(10),
        },
    }
}
pub fn settings() -> Settings {
    let mut forges = List::with_capacity(2);
    forges
        .push(Forge {
            number: FORGE,
            kind: Kind::Forgejo,
            log_origins: List::with_capacity(0),
            address: skein_io::Addr::Ip(SocketAddr::from((Ipv4Addr::LOCALHOST, 3000))),
            transport: Transport::Plaintext { name: None },
            access: Some(crate::Access {
                credential: skein_forge::credential::Credential::Token(Secret::new(Box::from(TOKEN))),
                user: WRITER,
                webhook_secret: None,
            }),
        })
        .expect("one forge");
    Settings { forges, webhooks: None, word: Box::from(WORD), user_agent: Box::from(USER_AGENT) }
}
#[derive(Debug)]
pub struct Harness {
    pub component: Component,
    pub env: Env<Limits>,
    pub up: Queue<Event>,
    pub io: Queue<skein_io::Request>,
    pub answers: skein_lib::Map<u64, Answered>,
    pub peers: skein_lib::Map<skein_lib::Token, Peer>,
    pub endpoints: List<(skein_io::Addr, u16)>,
    pub opened: u32,
    pub archive: List<(u16, Secret)>,
    pub stopped: Option<(u16, crate::Stopped)>,
    pub git_tokens: Option<List<Event>>,
    pub installation_events: Option<List<Event>>,
    pub facts: Option<List<Event>>,
    pub hints: List<crate::hooks::Hinted>,
    pub component_heap: Option<super::github_files::CallHeap>,
    pub connected_count: u32,
    pub closed: bool,
}
#[derive(Debug)]
pub struct Peer {
    forge: u16,
    socket: skein_lib::Token,
    wire: skein_world::stream::Wire,
    connected: bool,
}
impl Harness {
    pub fn new(settings: Settings) -> Self {
        Self::with(limits(), shared(), settings)
    }
    pub fn with(limits: Limits, shared: Shared, settings: Settings) -> Self {
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
        let mut endpoints = List::with_capacity(settings.forges.len());
        for forge in &settings.forges {
            endpoints.push((forge.address, forge.number)).expect("configured endpoints");
        }
        let mut github = false;
        for forge in &settings.forges {
            if forge.kind == Kind::GitHub {
                github = true;
            }
        }
        let mut h = Self {
            component: Component::new(settings, &env.limits, &shared).expect("fixture limits agree"),
            env,
            up: Queue::with_capacity(32),
            io: Queue::with_capacity(32),
            answers: skein_lib::Map::with_capacity(32),
            peers: skein_lib::Map::with_capacity(32),
            endpoints,
            opened: 0,
            archive: List::with_capacity(32),
            stopped: None,
            git_tokens: if github { Some(List::with_capacity(32)) } else { None },
            installation_events: if github { Some(List::with_capacity(256)) } else { None },
            facts: if github { Some(List::with_capacity(32)) } else { None },
            hints: List::with_capacity(32),
            component_heap: None,
            connected_count: 0,
            closed: false,
        };
        h.component.begin(&h.env, &mut h.up, &mut h.io);
        h.drain();
        h
    }
    pub fn call(&mut self, call: u64, repository: api::Repository, op: api::Op) {
        self.send(crate::Request::Call { call: skein_lib::Token::new(call), repository, op });
    }
    pub fn known(&mut self, repository: api::Repository, row: skein_forge::forgejo::plans::tables::Repository) {
        self.component.forgejo_tables().keep_repository(repository, row);
    }
    pub fn send(&mut self, request: crate::Request) {
        let span = self.start_component();
        self.component.down(&self.env, request, &mut self.up, &mut self.io);
        self.count_component(span);
    }
    pub fn run(&mut self, span: Duration) {
        self.env.now = self.env.now.saturating_add(span);
        let span = self.start_component();
        self.component.fire(&self.env, &mut self.up, &mut self.io);
        self.count_component(span);
        self.drain();
        let span = self.start_component();
        self.component.reclaim();
        self.count_component(span);
        for _ in 0..self.up.len() {
            match self.up.pop().expect("queued event") {
                Event::Connected { .. } => {
                    self.connected_count = self.connected_count.checked_add(1).expect("fixture connects");
                }
                Event::Answered(answer) => {
                    assert!(self.answered(answer.call.raw()).is_none(), "one terminal per call");
                    drop(self.answers.insert(answer.call.raw(), answer).expect("fixture answers fit"));
                }
                Event::Stopped { forge, why } => {
                    assert!(self.stopped.is_none(), "start stopped once");
                    self.stopped = Some((forge, why));
                }
                event @ Event::Installation { .. } => {
                    self.installation_events
                        .as_mut()
                        .expect("GitHub fixture")
                        .push(event)
                        .expect("fixture changes fit");
                }
                event @ Event::Fact { .. } => {
                    self.facts.as_mut().expect("GitHub fixture").push(event).expect("fixture facts fit");
                }
                event @ Event::GitToken { .. } => {
                    self.git_tokens.as_mut().expect("GitHub fixture").push(event).expect("fixture tokens fit");
                }
                Event::Hint(hint) => self.hints.push(hint).expect("fixture hints fit"),
                Event::Listening { .. } | Event::ListenerLost => panic!("webhooks absent"),
                Event::Closed => {
                    assert!(!self.closed, "closed once");
                    self.closed = true;
                }
            }
        }
    }
    pub fn answered(&self, call: u64) -> Option<&Answered> {
        self.answers.get(&call)
    }
    pub fn start(&self, forge: u16) -> Start {
        self.component.start(forge).expect("fixture forge configured")
    }
    fn newest(&self, forge: u16) -> skein_lib::Token {
        let mut found = None;
        for (owner, peer) in &self.peers {
            if peer.forge == forge {
                found = Some(*owner);
            }
        }
        found.expect("forge has a connection")
    }
    fn owners(&self) -> List<skein_lib::Token> {
        let mut owners = List::with_capacity(32);
        for (owner, _) in &self.peers {
            owners.push(*owner).expect("fixture peers fit");
        }
        owners
    }
    #[expect(clippy::manual_map, reason = "the workspace uses exhaustive matches instead of closures")]
    fn start_component(&self) -> Option<skein_heap::Span> {
        match self.component_heap {
            Some(_) => Some(skein_heap::Span::start()),
            None => None,
        }
    }
    fn count_component(&mut self, span: Option<skein_heap::Span>) {
        if let Some(span) = span {
            self.component_heap.as_mut().expect("component metering").count(span.end());
        }
    }
    fn event(&mut self, event: skein_io::Event) {
        if let Some(heap) = &mut self.component_heap
            && let skein_io::Event::Stream { up: skein_lib::stream::Up::Bytes(bytes), .. } = &event
        {
            // Incoming bytes become the component's at this boundary.
            heap.held = heap.held.checked_add(i64::try_from(bytes.len()).expect("input bytes")).expect("held input");
        }
        let span = self.start_component();
        self.component.up(&self.env, event, &mut self.up, &mut self.io);
        self.count_component(span);
    }
    pub fn connected(&mut self, forge: u16) {
        let owner = self.newest(forge);
        self.peers.get_mut(&owner).expect("registered connect").connected = true;
        self.event(skein_io::Event::Connected { owner });
        self.drain();
    }
    pub fn refuse(&mut self, forge: u16) {
        let owner = self.newest(forge);
        self.event(skein_io::Event::Failed { owner, error: skein_io::Error::Refused });
        self.event(skein_io::Event::Closed { owner });
        drop(self.peers.remove(&owner));
        self.drain();
    }
    pub fn answer(&mut self, forge: u16, bytes: &[u8]) {
        let owner = self.newest(forge);
        self.peers.get_mut(&owner).expect("registered connect").wire.write(bytes);
        self.drain();
    }
    pub fn reset(&mut self, forge: u16) {
        let owner = self.newest(forge);
        self.event(skein_io::Event::Stream {
            owner,
            up: skein_lib::stream::Up::Failed(skein_lib::stream::Fault::Reset),
        });
        self.drain();
    }
    pub fn request_lines(&self, forge: u16) -> List<Box<[u8]>> {
        let mut lines = List::with_capacity(16);
        for (_, peer) in &self.peers {
            if peer.forge != forge {
                continue;
            }
            let mut start = 0;
            for (at, byte) in peer.wire.received.iter().enumerate() {
                if *byte == b'\n' {
                    let line = peer.wire.received.get(start..at).expect("line positions");
                    if line.starts_with(b"GET ") {
                        lines.push(Box::from(line.strip_suffix(b"\r").unwrap_or(line))).expect("fixture lines fit");
                    }
                    start = at.saturating_add(1);
                }
            }
        }
        lines
    }
    pub fn request_bytes(&self, forge: u16) -> Box<[u8]> {
        let mut length = 0_usize;
        for (_, peer) in &self.peers {
            if peer.forge == forge {
                length = length.checked_add(peer.wire.received.len()).expect("bounded requests");
            }
        }
        let mut out = skein_lib::Writer::new(length);
        for (_, peer) in &self.peers {
            if peer.forge == forge {
                out.put(&peer.wire.received).expect("requests measured");
            }
        }
        out.finish()
    }
    pub fn all_request_bytes(&self, forge: u16) -> Box<[u8]> {
        let current = self.request_bytes(forge);
        let mut length = current.len();
        for (number, bytes) in &self.archive {
            if *number == forge {
                length = length.checked_add(bytes.len()).expect("fixture history");
            }
        }
        let mut out = skein_lib::Writer::new(length);
        for (number, bytes) in &self.archive {
            if *number == forge {
                out.put(bytes.expose()).expect("measured history");
            }
        }
        out.put(&current).expect("measured current");
        out.finish()
    }
    pub fn connections_opened(&self) -> u32 {
        self.opened
    }
    pub fn stopped(&self) -> Option<(u16, crate::Stopped)> {
        self.stopped
    }
    pub fn ready() -> Self {
        let mut h = Self::new(settings());
        h.connected(FORGE);
        h.answer(FORGE, &user_ok());
        h.answer(FORGE, &settings_ok());
        h
    }
    fn drain(&mut self) {
        let mut quiet = 0_u32;
        for _ in 0..1024_u32 {
            for _ in 0..self.io.len() {
                match self.io.pop().expect("queued request") {
                    skein_io::Request::Connect { owner, addr } => {
                        let mut forge = None;
                        for (address, number) in &self.endpoints {
                            if *address == addr {
                                forge = Some(*number);
                            }
                        }
                        let forge = forge.expect("connect names a configured endpoint");
                        self.opened = self.opened.saturating_add(1);
                        let socket = skein_lib::Token::new(owner.raw().saturating_add(100));
                        drop(
                            self.peers
                                .insert(
                                    owner,
                                    Peer {
                                        forge,
                                        socket,
                                        wire: skein_world::stream::Wire::new(owner.raw()),
                                        connected: false,
                                    },
                                )
                                .expect("fixture peers fit"),
                        );
                        self.event(skein_io::Event::Connecting { owner, socket });
                    }
                    skein_io::Request::Stream { stream, down } => {
                        for owner in self.owners().into_boxed() {
                            let peer = self.peers.get_mut(&owner).expect("fixture peer");
                            if peer.socket == stream {
                                peer.wire.take(down);
                                break;
                            }
                        }
                    }
                    skein_io::Request::Close { entity } | skein_io::Request::Abort { entity } => {
                        let mut owner = None;
                        for (token, peer) in &self.peers {
                            if peer.socket == entity {
                                owner = Some(*token);
                                break;
                            }
                        }
                        if let Some(owner) = owner {
                            self.event(skein_io::Event::Closed { owner });
                            let peer = self.peers.remove(&owner).expect("closed peer");
                            self.archive
                                .push((peer.forge, Secret::new(Box::from(peer.wire.received.as_slice()))))
                                .expect("fixture history");
                        }
                    }
                    skein_io::Request::Listen { .. }
                    | skein_io::Request::Bind { .. }
                    | skein_io::Request::Reject { .. }
                    | skein_io::Request::Output { .. }
                    | skein_io::Request::Spawn { .. }
                    | skein_io::Request::Signal { .. }
                    | skein_io::Request::Usage { .. }
                    | skein_io::Request::Window { .. } => unreachable!("forge only uses sockets"),
                }
            }
            let mut events = List::with_capacity(32);
            let mut moved = false;
            for owner in self.owners().into_boxed() {
                let peer = self.peers.get_mut(&owner).expect("fixture peer");
                let trace = peer.wire.trace.len();
                if peer.connected
                    && let Some(up) = peer.wire.answer()
                {
                    events.push(skein_io::Event::Stream { owner, up }).expect("fixture events fit");
                }
                moved |= peer.wire.trace.len() != trace;
                // The trace signals progress; retaining it would charge the
                // scripted peer's transcript to component heap measurements.
                peer.wire.trace.clear();
            }
            moved |= !events.is_empty();
            for event in events.into_boxed() {
                self.event(event);
            }
            if !moved && self.io.is_empty() {
                quiet = quiet.saturating_add(1);
                if quiet == 32 {
                    break;
                }
            } else {
                quiet = 0;
            }
        }
    }
}
pub fn ok(body: &[u8]) -> Box<[u8]> {
    response(200, body)
}
pub fn response(code: u16, body: &[u8]) -> Box<[u8]> {
    response_with_fields(code, body, b"")
}
pub fn last_page(body: &[u8]) -> Box<[u8]> {
    response_with_fields(200, body, b"X-HasMore: false\r\n")
}
pub fn response_with_fields(code: u16, body: &[u8], fields: &[u8]) -> Box<[u8]> {
    let decimal = skein_lib::Decimal::of(u64::try_from(body.len()).expect("body length"));
    let mut out = skein_lib::Writer::new(
        body.len()
            .saturating_add(fields.len())
            .saturating_add(b"HTTP/1.1 200 OK\r\nContent-Length: ".len())
            .saturating_add(decimal.as_bytes().len())
            .saturating_add(b"\r\nDate: Wed, 01 Jan 2020 00:00:00 GMT\r\n\r\n".len()),
    );
    out.put(b"HTTP/1.1 ").expect("head fits");
    out.put(skein_lib::Decimal::of(u64::from(code)).as_bytes()).expect("code fits");
    out.put(b" OK\r\n").expect("head fits");
    out.put(fields).expect("fields fit");
    out.put(b"Content-Length: ").expect("head fits");
    out.put(decimal.as_bytes()).expect("length fits");
    out.put(b"\r\nDate: Wed, 01 Jan 2020 00:00:00 GMT\r\n\r\n").expect("date fits");
    out.put(body).expect("body fits");
    out.finish()
}
pub fn ok_dated(body: &[u8]) -> Box<[u8]> {
    ok(body)
}
pub fn status(code: u16) -> Box<[u8]> {
    let decimal = skein_lib::Decimal::of(u64::from(code));
    let mut out = skein_lib::Writer::new(
        b"HTTP/1.1 "
            .len()
            .saturating_add(decimal.as_bytes().len())
            .saturating_add(b" Refused\r\nContent-Length: 0\r\n\r\n".len()),
    );
    out.put(b"HTTP/1.1 ").expect("head fits");
    out.put(skein_lib::Decimal::of(u64::from(code)).as_bytes()).expect("code fits");
    out.put(b" Refused\r\nContent-Length: 0\r\n\r\n").expect("head fits");
    out.finish()
}
pub fn user_ok() -> Box<[u8]> {
    ok(b"{\"id\":2,\"login\":\"writer\"}")
}
pub fn settings_ok() -> Box<[u8]> {
    ok(b"{\"max_response_items\":2,\"default_paging_num\":30}")
}
pub fn specimen() -> skein_forge::forgejo::plans::tables::Repository {
    skein_forge::forgejo::plans::tables::Repository {
        owner: Box::from(&b"fixture"[..]),
        name: Box::from(&b"specimen"[..]),
        object_format: skein_forge::forgejo::id::ObjectFormat::Sha1,
        merge_style: skein_forge::forgejo::document::MergeStyle::Squash,
        html_url: Box::from(&b"http://127.0.0.1:3000/fixture/specimen"[..]),
    }
}

/// One connection's scripted stream, shared by its step tests.
#[derive(Debug)]
pub struct ConnectionHarness {
    pub connection: crate::connection::Connection,
    pub env: Env<Limits>,
    pub forge: Forge,
    pub io: Queue<skein_io::Request>,
    pub sent: Box<[u8]>,
    pub body_demands: u32,
    pub local_pumps: u32,
}
impl ConnectionHarness {
    pub fn new() -> Self {
        let forge = settings().forges.into_boxed().into_iter().next().expect("one forge");
        Self::with_forge(forge, shared())
    }
    pub fn github() -> Self {
        Self::github_with(shared())
    }
    pub fn github_with(shared: Shared) -> Self {
        let mut forge = settings().forges.into_boxed().into_iter().next().expect("one forge");
        forge.kind = Kind::GitHub;
        forge.access = None;
        Self::with_forge(forge, shared)
    }
    pub fn with_chunk(chunk: u32) -> Self {
        let forge = settings().forges.into_boxed().into_iter().next().expect("one forge");
        let mut limits = limits();
        limits.chunk = chunk;
        Self::with_forge_limits(forge, shared(), limits)
    }
    fn with_forge(forge: Forge, shared: Shared) -> Self {
        Self::with_forge_limits(forge, shared, limits())
    }
    fn with_forge_limits(forge: Forge, shared: Shared, limits: Limits) -> Self {
        let mut connection =
            crate::connection::Connection::new(&forge, &limits, shared, Time::from_nanos(10_000_000_000));
        connection.socket = Some(skein_lib::Token::new(100));
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
        connection.connect(&env);
        Self {
            connection,
            env,
            forge,
            io: Queue::with_capacity(32),
            sent: Box::from([]),
            body_demands: 0,
            local_pumps: 0,
        }
    }
    pub fn pump(&mut self) {
        self.connection.pump(&self.env, &mut self.io, 16);
    }
    pub fn answer(&mut self, response: &[u8]) -> List<skein_forge::forgejo::plans::Heard> {
        let mut heard = List::with_capacity(16);
        for event in self.answer_dialect(response).into_boxed() {
            match event {
                crate::connection::Heard::Forgejo(event) => heard.push(event).expect("fixture events fit"),
                crate::connection::Heard::GitHub(_) => unreachable!("Forgejo harness answer"),
            }
        }
        heard
    }
    /// The existing connection fixture with a buffered or fragmented peer body.
    pub fn buffered_answer(&mut self, response: &[u8], fragment: usize, eof: bool) -> List<crate::connection::Heard> {
        use skein_lib::stream::{Down, Read, Up};
        let mut intake = skein_lib::Intake::with_capacity(shared().io.intake);
        let mut at = 0_usize;
        let mut pending = None;
        let mut ended = false;
        let mut heard = List::with_capacity(16);
        for _ in 0_u32..1024 {
            if self.connection.ready() && self.io.is_empty() {
                self.local_pumps = self.local_pumps.saturating_add(1);
            }
            self.pump();
            for _ in 0..self.io.len() {
                match self.io.pop().expect("queued request") {
                    skein_io::Request::Stream { down, .. } => match down {
                        Down::Demand { read: Read::Nothing, room: 0 } => {
                            assert!(pending.take().is_some(), "withdraw an outstanding read");
                        }
                        Down::Demand { read, room } => {
                            assert!(pending.is_none(), "one read below at a time");
                            let body = match read {
                                Read::Nothing => false,
                                Read::Scan { until, .. } => until != skein_lib::stream::Delimiter::LF,
                                Read::Fill(_) | Read::Line { .. } | Read::Arrived { .. } => true,
                            };
                            self.body_demands = self.body_demands.saturating_add(u32::from(body));
                            pending = Some((read, room));
                        }
                        Down::Send(bytes) => self.sent = bytes,
                        Down::Finish => {}
                    },
                    skein_io::Request::Listen { .. }
                    | skein_io::Request::Connect { .. }
                    | skein_io::Request::Bind { .. }
                    | skein_io::Request::Reject { .. }
                    | skein_io::Request::Output { .. }
                    | skein_io::Request::Spawn { .. }
                    | skein_io::Request::Signal { .. }
                    | skein_io::Request::Usage { .. }
                    | skein_io::Request::Window { .. }
                    | skein_io::Request::Close { .. }
                    | skein_io::Request::Abort { .. } => unreachable!("one connection emits stream requests"),
                }
            }
            if at < response.len() {
                let end = at.saturating_add(fragment).min(response.len());
                assert!(end > at, "positive fixture fragments");
                intake.append(response.get(at..end).expect("fixture fragment")).expect("response capacity");
                at = end;
            }
            if let Some((read, room)) = pending {
                if let Some(bytes) = intake.meet(read) {
                    pending = None;
                    self.connection.stream(&self.env, Up::Bytes(bytes));
                } else if room > 0 {
                    pending = None;
                    self.connection.stream(&self.env, Up::Room);
                }
            }
            // A terminal may accompany the final byte delivery, before the
            // connection pumps either of HTTP's resulting events.
            if eof && !ended && at == response.len() && intake.is_empty() {
                ended = true;
                self.connection.stream(&self.env, Up::End);
            }
            for _ in 0_u32..16 {
                let Some(event) = self.connection.heard() else { break };
                heard.push(event).expect("fixture events fit");
            }
            if !self.connection.ready() && pending.is_none() && at == response.len() {
                break;
            }
        }
        heard
    }
    pub fn github_answer(&mut self, response: &[u8]) -> List<skein_forge::github::plans::Heard> {
        let mut heard = List::with_capacity(16);
        for event in self.answer_dialect(response).into_boxed() {
            match event {
                crate::connection::Heard::GitHub(event) => heard.push(event).expect("fixture events fit"),
                crate::connection::Heard::Forgejo(_) => unreachable!("GitHub harness answer"),
            }
        }
        heard
    }
    fn answer_dialect(&mut self, response: &[u8]) -> List<crate::connection::Heard> {
        let mut wire = skein_world::stream::Wire::new(1);
        let mut written = false;
        let mut heard = List::with_capacity(16);
        for _ in 0_u32..1024 {
            self.pump();
            for _ in 0..self.io.len() {
                let request = self.io.pop().expect("queued io request");
                match request {
                    skein_io::Request::Stream { down, .. } => wire.take(down),
                    skein_io::Request::Listen { .. }
                    | skein_io::Request::Connect { .. }
                    | skein_io::Request::Bind { .. }
                    | skein_io::Request::Reject { .. }
                    | skein_io::Request::Output { .. }
                    | skein_io::Request::Spawn { .. }
                    | skein_io::Request::Signal { .. }
                    | skein_io::Request::Usage { .. }
                    | skein_io::Request::Window { .. }
                    | skein_io::Request::Close { .. }
                    | skein_io::Request::Abort { .. } => unreachable!("one connection emits stream requests"),
                }
            }
            if !written && self.connection.phase == crate::connection::Phase::Reading {
                wire.write(response);
                written = true;
            }
            if let Some(event) = wire.answer() {
                self.connection.stream(&self.env, event);
            }
            if let Some(event) = self.connection.heard() {
                heard.push(event).expect("fixture events fit");
            }
        }
        self.sent = Box::from(wire.received.as_slice());
        heard
    }
}
/// Forgejo tests supply the component's complete token header.
pub fn forgejo_exchange(
    request: skein_forge::forgejo::request::Request,
    range: Option<skein_forge::forgejo::plans::plan::Range>,
) -> crate::connection::Exchange {
    let credential = skein_forge::credential::Credential::Token(Secret::new(Box::from(TOKEN)));
    let authorization = skein_forge::forgejo::credential::authorization(&credential).expect("fixture token");
    crate::connection::Exchange::Forgejo { request, range, authorization }
}
pub fn known() -> Harness {
    let mut h = Harness::ready();
    h.known(REPO, specimen());
    h
}

pub const HOOK_SECRET: &[u8] = b"fixture-hook-secret";
pub fn hook_settings() -> Settings {
    let mut settings = settings();
    settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").webhook_secret =
        Some(Secret::new(Box::from(HOOK_SECRET)));
    settings.webhooks = Some(crate::hooks::Webhooks { listener: skein_io::Addr::Ip(([127, 0, 0, 1], 7200).into()) });
    settings
}
pub const HOOK_REPOSITORY: u64 = 2;
pub const HOOK_NUMBER: u64 = 7;
pub const HOOK_HEAD: [u8; 32] = [0x77; 32];
pub fn hook_body() -> skein_forge::forgejo::document::Body {
    skein_forge::forgejo::document::Body::Kept(Box::from(b"<!-- kit:key 0102 -->\nbody".as_slice()))
}
pub fn hook_id() -> skein_forge::forgejo::id::ObjectId {
    skein_forge::forgejo::id::ObjectId { format: skein_forge::forgejo::id::ObjectFormat::Sha256, bytes: HOOK_HEAD }
}
pub fn hook_ref(reference: &[u8]) -> skein_forge::forgejo::delivery::Delivery {
    skein_forge::forgejo::delivery::Delivery::Ref {
        repository: HOOK_REPOSITORY,
        reference: Box::from(reference),
        created: false,
        ref_type: skein_forge::forgejo::delivery::RefType::Branch,
    }
}

/// Trust shared by the configured TLS names; the HTTP harness captures
/// requests before their TLS record transport (the TLS world owns encryption).
pub fn trust() -> skein_tls::Config {
    let root = std::fs::read("../../tests/tls/fixtures/root.der").expect("shared TLS root");
    skein_tls::Config::from_der(&[root.into_boxed_slice()], &[]).expect("valid root").0
}

/// Exercise request construction and HTTP serialization on a scripted carrier.
/// The supplied forge still supplies its transport name to `Connection::start`.
pub fn request_bytes(settings: Settings) -> Box<[u8]> {
    let mut h = ConnectionHarness::new();
    h.forge = settings.forges.into_boxed().into_iter().next().expect("one configured forge");
    h.connection
        .start(
            &h.env,
            &h.forge,
            &settings.user_agent,
            forgejo_exchange(skein_forge::forgejo::request::Request::User, None),
        )
        .expect("valid request");
    h.answer(&user_ok());
    h.sent
}

/// The literal value of the sole named header on the HTTP wire.
pub fn header<'a>(sent: &'a [u8], name: &[u8]) -> &'a [u8] {
    let mut found = None;
    let mut rest = sent;
    while let Some(end) = skein_lib::bytes::find(rest, b"\r\n") {
        let line = rest.get(..end).expect("line end in request");
        if let Some(value) = line.strip_prefix(name) {
            assert!(found.is_none(), "header occurs once");
            found = Some(value);
        }
        rest = rest.get(end.checked_add(2).expect("line end fits")..).expect("next line");
    }
    found.expect("header sent")
}

// The request caps are the fixture's TLS send and io output bounds.
pub const TLS_REQUEST_ROOM: usize = 16_384;
pub const PLAIN_REQUEST_ROOM: usize = 32_768;
// forge-connection.md 7.4 reserves 192 bytes for head framing.
pub const HEAD_FRAMING: usize = 192;
pub const TOKEN_BOUND: usize = 64;
pub const SECRET_BOUND: usize = 128;
pub const PULL_PAYLOAD: &[u8] = b"{\"repository\":{\"id\":2},\"number\":7}";

/// Lowercase HMAC text for the configured hook secret.
pub fn hook_signature(body: &[u8]) -> [u8; 64] {
    let mut mac = skein_lib::sha256::Hmac::new(HOOK_SECRET);
    mac.update(body);
    let mut signature = [0; 64];
    for (digits, byte) in signature.chunks_exact_mut(2).zip(mac.finish()) {
        let [high, low] = digits else { unreachable!("hex pair") };
        *high = *b"0123456789abcdef".get(usize::from(byte / 16)).expect("hex digit");
        *low = *b"0123456789abcdef".get(usize::from(byte % 16)).expect("hex digit");
    }
    signature
}
pub fn signed_head(body: &[u8], event: Option<&[u8]>) -> Box<[u8]> {
    hook_head(body, event, &hook_signature(body))
}
pub fn hook_head(body: &[u8], event: Option<&[u8]>, signature: &[u8]) -> Box<[u8]> {
    use skein_forge::forgejo::delivery::{EVENT_HEADER, SIGNATURE_HEADER};
    let target = skein_forge::forgejo::delivery::hook_target(FORGE);
    let length = skein_lib::Decimal::of(u64::try_from(body.len()).expect("body length"));
    let mut pieces = List::with_capacity(16);
    for piece in
        [b"POST ".as_slice(), &target, b" HTTP/1.1\r\nHost: localhost\r\nContent-Length: ", length.as_bytes(), b"\r\n"]
    {
        pieces.push(piece).expect("bounded head pieces");
    }
    if let Some(event) = event {
        for piece in [EVENT_HEADER, b": ", event, b"\r\n"] {
            pieces.push(piece).expect("bounded event pieces");
        }
    }
    for piece in [SIGNATURE_HEADER, b": ", signature, b"\r\n\r\n"] {
        pieces.push(piece).expect("bounded signature pieces");
    }
    let mut bytes = 0_usize;
    for piece in &pieces {
        bytes = bytes.checked_add(piece.len()).expect("head length");
    }
    let mut out = skein_lib::Writer::new(bytes);
    for piece in pieces.into_boxed() {
        out.put(piece).expect("measured head");
    }
    out.finish()
}

#[derive(Debug)]
pub struct HookHarness {
    connection: crate::hooks::connection::Hooked,
    settings: Settings,
    env: Env<Limits>,
    wire: skein_world::stream::Wire,
    closed: bool,
    hints: u32,
    body_demands: u32,
}
pub fn hook() -> HookHarness {
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits() };
    HookHarness {
        connection: crate::hooks::connection::Hooked::new(skein_lib::Token::new(100), &env),
        settings: hook_settings(),
        env,
        wire: skein_world::stream::Wire::new(1),
        closed: false,
        hints: 0,
        body_demands: 0,
    }
}
impl HookHarness {
    pub fn write(&mut self, bytes: &[u8]) {
        self.wire.write(bytes);
        self.pump();
    }
    pub fn body_demands(&self) -> u32 {
        self.body_demands
    }
    pub fn hints(&self) -> u32 {
        self.hints
    }
    pub fn closed(&self) -> bool {
        self.closed
    }
    pub fn status(&self) -> Option<u16> {
        if self.wire.received.len() < b"HTTP/1.1 204".len() {
            return None;
        }
        let text = self.wire.received.get(9..12).expect("HTTP status");
        let mut value = 0_u16;
        for digit in text {
            value = value
                .checked_mul(10)
                .expect("status fits")
                .checked_add(u16::from(digit.checked_sub(b'0').expect("decimal digit")))
                .expect("status fits");
        }
        Some(value)
    }
    fn pump(&mut self) {
        use skein_lib::stream::{Delimiter, Down, Read};
        let mut up = Queue::with_capacity(16);
        let mut io = Queue::with_capacity(16);
        for _ in 0_u32..1024 {
            self.connection.pump(skein_lib::Token::new(1), &self.env, &self.settings, &mut up, &mut io, (16, 16));
            for _ in 0..io.len() {
                match io.pop().expect("queued request") {
                    skein_io::Request::Stream { down, .. } => {
                        if let Down::Demand { read, .. } = &down {
                            let body = match read {
                                Read::Nothing => false,
                                Read::Scan { until, .. } => *until != Delimiter::LF,
                                Read::Fill(_) | Read::Line { .. } | Read::Arrived { .. } => true,
                            };
                            self.body_demands = self.body_demands.saturating_add(u32::from(body));
                        }
                        self.wire.take(down);
                    }
                    skein_io::Request::Close { .. } => self.closed = true,
                    skein_io::Request::Listen { .. }
                    | skein_io::Request::Connect { .. }
                    | skein_io::Request::Bind { .. }
                    | skein_io::Request::Reject { .. }
                    | skein_io::Request::Output { .. }
                    | skein_io::Request::Spawn { .. }
                    | skein_io::Request::Signal { .. }
                    | skein_io::Request::Usage { .. }
                    | skein_io::Request::Window { .. }
                    | skein_io::Request::Abort { .. } => unreachable!("hook emits stream/close"),
                }
            }
            for _ in 0..up.len() {
                match up.pop().expect("queued event") {
                    Event::Hint(_) => self.hints = self.hints.saturating_add(1),
                    Event::Installation { .. }
                    | Event::Fact { .. }
                    | Event::GitToken { .. }
                    | Event::Connected { .. }
                    | Event::Answered(_)
                    | Event::Stopped { .. }
                    | Event::Listening { .. }
                    | Event::ListenerLost
                    | Event::Closed => {
                        unreachable!("hook emits hints")
                    }
                }
            }
            if self.closed {
                break;
            }
            if let Some(event) = self.wire.answer() {
                self.connection.stream(&self.env, event);
            }
        }
    }
}

pub fn filled(length: usize, byte: u8) -> Box<[u8]> {
    let mut out = skein_lib::Writer::new(length);
    for _ in 0..length {
        out.put(&[byte]).expect("measured fill");
    }
    out.finish()
}

pub fn head_settings(name: &str, tls: bool, agent_bytes: usize) -> Settings {
    let mut settings = settings();
    settings.forges.get_mut(0).expect("forge").transport = if tls {
        Transport::Tls { server_name: skein_tls::Name::new(name).expect("TLS name"), trust: trust() }
    } else {
        Transport::Plaintext { name: Some(Box::from(name.as_bytes())) }
    };
    settings.user_agent = filled(agent_bytes, b'a');
    settings
}

// forge.md 3.7 fixed widths, with the fixture's string/body/inner bounds.
pub const ROW_ITEMS: u32 = 2971;
pub const ROW_REVIEWS: u32 = 1691;
pub const ROW_STATUSES: u32 = 544;
pub const ROW_REMARKS: u32 = 1234;
pub const ROW_BRANCHES: u32 = 144;
pub const ROW_COLLABORATORS: u32 = 16;
pub const ROW_PULLFILES: u32 = 2276;
pub const ROW_COMPARE: u32 = 32;
pub const MAX_ROW: u32 = ROW_ITEMS;
pub const OTHER_FORGE: u16 = 2;
pub const OTHER_REPO: api::Repository = api::Repository { forge: OTHER_FORGE, repository: 1 };
pub const BRANCH: &[u8] = b"main";
pub const BRANCH_HEAD: [u8; 32] = [0; 32];
pub const REPOSITORY_BODY: &[u8] = br#"{"id":1,"owner":{"id":2,"login":"fixture"},"name":"specimen","object_format_name":"sha1","default_branch":"main","default_merge_style":"merge","allow_merge_commits":true,"allow_squash_merge":false,"allow_rebase":false,"html_url":"http://127.0.0.1:3000/fixture/specimen"}"#;
pub const BRANCH_BODY: &[u8] = br#"{"name":"main","commit":{"id":"0000000000000000000000000000000000000000"},"effective_branch_protection_name":""}"#;
pub fn two_forges() -> Settings {
    let mut settings = settings();
    let mut other = self::settings().forges.into_boxed().into_iter().next().expect("forge");
    other.number = OTHER_FORGE;
    other.address = skein_io::Addr::Ip(([127, 0, 0, 1], 3001).into());
    settings.forges.push(other).expect("two forges fit");
    settings
}
pub fn ready_two(limits: Limits, shared: Shared) -> Harness {
    let mut h = Harness::with(limits, shared, two_forges());
    for forge in [FORGE, OTHER_FORGE] {
        h.connected(forge);
        h.answer(forge, &user_ok());
        h.answer(forge, &settings_ok());
        h.run(limits.idle);
    }
    h
}
pub fn branch_answers(h: &mut Harness, forge: u16) {
    h.answer(forge, &ok(REPOSITORY_BODY));
    h.answer(forge, &ok(BRANCH_BODY));
    h.run(Duration::ZERO);
}

impl Harness {
    pub fn connections_for(&self, forge: u16) -> u32 {
        let mut count = 0_u32;
        for (_, peer) in &self.peers {
            if peer.forge == forge {
                count = count.checked_add(1).expect("bounded peers");
            }
        }
        count
    }
}

// The page premise contains two maximal rows, and also tests one row and no room.
pub const PAGE_ROWS: u32 = 2; // The premise budgets exactly two maximal rows.
pub const ONE_PAGE_ROW: u32 = 1; // One maximal row is the admission boundary.
pub const NO_PAGE_ROWS: u32 = 0; // No row fits below the first row byte bound.
// Eviction shares one connection and one call slot between two sequential requests.
pub const EVICTION_CONNECTIONS: u32 = 1; // A single connection forces reassignment.
pub const EVICTION_CALLS: u32 = EVICTION_CONNECTIONS;
pub const EVICTION_NO_CONNECTIONS: u32 = 0; // The evicted forge has no connection.
pub const EVICTION_ANSWERS: u32 = 2; // Each of the two sequential requests answers once.

pub fn app_access(key: &[u8], id: u64, user: u64) -> crate::Access {
    crate::Access {
        credential: skein_forge::credential::Credential::App { id, key: Secret::new(Box::from(key)) },
        user,
        webhook_secret: None,
    }
}

pub fn github_settings() -> Settings {
    let mut settings = settings();
    let forge = settings.forges.get_mut(0).expect("forge");
    forge.kind = Kind::GitHub;
    forge.access = Some(app_access(skein_fake_forge_github::keys::KEYS.first().expect("test key"), 7, 9));
    settings
}

/// Ordinary calls start from a checked identity without repeating startup's RSA work.
pub fn github_calls() -> Harness {
    let mut settings = github_settings();
    settings.forges.get_mut(0).expect("forge").access = None;
    let mut h = Harness::new(settings);
    h.component.test_calls_ready();
    github_account(&mut h, 1, 22, b"acme", b"artificial-full-one");
    h
}
pub fn github_account(h: &mut Harness, installation: u64, account: u64, login: &[u8], token: &[u8]) {
    use crate::installations::{Clock, Row, Tokens};
    let gh = h.component.test_github_mut();
    let _accepted = gh.accepted.insert(installation).expect("fixture accepted");
    drop(
        gh.rows
            .insert(
                installation,
                Row {
                    state: Tokens::Current,
                    revision: 0,
                    token_revision: 1,
                    refused_revision: None,
                    full: Clock { expiry: Time::from_nanos(3_600_000_000_000), ..Clock::empty() },
                    contents: Clock::empty(),
                    account: crate::Account { id: account, login: Box::from(login) },
                    suspended: false,
                    accepted: true,
                    api: Some(Secret::new(Box::from(token))),
                    permissions: skein_forge::github::document::Levels::NONE,
                    api_expires: Wall::from_nanos(3_600_000_000_000),
                    git_expires: Wall::from_nanos(3_600_000_000_000),
                },
            )
            .expect("fixture installation"),
    );
    let account = gh.rows.get(&installation).expect("fixture row").account.clone();
    drop(
        gh.known
            .insert(installation, crate::installations::Accepted { known: Some(account), checked: true })
            .expect("fixture accepted bookkeeping"),
    );
}
pub fn github_repository(h: &mut Harness, repository: api::Repository, account: u64) {
    h.component.github_tables().keep_repository(
        repository,
        skein_forge::github::plans::tables::Repository {
            owner: Box::from(b"acme".as_slice()),
            name: Box::from(b"widgets".as_slice()),
            owner_id: account,
            organization: true,
            merge: Some(skein_forge::github::plans::tables::Method::Merge),
            html_url: Box::from(b"https://github.com/acme/widgets".as_slice()),
        },
    );
}
pub const GITHUB_REPOSITORY: &[u8] = br#"{"id":1,"owner":{"id":22,"login":"acme","type":"Organization"},"name":"widgets","default_branch":"main","html_url":"https://github.com/acme/widgets","allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":false}"#;
pub const GITHUB_BRANCH: &[u8] = br#"{"name":"main","commit":{"sha":"0000000000000000000000000000000000000000"}}"#;

// GitHub dialect fixtures: issue/comment, pull, combined status, check runs and GraphQL.
pub const GITHUB_ISSUE: &[u8] = br#"{"number":7,"state":"open","user":{"id":9},"updated_at":"2026-10-20T08:00:00Z","title":"widgets","labels":[],"body":"description"}"#;
pub const GITHUB_COMMENTS: &[u8] = br#"[{"id":8,"user":{"id":9},"created_at":"2026-10-20T08:00:00Z","updated_at":"2026-10-20T08:00:00Z","body":"hello"}]"#;
pub const GITHUB_PULL: &[u8] = br#"{"number":7,"state":"open","merged":false,"merge_commit_sha":null,"mergeable":true,"head":{"ref":"feature","sha":"0000000000000000000000000000000000000000"},"base":{"ref":"main","sha":"1111111111111111111111111111111111111111"},"requested_reviewers":[]}"#;
pub const GITHUB_COMBINED: &[u8] = br#"{"state":"pending","total_count":0,"statuses":[]}"#;
pub const GITHUB_RUNS: &[u8] = br#"{"check_runs":[]}"#;
pub const GITHUB_GRAPHQL: &[u8] = br#"{"data":{"repository":{"pullRequest":{"id":"opaque-node","mergeCommit":null,"mergeQueueEntry":null,"timelineItems":{"nodes":[]}}}}}"#;
