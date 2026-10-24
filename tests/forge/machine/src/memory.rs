//! Reachable full holders, with the peer and owner outside the component's meter.
use crate::{fixtures, github};
use skein_forge::{api, forgejo, github as gh};
use skein_forge_connection::{
    Component, Event, Limits, Request, Settings,
    hooks::Hinted,
    limits::{self, Forges},
    settings::{Forge, Kind, Transport},
};
use skein_heap::{Measured, Span};
use skein_lib::{
    Env, Intake, List, Queue, Secret, Time, Token,
    stream::{Down, Read, Up},
};
use skein_tls_world::{meter::Steps, pki, server::Server};
use std::collections::{BTreeMap, VecDeque};

/// Maxima observed at the component boundary; tables are checked by lookup.
#[derive(Debug)]
pub struct Full {
    pub owned: u64,
    pub bound: u64,
    pub owned_bytes: u64,
    pub forges: u32,
    pub calls: u32,
    pub connections: u32,
    pub repositories: u32,
    pub users: u32,
    pub hooks: u32,
    pub installations: u32,
    pub minted_tokens: u32,
    pub held_answers: u32,
    pub held_hints: u32,
    pub file_rows: u32,
    pub compare_commits: u32,
    pub reviewers: u32,
    pub queued: u32,
    pub raw_bytes: u32,
    pub op_bytes: u32,
    pub body_bytes: u32,
    pub rows: u32,
    pub inner: u32,
    pub key_bytes: u32,
    pub jwt_bytes: u32,
    pub answer_bytes: u32,
}
impl Full {
    #[must_use]
    pub const fn saturation(&self) -> [u32; 15] {
        [
            self.forges,
            self.calls,
            self.connections,
            self.repositories,
            self.users,
            self.hooks,
            self.installations,
            self.minted_tokens,
            self.held_answers,
            self.file_rows,
            self.compare_commits,
            self.reviewers,
            self.queued,
            self.raw_bytes,
            self.held_hints,
        ]
    }
}

// A prompt in-memory endpoint. Intake checks every read; sends consume granted room.
struct Peer {
    forge: u16,
    owner: Option<Token>,
    tls: Option<Server>,
    input: Intake,
    pending: VecDeque<u8>,
    received: Vec<u8>,
    demand: Option<(Read, u32)>,
    granted: u32,
    consumed: usize,
}
impl Peer {
    fn new(forge: u16, tls: bool) -> Self {
        Self {
            forge,
            owner: None,
            tls: tls.then(|| Server::new(pki::Server::plain().config())),
            input: Intake::with_capacity(fixtures::shared().io.intake),
            pending: VecDeque::new(),
            received: Vec::new(),
            demand: None,
            granted: 0,
            consumed: 0,
        }
    }
    fn take(&mut self, down: Down) {
        match down {
            Down::Demand { read: Read::Nothing, room: 0 } => {
                assert!(self.demand.take().is_some());
            }
            Down::Demand { read, room } => {
                assert!(self.demand.replace((read, room)).is_none());
            }
            Down::Send(bytes) => {
                assert!(bytes.len() <= usize::try_from(self.granted).expect("room"));
                self.granted = 0;
                if let Some(tls) = &mut self.tls {
                    tls.receive(&bytes);
                    assert!(tls.failed.is_none(), "fixture TLS handshake");
                    self.pending.extend(tls.transmit());
                } else {
                    self.received.extend_from_slice(&bytes);
                }
            }
            Down::Finish => {}
        }
    }
    fn write(&mut self, bytes: &[u8]) {
        if let Some(tls) = &mut self.tls {
            for piece in bytes.chunks(1024) {
                tls.write(piece);
                self.pending.extend(tls.transmit());
            }
        } else {
            self.pending.extend(bytes);
        }
    }
    fn answer(&mut self) -> Option<Up> {
        if self.input.room() > 0 && !self.pending.is_empty() {
            let count = self.pending.len().min(usize::try_from(self.input.room()).expect("intake room"));
            let bytes: Vec<_> = self.pending.drain(..count).collect();
            self.input.append(&bytes).expect("bounded input");
        }
        let (read, room) = self.demand?;
        if let Some(bytes) = self.input.meet(read) {
            self.demand = None;
            return Some(Up::Bytes(bytes));
        }
        if room > 0 {
            self.demand = None;
            self.granted = room;
            return Some(Up::Room);
        }
        None
    }
    fn request(&mut self) -> Option<Vec<u8>> {
        let bytes = self.tls.as_ref().map_or(self.received.as_slice(), |tls| tls.received.as_slice());
        let pending = &bytes[self.consumed..];
        let end = pending.windows(4).position(|p| p == b"\r\n\r\n")? + 4;
        let length = pending[..end]
            .split(|b| *b == b'\n')
            .find_map(|line| {
                line.strip_prefix(b"Content-Length: ")
                    .map(|v| std::str::from_utf8(v.trim_ascii()).expect("length").parse::<usize>().expect("number"))
            })
            .unwrap_or(0);
        if pending.len() < end + length {
            return None;
        }
        let request = pending[..end + length].to_vec();
        self.consumed += request.len();
        Some(request)
    }
}

enum Io {
    Connect(Token, skein_io::Addr),
    Listen(Token, skein_io::Addr),
    Bind(Token, Token),
    Stream(Token, Down),
    Close(Token),
}
struct Saved {
    request: Request,
    heap: i64,
}
struct World {
    component: Component,
    env: Env<Limits>,
    steps: Steps,
    above: Queue<Event>,
    below: Queue<skein_io::Request>,
    ios: VecDeque<Io>,
    saved: VecDeque<Saved>,
    peers: BTreeMap<Token, Peer>,
    kinds: Vec<Kind>,
    listener: Option<Token>,
    _trust: skein_tls::Config,
    held: u32,
    hints: u32,
    live_calls: u32,
    seen: Full,
    hold_answers: bool,
    hold_responses: bool,
    responses: Vec<(Token, Vec<u8>)>,
    closed: bool,
}
impl World {
    fn new(forges: Forges) -> Self {
        let l = fixtures::limits();
        let s = fixtures::shared();
        let above = Queue::with_capacity(64);
        let below = Queue::with_capacity(64);
        let kinds: Vec<_> = std::iter::repeat_n(Kind::Forgejo, usize::try_from(forges.forgejo).expect("count"))
            .chain(std::iter::repeat_n(Kind::GitHub, usize::try_from(forges.github).expect("count")))
            .collect();
        assert_eq!(kinds.len(), 2, "full fixture");
        // Shared trust is its owner's allocation, as in the TLS machine world.
        let trust = pki::client(&[]);
        let owned_bytes = 3 + 128 + 2 * u64::try_from("skein.test".len()).expect("name");
        let bound = limits::worst_case(&l, &s, forges, owned_bytes).expect("fixture bound");
        let steps = Steps::new(bound);
        steps.start();
        let mut configured = List::with_capacity(l.forges);
        for (at, kind) in kinds.iter().copied().enumerate() {
            configured
                .push(Forge {
                    number: u16::try_from(at + 1).expect("forge"),
                    kind,
                    log_origins: List::with_capacity(0),
                    address: skein_io::Addr::Ip(([127, 0, 0, 1], 3000 + u16::try_from(at).expect("port")).into()),
                    transport: Transport::Tls { server_name: pki::name(), trust: trust.clone() },
                    access: None,
                })
                .expect("forges");
        }
        let settings = Settings {
            forges: configured,
            webhooks: Some(skein_forge_connection::hooks::Webhooks {
                listener: skein_io::Addr::Ip(([127, 0, 0, 1], 7200).into()),
            }),
            word: Box::from(b"kit".as_slice()),
            user_agent: vec![b'a'; 128].into_boxed_slice(),
        };
        assert_eq!(skein_forge_connection::settings::owned_bytes(&settings), Some(owned_bytes));
        let component = Component::new(settings, &l, &s).expect("full fixture accepted");
        let measured = steps.end();
        let mut steps = steps;
        steps.check(measured, 0, &"construction");
        Self {
            component,
            env: Env { now: Time::ZERO, wall: pki::VALID, limits: l },
            steps,
            above,
            below,
            ios: VecDeque::new(),
            saved: VecDeque::new(),
            peers: BTreeMap::new(),
            kinds,
            listener: None,
            _trust: trust,
            held: 0,
            hints: 0,
            live_calls: 0,
            hold_answers: false,
            hold_responses: false,
            responses: Vec::new(),
            closed: false,
            seen: Full {
                owned: 0,
                bound,
                owned_bytes,
                forges: 0,
                calls: 0,
                connections: 0,
                repositories: 0,
                users: 0,
                hooks: 0,
                installations: 0,
                minted_tokens: 0,
                held_answers: 0,
                held_hints: 0,
                file_rows: 0,
                compare_commits: 0,
                reviewers: 0,
                queued: 0,
                raw_bytes: 0,
                op_bytes: 0,
                body_bytes: 0,
                rows: 0,
                inner: 0,
                key_bytes: 0,
                jwt_bytes: 0,
                answer_bytes: 0,
            },
        }
    }
    // Everything in these spans belongs to the owner, byte peer or TLS server.
    fn outside<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let span = Span::start();
        let result = f(self);
        self.steps.outside(span.end().net);
        result
    }
    fn down(&mut self, make: impl FnOnce() -> Request) {
        self.steps.start();
        let request = make();
        if let Request::Connect { access, .. } = &request
            && let skein_forge::credential::Credential::App { key, .. } = &access.credential
        {
            self.seen.key_bytes = self.seen.key_bytes.max(u32::try_from(key.len()).expect("key bytes"));
        }
        if let Request::Call { op, .. } = &request {
            self.seen.op_bytes = self
                .seen
                .op_bytes
                .max(u32::try_from(skein_forge::measure::op(op).expect("bounded operation")).expect("operation bytes"));
        }
        self.component.down(&self.env, request, &mut self.above, &mut self.below);
        self.finish(self.steps.end());
    }
    fn up(&mut self, event: skein_io::Event) {
        if let skein_io::Event::Stream { up: Up::Bytes(bytes), .. } = &event {
            self.steps.outside(-i64::try_from(bytes.len()).expect("input bytes"));
        }
        self.steps.start();
        self.component.up(&self.env, event, &mut self.above, &mut self.below);
        self.finish(self.steps.end());
    }
    fn finish(&mut self, step: Measured) {
        // Clone only for the peer/owner, then drop every original handoff before check.
        // The clones' net allocation is charged outside only after this check.
        let mut outside = 0;
        while let Some(request) = self.below.pop() {
            let span = Span::start();
            let copy = match &request {
                skein_io::Request::Connect { owner, addr } => Io::Connect(*owner, *addr),
                skein_io::Request::Listen { owner, addr } => Io::Listen(*owner, *addr),
                skein_io::Request::Bind { owner, socket } => Io::Bind(*owner, *socket),
                skein_io::Request::Stream { stream, down } => Io::Stream(
                    *stream,
                    match down {
                        Down::Demand { read, room } => Down::Demand { read: *read, room: *room },
                        Down::Send(bytes) => Down::Send(bytes.clone()),
                        Down::Finish => Down::Finish,
                    },
                ),
                skein_io::Request::Close { entity } | skein_io::Request::Abort { entity } => Io::Close(*entity),
                other @ (skein_io::Request::Reject { .. }
                | skein_io::Request::Output { .. }
                | skein_io::Request::Spawn { .. }
                | skein_io::Request::Signal { .. }
                | skein_io::Request::Usage { .. }
                | skein_io::Request::Window { .. }) => panic!("socket io only: {other:?}"),
            };
            self.ios.push_back(copy);
            outside += span.end().net;
            drop(request);
        }
        while let Some(event) = self.above.pop() {
            let span = Span::start();
            match &event {
                Event::Answered(answer) => {
                    assert!(answer.result.is_ok(), "full answer: {answer:?}");
                    self.live_calls -= 1;
                    if let Ok(value) = &answer.result {
                        observe(&mut self.seen, value);
                    }
                    if self.hold_answers {
                        let cloning = Span::start();
                        let request = Request::Returned(answer.clone());
                        let heap = cloning.end().net;
                        self.saved.push_back(Saved { request, heap });
                        self.held += 1;
                        self.seen.held_answers = self.seen.held_answers.max(self.held);
                    }
                }
                Event::Hint(hint) => {
                    assert_eq!(hint.hint.key.as_ref().expect("full marker").len(), 512);
                    let cloning = Span::start();
                    let request = Request::RefusedHint(Hinted { connection: hint.connection, hint: hint.hint.clone() });
                    let heap = cloning.end().net;
                    self.saved.push_back(Saved { request, heap });
                    self.hints += 1;
                    self.seen.held_hints = self.seen.held_hints.max(self.hints);
                }
                Event::Connected { .. } => self.seen.forges += 1,
                Event::Installation { .. } | Event::Fact { .. } | Event::Listening { .. } => {}
                Event::GitToken { token, .. } => {
                    assert_eq!(token.len(), usize::try_from(self.env.limits.token_bytes).expect("token"));
                    // The scripted mint records Full separately; this proves Contents crossed the boundary.
                    self.seen.minted_tokens += 1;
                    self.seen.installations += 1;
                }
                Event::Closed => self.closed = true,
                Event::Stopped { .. } | Event::ListenerLost => panic!("full fixture stopped: {event:?}"),
            }
            outside += span.end().net;
            drop(event);
        }
        self.steps.check(step, 0, &"component entry point");
        self.steps.outside(outside);
    }
    fn settle(&mut self) {
        for _ in 0..100_000 {
            self.steps.start();
            self.component.reclaim();
            self.finish(self.steps.end());
            if let Some(saved) = self.outside(|w| w.saved.pop_front()) {
                self.steps.outside(-saved.heap);
                self.down(|| saved.request);
                continue;
            }
            if let Some(io) = self.outside(|w| w.ios.pop_front()) {
                self.io(io);
                continue;
            }
            let next = self.outside(|w| {
                for (owner, peer) in &mut w.peers {
                    if peer.tls.is_some()
                        && let Some(request) = peer.request()
                    {
                        let response =
                            response(peer.forge, w.kinds[usize::from(peer.forge) - 1], &request, &mut w.seen);
                        if w.hold_responses {
                            w.responses.push((*owner, response));
                        } else {
                            peer.write(&response);
                        }
                        return Some(None);
                    }
                    if let Some(up) = peer.answer() {
                        return Some(Some(skein_io::Event::Stream { owner: peer.owner.expect("bound peer"), up }));
                    }
                }
                None
            });
            match next {
                Some(Some(event)) => self.up(event),
                Some(None) => {}
                None => {
                    if self.component.next_deadline().is_some_and(|t| t <= self.env.now) {
                        self.steps.start();
                        self.component.fire(&self.env, &mut self.above, &mut self.below);
                        self.finish(self.steps.end());
                    } else {
                        return;
                    }
                }
            }
        }
        panic!("full holder run failed to settle");
    }
    fn io(&mut self, io: Io) {
        match io {
            Io::Connect(owner, addr) => {
                let skein_io::Addr::Ip(addr) = addr else { panic!("IP fixture") };
                let forge = addr.port() - 2999;
                self.outside(|w| {
                    let mut peer = Peer::new(forge, true);
                    peer.owner = Some(owner);
                    w.peers.insert(owner, peer);
                });
                self.seen.connections = self
                    .seen
                    .connections
                    .max(u32::try_from(self.peers.values().filter(|p| p.tls.is_some()).count()).expect("connections"));
                self.up(skein_io::Event::Connecting { owner, socket: owner });
                self.up(skein_io::Event::Connected { owner });
            }
            Io::Listen(owner, addr) => {
                self.listener = Some(owner);
                self.up(skein_io::Event::Listening { owner, listener: owner, addr });
            }
            Io::Bind(owner, socket) => {
                self.peers.get_mut(&socket).expect("accepted").owner = Some(owner);
            }
            Io::Stream(owner, down) => self.outside(|w| w.peers.get_mut(&owner).expect("peer").take(down)),
            Io::Close(owner) => {
                let owner = self.outside(|w| w.peers.remove(&owner).and_then(|p| p.owner).unwrap_or(owner));
                self.up(skein_io::Event::Closed { owner });
            }
        }
    }
    fn call(&mut self, call: u64, forge: u16, make: impl FnOnce() -> api::Op) {
        self.live_calls += 1;
        self.seen.calls = self.seen.calls.max(self.live_calls);
        self.down(|| Request::Call {
            call: Token::new(call),
            repository: api::Repository { forge, repository: 1 },
            op: make(),
        });
    }
    fn fill_tables(&mut self) {
        self.steps.start();
        let l = self.env.limits;
        for (at, kind) in self.kinds.iter().copied().enumerate() {
            let forge = u16::try_from(at + 1).expect("forge");
            for repository in 1..=l.repositories {
                let key = api::Repository { forge, repository };
                match kind {
                    Kind::Forgejo => self.component.forgejo_tables().keep_repository(
                        key,
                        forgejo::plans::tables::Repository {
                            owner: text(b'o'),
                            name: text(b'n'),
                            html_url: text(b'h'),
                            object_format: forgejo::id::ObjectFormat::Sha1,
                            merge_style: forgejo::document::MergeStyle::Merge,
                        },
                    ),
                    Kind::GitHub => self.component.github_tables().keep_repository(
                        key,
                        gh::plans::tables::Repository {
                            owner: text(b'o'),
                            name: text(b'n'),
                            html_url: text(b'h'),
                            owner_id: 1,
                            organization: false,
                            merge: Some(gh::plans::tables::Method::Merge),
                        },
                    ),
                }
            }
            for user in 1..=l.users {
                match kind {
                    Kind::Forgejo => self.component.forgejo_tables().keep_user(forge, u64::from(user), text(b'u')),
                    Kind::GitHub => self.component.github_tables().keep_user(forge, u64::from(user), text(b'u')),
                }
            }
        }
        // Each dialect has a shared table: refill/inspect the final forge of that kind.
        self.seen.repositories = 0;
        self.seen.users = 0;
        for kind in [Kind::Forgejo, Kind::GitHub] {
            if let Some(at) = self.kinds.iter().rposition(|k| *k == kind) {
                let forge = u16::try_from(at + 1).expect("forge");
                for repository in 1..=l.repositories {
                    let key = api::Repository { forge, repository };
                    let found = match kind {
                        Kind::Forgejo => self.component.forgejo_tables().repository(key).is_some(),
                        Kind::GitHub => self.component.github_tables().repository(key).is_some(),
                    };
                    self.seen.repositories += u32::from(found);
                }
                for user in 1..=l.users {
                    let found = match kind {
                        Kind::Forgejo => self.component.forgejo_tables().user(forge, u64::from(user)).is_some(),
                        Kind::GitHub => self.component.github_tables().user(forge, u64::from(user)).is_some(),
                    };
                    self.seen.users += u32::from(found);
                }
            }
        }
        self.finish(self.steps.end());
    }
}

fn text(byte: u8) -> Box<[u8]> {
    vec![byte; usize::try_from(fixtures::limits().string_bytes).expect("string")].into_boxed_slice()
}
fn observe(full: &mut Full, answer: &api::Answer) {
    let bytes = u32::try_from(skein_forge::measure::answer(answer).expect("bounded answer")).expect("answer bytes");
    full.answer_bytes = full.answer_bytes.max(bytes);
    if let api::Answer::Compare { files, commits, .. } = answer {
        assert_eq!(bytes, 65_536, "the full comparison fills its answer");
        full.file_rows = full.file_rows.max(u32::try_from(files.len()).expect("files"));
        full.compare_commits = full.compare_commits.max(u32::try_from(commits.len()).expect("commits"));
    } else if let api::Answer::PullFiles { files, .. } = answer {
        assert_eq!(bytes, 65_536, "the full files page fills its answer");
        full.file_rows = full.file_rows.max(u32::try_from(files.len()).expect("files"));
    } else if let api::Answer::File { bytes, .. } = answer {
        full.raw_bytes = full.raw_bytes.max(u32::try_from(bytes.len()).expect("raw"));
    } else if let api::Answer::Items { items, .. } = answer {
        full.rows = full.rows.max(u32::try_from(items.len()).expect("rows"));
        for item in items {
            full.inner = full.inner.max(u32::try_from(item.labels.len()).expect("labels"));
            full.body_bytes = full.body_bytes.max(u32::try_from(item.body.len()).expect("body"));
        }
    } else if let api::Answer::Queued = answer {
        full.queued += 1;
    }
}

const ZERO: &str = "0000000000000000000000000000000000000000";
const ONE: &str = "1111111111111111111111111111111111111111";
fn response(_forge: u16, kind: Kind, request: &[u8], full: &mut Full) -> Vec<u8> {
    let first = request.split(|b| *b == b'\r').next().expect("request line");
    let line = std::str::from_utf8(first).expect("ASCII target");
    let target = line.split(' ').nth(1).expect("target");
    let text = "x".repeat(128);
    if kind == Kind::GitHub
        && let Some(header) =
            request.split(|b| *b == b'\n').find_map(|line| line.strip_prefix(b"Authorization: Bearer "))
    {
        let length = header.trim_ascii().len();
        if length > 64 {
            full.jwt_bytes = full.jwt_bytes.max(u32::try_from(length).expect("JWT length"));
        }
    }
    let mut status = 200;
    let body = if target == "/api/v1/user" {
        format!(r#"{{"id":2,"login":"{text}"}}"#)
    } else if target == "/api/v1/settings/api" {
        r#"{"max_response_items":8,"default_paging_num":8}"#.to_owned()
    } else if target.starts_with("/app/installations?") {
        if target.contains("page=2") {
            "[]".to_owned()
        } else {
            let rows: Vec<_> = (1..=100)
                .map(|id| format!(r#"{{"id":{id},"account":{{"id":{id},"login":"{text}","type":"User"}},"created_at":"2026-01-01T00:00:00Z","suspended_at":null}}"#))
                .collect();
            format!("[{}]", rows.join(","))
        }
    } else if target.ends_with("/access_tokens") {
        status = 201;
        if !request.windows(b"\"contents\"".len()).any(|p| p == b"\"contents\"") {
            full.minted_tokens += 1;
        }
        format!(r#"{{"token":"{}","expires_at":"2030-01-01T01:00:00Z"}}"#, "t".repeat(64))
    } else if target == "/graphql" {
        if request.windows(b"enqueuePullRequest".len()).any(|p| p == b"enqueuePullRequest") {
            r#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":1}}}}"#.to_owned()
        } else if request.windows(b"viewer".len()).any(|p| p == b"viewer") {
            format!(r#"{{"data":{{"viewer":{{"databaseId":9,"login":"{text}"}}}}}}"#)
        } else {
            format!(
                r#"{{"data":{{"repository":{{"pullRequest":{{"id":"{text}","mergeCommit":null,"mergeQueueEntry":null,"timelineItems":{{"nodes":[]}}}}}}}}}}"#
            )
        }
    } else if target.ends_with("/merge") {
        status = 405;
        r#"{"message":"Repository rule violations found\nChanges must be made through the merge queue"}"#.to_owned()
    } else if target.ends_with("/requested_reviewers") {
        if line.starts_with("POST ") {
            status = 201;
        }
        let additions = target.contains("/pulls/2/");
        let width = if additions { 125 } else { 128 };
        let count = request.windows(width).filter(|p| p.iter().all(|b| *b == b'u')).count();
        assert_eq!(count, if additions { 8 } else { 4 }, "bounded difference request");
        if additions {
            full.reviewers = full.reviewers.max(8);
        } else {
            full.reviewers += u32::try_from(count).expect("reviewers");
        }
        "{}".to_owned()
    } else if target.starts_with("/user/") {
        let id = target.strip_prefix("/user/").expect("user");
        let width = if id.parse::<u64>().expect("user id") >= 17 { 125 } else { 128 };
        format!(r#"{{"id":{id},"login":"{}"}}"#, "u".repeat(width))
    } else if target.ends_with("/comments") && line.starts_with("POST ") {
        status = 201;
        format!(r#"{{"id":1,"user":{{"id":2,"login":"{text}"}},"body":"{}"}}"#, "b".repeat(1042))
    } else if target.contains("/issues?") {
        let labels = (0..8).map(|_| format!(r#"{{"name":"{text}"}}"#)).collect::<Vec<_>>().join(",");
        let items = (1..=8).map(|number| format!(r#"{{"number":{number},"state":"open","user":{{"id":2,"login":"{text}"}},"labels":[{labels}],"updated_at":"2026-01-01T00:00:00Z","title":"{text}","body":"{}"}}"#, "b".repeat(1042))).collect::<Vec<_>>().join(",");
        return fixtures::dated_response(200, format!("[{items}]").as_bytes());
    } else if target.contains("/pulls/") && !target.contains("/files?") {
        pull_body(target, &text)
    } else if target.contains("/compare/") {
        return comparison_response(kind, target);
    } else if target.contains("/files?") {
        format!("[{}]", file_descriptors(false))
    } else if target.contains("/raw/") || target.contains("/contents/") {
        // File's path is 128 bytes,
        // leaving 65408 bytes in its answer; its distinct 'r' path selects that body.
        let single = target.contains(&"r".repeat(128));
        let comparison = target.contains(&"c".repeat(127));
        return fixtures::response(
            200,
            &vec![
                b'b';
                if single {
                    65408
                } else if comparison {
                    if kind == Kind::Forgejo { 3984 } else { 3998 }
                } else {
                    4000
                }
            ],
        );
    } else {
        panic!("unhandled {kind:?} script target: {line}");
    };
    fixtures::response(status, body.as_bytes())
}
fn file_descriptors(comparison: bool) -> String {
    (0..8)
        .map(|i| {
            let path = format!("{i}{}", (if comparison { "c" } else { "p" }).repeat(127));
            let old = format!("{i}{}", "q".repeat(127));
            format!(r#"{{"filename":"{path}","previous_filename":"{old}","status":"renamed"}}"#)
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Measures each kind at the machine fixture's limits, including reachable handoffs.
#[must_use]
pub fn full_heap(forges: Forges) -> Full {
    let mut w = World::new(forges);
    w.steps.start();
    w.component.begin(&w.env, &mut w.above, &mut w.below);
    w.finish(w.steps.end());
    for at in 0..w.kinds.len() {
        let kind = w.kinds[at];
        let l = w.env.limits;
        if w.kinds[at] == Kind::GitHub {
            w.down(|| Request::Installations {
                forge: u16::try_from(at + 1).expect("forge"),
                accepted: Box::from([1, 2, 3]),
            });
        }
        w.down(|| Request::Connect { forge: u16::try_from(at + 1).expect("forge"), access: access(kind, l) });
    }
    w.settle();
    for _ in 0..16 {
        if w.seen.forges == 2 {
            break;
        }
        w.down(|| Request::Offer);
        w.settle();
    }
    assert_eq!(w.seen.forges, 2, "both configured starts reach Ready");
    w.fill_tables();
    w.fill_hooks();
    let at = w.kinds.iter().rposition(|k| *k == Kind::GitHub).unwrap_or(w.kinds.len() - 1);
    let forge = u16::try_from(at + 1).expect("forge");
    // Both calls remain outstanding at the response boundary, then complete together.
    {
        w.hold_responses = true;
        w.call(1, forge, || api::Op::Read(api::Read::Compare { before: [0; 32], after: head() }));
        w.call(2, forge, || api::Op::Read(api::Read::PullFiles { number: 1, head: head(), page: 1 }));
        w.settle();
        assert_eq!(w.live_calls, 2);
        w.release_responses();
        assert_eq!(w.live_calls, 0, "both file calls completed");
    }
    if forges.github > 0 {
        w.call(3, forge, || api::Op::Write(api::Write::Merge { number: 1, head: head() }));
        w.settle();
        w.call(4, forge, || {
            api::Op::Write(api::Write::SetReviewers {
                number: 1,
                reviewers: (5..=12).collect::<Vec<_>>().into_boxed_slice(),
            })
        });
        w.settle();
        // Eight additions fill the pending mutation's login slots; 125-byte
        // logins fit its serialized body, beside the separate 128-byte case.
        w.call(10, forge, || {
            api::Op::Write(api::Write::SetReviewers {
                number: 2,
                reviewers: (17..=24).collect::<Vec<_>>().into_boxed_slice(),
            })
        });
        w.settle();
        w.fill_tables();
    }
    w.full_operations(forge, forges.github > 0);
    // A complete maximum raw answer in every call slot, then retained by Returned.
    w.hold_answers = true;
    w.hold_responses = true;
    for call in [5, 6] {
        w.call(call, forge, || api::Op::Read(api::Read::File { head: head(), path: text(b'r'), max_bytes: 65408 }));
    }
    w.settle();
    assert_eq!(w.live_calls, 2);
    w.release_responses();
    assert_eq!(w.live_calls, 0);
    w.down(|| Request::Close);
    w.settle();
    assert!(w.closed, "all transports closed");
    assert_eq!(w.component.diagnostics(), skein_forge_connection::Diagnostics { long_bodies: 0, malformed: 0 });
    w.seen.owned = w.steps.finish(w.component, 0, &"full holders release all owned allocations");
    w.seen
}

fn access(kind: Kind, l: Limits) -> skein_forge_connection::Access {
    match kind {
        Kind::Forgejo => fixtures::access(
            &vec![b't'; usize::try_from(l.token_bytes).expect("token")],
            fixtures::WRITER,
            Some(&vec![b's'; usize::try_from(l.hook_secret_bytes).expect("secret")]),
        ),
        Kind::GitHub => skein_forge_connection::Access {
            credential: skein_forge::credential::Credential::App {
                id: 7,
                key: Secret::new(Box::from(github::app_key())),
            },
            user: 9,
            webhook_secret: None,
        },
    }
}

fn head() -> api::Commit {
    let mut head = [0; 32];
    head[..20].fill(0x11);
    head
}

impl World {
    fn fill_hooks(&mut self) {
        let w = self;
        // Signed hints coexist with tables, active calls and returned answers.
        {
            let forgejo = w.kinds.iter().position(|k| *k == Kind::Forgejo);
            let at = forgejo.unwrap_or(0);
            for n in 0..w.env.limits.hooks {
                let socket = Token::new(10_000 + u64::from(n));
                let forge = u16::try_from(at + 1).expect("forge");
                w.outside(|w| {
                    w.peers.insert(socket, Peer::new(forge, false));
                });
                w.up(skein_io::Event::Accepted {
                    owner: w.listener.expect("listener"),
                    socket,
                    peer: skein_io::Addr::Ip(([127, 0, 0, 1], 9000).into()),
                });
                w.settle();
                w.seen.hooks += 1;
                w.outside(|w| {
                if forgejo.is_none() {
                    let mut head = b"POST /hooks/1 HTTP/1.1\r\nX-Padding: ".to_vec();
                    head.resize(8191, b'p');
                    w.peers.get_mut(&socket).expect("accepted").write(&head);
                    return;
                }
                let body = format!(r#"{{"repository":{{"id":1}},"issue":{{"number":1}},"comment":{{"body":"<!-- kit:key {} -->\n"}}}}"#, "61".repeat(512));
                let secret = Secret::new(vec![b's';128].into_boxed_slice());
                let signature = skein_fake_forge_protocol::delivery::sign(&secret, body.as_bytes());
                let head = format!("POST /hooks/{forge} HTTP/1.1\r\nHost: loopback\r\nContent-Length: {}\r\nX-Forgejo-Event: issue_comment\r\nX-Forgejo-Signature: {}\r\n\r\n", body.len(), std::str::from_utf8(&signature).expect("signature"));
                let peer = w.peers.values_mut().find(|p| p.tls.is_none() && p.pending.is_empty() && p.consumed == 0 && p.received.is_empty()).expect("new hook peer");
                peer.write(head.as_bytes()); peer.write(body.as_bytes());
            });
                w.settle();
            }
        }
    }
}

impl World {
    fn release_responses(&mut self) {
        self.hold_responses = false;
        self.outside(|w| {
            for (owner, bytes) in w.responses.drain(..) {
                w.peers.get_mut(&owner).expect("peer").write(&bytes);
            }
        });
        self.settle();
    }
    fn full_operations(&mut self, forge: u16, github: bool) {
        self.call(7, forge, || api::Op::Read(api::Read::Items { since: Time::ZERO, page: 1, kind: None }));
        self.settle();
        self.hold_responses = true;
        for call in [8, 9] {
            self.call(call, forge, || {
                api::Op::Write(api::Write::Post {
                    number: 1,
                    // The GitHub JSON envelope and escaped marker newline share body_bytes.
                    key: vec![b'k'; if github { 500 } else { 512 }].into_boxed_slice(),
                    body: vec![b'b'; if github { 12 } else { 0 }].into_boxed_slice(),
                })
            });
        }
        self.settle();
        assert_eq!(self.live_calls, 2);
        self.release_responses();
        assert_eq!(self.live_calls, 0);
    }
}

fn pull_body(target: &str, text: &str) -> String {
    let number = if target.ends_with("/2") { 2 } else { 1 };
    let reviewers: Vec<_> =
        (1..=if number == 2 { 0 } else { 8 }).map(|id| format!(r#"{{"id":{id},"login":"{text}"}}"#)).collect();
    format!(
        r#"{{"number":{number},"state":"open","merged":false,"merge_commit_sha":null,"mergeable":true,"head":{{"ref":"{text}","sha":"{ONE}","repo_id":1}},"base":{{"ref":"{text}","sha":"{ZERO}","repo_id":1}},"merge_base":"{ZERO}","requested_reviewers":[{}]}}"#,
        reviewers.join(",")
    )
}

fn comparison_response(kind: Kind, target: &str) -> Vec<u8> {
    if kind == Kind::Forgejo {
        let commits = (0..8).map(|_| format!(r#"{{"sha":"{ONE}"}}"#)).collect::<Vec<_>>().join(",");
        let body = if target.contains("files=false") {
            r#"{"total_commits":0,"commits":[],"files":[]}"#.to_owned()
        } else {
            format!(r#"{{"total_commits":8,"commits":[{commits}],"files":[{}]}}"#, file_descriptors(true))
        };
        return fixtures::response(200, body.as_bytes());
    }
    assert!(
        target.contains("per_page=8") || target.contains("per_page=1"),
        "Compare admits eight commits; PullFiles asks only for the merge base"
    );
    let files = file_descriptors(true);
    // The response holds one commit and eight files.
    let body = format!(
        r#"{{"total_commits":1,"behind_by":0,"merge_base_commit":{{"sha":"{ZERO}"}},"commits":[{{"sha":"{ONE}"}}],"files":[{files}]}}"#
    );
    fixtures::response(200, body.as_bytes())
}
