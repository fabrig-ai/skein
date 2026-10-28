//! HTTP framing over seeded streams for the landed in-process GitHub service.
use crate::{fixtures, github_fixture as fixture, referee::Accounting};
use skein_fake_forge_domain::{self as fake, api as raw};
use skein_fake_forge_github::service::{Outcome, Pending, Request as ApiRequest, Service};
use skein_forge::github::request::Method;
use skein_forge_connection::{Component, Event, Limits, Request};
use skein_forge_machine_world::{
    fixtures as machine,
    joint::REPO,
    pipe::{Cut, Pipe},
};
use skein_http::server as http;
use skein_lib::{
    Duration, Env, Queue, ReplyTo, Rng, Secret, Time, Token, Wall,
    stream::{Down, Read, Up},
};
use std::collections::BTreeMap;

// Recovery listing timestamps must be above the observed issues-since cutoff.
const WALL_BASE: Wall = Wall::from_nanos(31_536_000_000_000_000);
const TURN: Duration = Duration::from_nanos(100_000);

#[derive(Default)]
struct Trace {
    own: bool,
    served: bool,
}

struct Peer {
    pipe: Pipe,
    server: http::Server,
    above: Queue<http::Event>,
    below: Queue<Down>,
    head: Option<http::Call>,
    body: Vec<u8>,
    reply: Vec<u8>,
    offset: usize,
    traces: Vec<Trace>,
    entrance: usize,
    sent: Vec<u8>,
    remaining: usize,
    delayed: bool,
    received: Vec<u8>,
    receiving_body: usize,
    delivered_heads: usize,
}
impl Peer {
    fn action(&mut self, env: &Env<http::Limits>, request: http::Request) {
        http::down(&mut self.server, env, request, &mut self.above, &mut self.below);
    }

    // Observe bytes at Send, before the pipe cuts or fragments them. Heads are
    // queued whole by HTTP; bodies may arrive in separate sends.
    fn sent(&mut self, bytes: &[u8], secrets: &mut Vec<Secret>) {
        if bytes.is_empty() {
            return;
        }
        if self.remaining > 0 {
            assert!(bytes.len() <= self.remaining, "one request body per Send");
            self.remaining -= bytes.len();
            return;
        }
        if self.sent.is_empty() {
            self.traces.push(Trace::default());
        }
        self.sent.extend_from_slice(bytes);
        let Some(end) = self.sent.windows(4).position(|w| w == b"\r\n\r\n") else { return };
        let mut lines = self.sent[..end].split(|b| *b == b'\n').map(<[u8]>::trim_ascii);
        let line = lines.next().expect("request line");
        let target = line.split(|b| *b == b' ').nth(1).expect("target");
        self.traces.last_mut().expect("first byte observed").own =
            target.starts_with(b"/app/") || target == b"/graphql";
        let mut length = 0;
        for line in lines {
            let colon = line.iter().position(|b| *b == b':').expect("header colon");
            let (name, value) = (&line[..colon], line[colon + 1..].trim_ascii());
            if name.eq_ignore_ascii_case(b"Authorization") {
                let bearer = value.strip_prefix(b"Bearer ").expect("GitHub bearer");
                secrets.push(Secret::new(Box::from(bearer)));
            }
            if name.eq_ignore_ascii_case(b"Content-Length") {
                length = std::str::from_utf8(value).expect("length text").parse::<usize>().expect("length");
            }
        }
        self.remaining = (end + 4 + length).checked_sub(self.sent.len()).expect("one request per Send");
        self.sent.clear();
    }
    // A second framing observer counts complete heads in delivered bytes,
    // independently of both the component and the HTTP server's Call events.
    fn received(&mut self, bytes: &[u8]) {
        self.received.extend_from_slice(bytes);
        loop {
            let consume = self.receiving_body.min(self.received.len());
            self.received.drain(..consume);
            self.receiving_body -= consume;
            if self.receiving_body > 0 {
                return;
            }
            let Some(end) = self.received.windows(4).position(|w| w == b"\r\n\r\n") else { return };
            for line in self.received[..end].split(|b| *b == b'\n').skip(1) {
                let colon = line.iter().position(|b| *b == b':').expect("header colon");
                if line[..colon].eq_ignore_ascii_case(b"Content-Length") {
                    self.receiving_body = std::str::from_utf8(line[colon + 1..].trim_ascii())
                        .expect("body length")
                        .parse()
                        .expect("body length number");
                }
            }
            self.delivered_heads += 1;
            self.received.drain(..end + 4);
        }
    }
}

pub(crate) struct Github {
    pub component: Component,
    pub env: Env<Limits>,
    pub above: Queue<Event>,
    pub below: Queue<skein_io::Request>,
    pub fake_env: Env<fake::Config>,
    pub domain: fake::Domain,
    pub answers: BTreeMap<u64, skein_forge_connection::Answered>,
    pub observations: Vec<fake::Observation>,
    pub hints: Vec<skein_forge_connection::hooks::Hinted>,
    pub events: Vec<String>,
    pub connected: u64,
    pub closed: bool,
    pub secrets: Vec<Secret>,
    pub mints: u64,
    peers: BTreeMap<Token, Peer>,
    service: Service,
    rng: Rng,
    cut_chance: u32,
    lines: Vec<Vec<u8>>,
    totals: Accounting,
    lose_creation: bool,
    seed: u64,
    key: &'static [u8],
}
impl Github {
    pub fn new(seed: u64, cut_chance: u32, log_poll: Duration) -> Self {
        let mut config = machine::fake_config();
        config.latency_min = Duration::ZERO;
        config.latency_max = Duration::ZERO;
        config.skew = fake::Skew::None;
        config.edit_updates = true;
        let mut domain = fake::Domain::new(&config, seed);
        fake::repository(
            &mut domain,
            &config,
            raw::Setup {
                workflows: Box::new([]),
                id: 2,
                name: Box::from(fixture::PATH),
                default: Box::from(b"main".as_slice()),
                tree: Box::from([]),
                labels: Box::from([]),
                checks: skein_forge_machine_world::joint::checks(1000),
                protection: None,
                hooked: true,
            },
        );
        fake::grant(&mut domain, fixture::PATH, fixture::USER, raw::Permission::Admin);
        fake::grant(&mut domain, fixture::PATH, 2, raw::Permission::Write);
        let limits = fixture::limits(log_poll);
        let mut world = Self {
            component: Component::new(fixture::settings(), &limits, &fixtures::shared()).expect("GitHub component"),
            env: Env { now: Time::ZERO, wall: WALL_BASE, limits },
            fake_env: Env { now: Time::from_nanos(WALL_BASE.as_nanos()), wall: WALL_BASE, limits: config },
            above: Queue::with_capacity(128),
            below: Queue::with_capacity(128),
            domain,
            answers: BTreeMap::new(),
            observations: Vec::new(),
            hints: Vec::new(),
            events: Vec::new(),
            connected: 0,
            closed: false,
            peers: BTreeMap::new(),
            service: fixture::service(seed, fixture::key()),
            rng: Rng::new(seed),
            cut_chance,
            lines: Vec::new(),
            totals: Accounting { reported: 0, written: 0, served: 0, cut_in_head: 0 },
            secrets: Vec::new(),
            mints: 0,
            lose_creation: false,
            seed,
            key: fixture::key(),
        };
        world.component.begin(&world.env, &mut world.above, &mut world.below);
        world.action(Request::Installations { forge: 1, accepted: Box::from([1]) });
        world
    }
    pub fn action(&mut self, request: Request) {
        self.component.down(&self.env, request, &mut self.above, &mut self.below);
    }
    pub fn time(&mut self, now: Time) {
        self.env.now = now;
        self.env.wall = Wall::from_nanos(WALL_BASE.as_nanos() + now.as_nanos());
        self.fake_env.now = Time::from_nanos(self.env.wall.as_nanos());
        self.fake_env.wall = self.env.wall;
    }
    pub fn busy(&self) -> bool {
        !self.below.is_empty()
            || self.peers.values().any(|p| p.pipe.has_work() || !p.above.is_empty() || !p.below.is_empty())
    }
    pub fn run_for(&mut self, span: Duration) {
        let end = self.env.now.saturating_add(span);
        while self.env.now < end {
            self.tick();
            let next = if self.busy() {
                self.env.now.saturating_add(TURN)
            } else {
                self.component.next_deadline().unwrap_or(end).max(self.env.now.saturating_add(TURN))
            };
            self.time(next.min(end));
        }
        self.tick();
    }
    fn http_env(&self) -> Env<http::Limits> {
        Env { now: self.env.now, wall: self.env.wall, limits: machine::fake_limits().http }
    }
    fn release_hooks(&mut self) {
        // The bounded domain may have several zero-latency hooks before its next call.
        while self.domain.next_deadline().is_some_and(|at| at <= self.fake_env.now) {
            let mut out = Queue::uncapped();
            fake::fire(&mut self.domain, &self.fake_env, &mut out);
            while let Some(hook) = out.pop() {
                assert!(matches!(hook, fake::Request::Hook { .. }), "direct calls settle synchronously");
                let post = self.service.hook(&hook, self.env.wall).expect("fixture hook fits");
                assert!(post.is_none(), "App hook has no endpoint");
            }
        }
        self.service.poll(self.env.wall);
    }
    pub fn tick(&mut self) {
        self.release_hooks();
        while let Some(request) = self.below.pop() {
            self.io(request);
        }
        let env = self.http_env();
        let owners: Vec<_> = self.peers.keys().copied().collect();
        for owner in owners {
            let mut peer = self.peers.remove(&owner).expect("live peer");
            while let Some(down) = peer.below.pop() {
                peer.pipe.server_down(down);
            }
            if let Some(up) = peer.pipe.server_up() {
                if let Up::Bytes(bytes) = &up {
                    peer.received(bytes);
                }
                http::up(&mut peer.server, &env, up, &mut peer.above, &mut peer.below);
            }
            self.server_events(&mut peer, &env);
            let up = peer.pipe.client_up();
            self.peers.insert(owner, peer);
            if let Some(up) = up {
                self.up(skein_io::Event::Stream { owner, up });
            }
        }
        if self.component.next_deadline().is_some_and(|at| at <= self.env.now) {
            self.component.fire(&self.env, &mut self.above, &mut self.below);
        }
        while let Some(event) = self.above.pop() {
            self.events.push(format!("{event:?}"));
            match event {
                Event::Answered(answer) => {
                    assert!(self.answers.insert(answer.call.raw(), answer).is_none(), "one terminal");
                }
                Event::Connected { .. } => self.connected += 1,
                Event::Closed => self.closed = true,
                Event::Installation { .. } | Event::GitToken { .. } | Event::Fact { .. } => {}
                Event::Stopped { why, .. } => panic!("GitHub stopped: {why:?}, seed {}", self.seed),
                Event::Hint(hint) => self.hints.push(hint),
                Event::Listening { .. } | Event::ListenerLost => panic!("GitHub has no listener"),
            }
        }
        while let Some(observation) = self.domain.pop_observation() {
            self.observations.push(observation);
        }
        self.domain.reclaim();
        self.component.reclaim();
    }
    fn server_events(&mut self, peer: &mut Peer, env: &Env<http::Limits>) {
        while let Some(event) = peer.above.pop() {
            match event {
                http::Event::Call(call) => {
                    let trace = peer.traces.get_mut(peer.entrance).expect("head follows write");
                    assert!(!trace.served);
                    trace.served = true;
                    peer.entrance += 1;
                    peer.head = Some(call);
                    peer.body.clear();
                    peer.action(env, http::Request::Body(Down::Demand { read: Read::Arrived { max: 1024 }, room: 0 }));
                }
                http::Event::Body(Up::Bytes(bytes)) => {
                    peer.body.extend_from_slice(&bytes);
                    peer.action(env, http::Request::Body(Down::Demand { read: Read::Arrived { max: 1024 }, room: 0 }));
                }
                http::Event::Body(Up::End) => {
                    self.serve(peer, env);
                }
                http::Event::Reply(Up::Room) => {
                    let end = (peer.offset + usize::try_from(env.limits.send).expect("send")).min(peer.reply.len());
                    let bytes = Box::from(&peer.reply[peer.offset..end]);
                    peer.offset = end;
                    peer.action(env, http::Request::Reply(Down::Send(bytes)));
                    if end == peer.reply.len() {
                        peer.action(env, http::Request::Reply(Down::Finish));
                    } else {
                        peer.action(
                            env,
                            http::Request::Reply(Down::Demand { read: Read::Nothing, room: env.limits.send }),
                        );
                    }
                }
                http::Event::Done(http::Reuse::Keep) => peer.action(env, http::Request::Next),
                http::Event::Done(http::Reuse::Close)
                | http::Event::Ended
                | http::Event::Failed(_)
                | http::Event::Closed
                | http::Event::Body(Up::Failed(_))
                | http::Event::Reply(Up::Failed(_)) => {}
                http::Event::Body(Up::Room) | http::Event::Reply(Up::Bytes(_) | Up::End) | http::Event::Refused(_) => {
                    panic!("HTTP adapter contract")
                }
            }
        }
    }
    fn serve(&mut self, peer: &mut Peer, env: &Env<http::Limits>) {
        let call = peer.head.take().expect("request head");
        let method = match call.method {
            skein_http::Method::Get => Method::Get,
            skein_http::Method::Post => Method::Post,
            skein_http::Method::Put => Method::Put,
            skein_http::Method::Patch => Method::Patch,
            skein_http::Method::Delete => Method::Delete,
            skein_http::Method::Head | skein_http::Method::Options => panic!("API method"),
        };
        let request = ApiRequest {
            validator: call.header(b"If-None-Match"),
            method,
            target: &call.target,
            authorization: call.header(b"Authorization"),
            version: call.header(b"X-GitHub-Api-Version"),
            agent: call.header(b"User-Agent").is_some(),
            body: &peer.body,
        };
        if call.target.as_ref() == b"/graphql" {
            assert_eq!(peer.body.as_slice(), br#"{"query":"query { viewer { login databaseId } }"}"#);
        }
        let mut outcome = self.service.request(&request, env.wall, ReplyTo::new(Token::new(77)));
        let response = loop {
            outcome = match outcome {
                Outcome::Reply(response) => break response,
                Outcome::Admit(admission) => self.service.admit(
                    admission,
                    &mut self.domain,
                    &Env { now: self.fake_env.now, wall: env.wall, limits: self.fake_env.limits },
                    ReplyTo::new(Token::new(77)),
                ),
                Outcome::Call(pending, event) => self.continue_call(pending, event),
            };
        };
        if call.target.ends_with(b"/access_tokens") && response.status == 201 {
            // Keep the fake's minted response as a private credential observation,
            // including the narrower git token that never authenticates an API call.
            let prefix = b"\"token\":\"";
            let start =
                response.body.windows(prefix.len()).position(|w| w == prefix).expect("minted token") + prefix.len();
            let rest = &response.body[start..];
            let end = rest.iter().position(|b| *b == b'"').expect("token end");
            self.secrets.push(Secret::new(Box::from(&rest[..end])));
            self.mints += 1;
        }
        peer.delayed = self.lose_creation && method == Method::Post && call.target.ends_with(b"/issues");
        self.lose_creation &= !peer.delayed;
        let mut headers = vec![skein_http::Header {
            name: Box::from(b"Content-Type".as_slice()),
            value: Box::from(b"application/json".as_slice()),
        }];
        if let Some(rate) = response.rate {
            let mut fields = skein_lib::List::with_capacity(6);
            skein_fake_forge_github::rates::headers(rate, response.status == 403, &mut fields);
            headers.extend(fields.into_boxed());
        }
        if let Some(etag) = response.etag {
            headers.push(skein_http::Header { name: Box::from(b"ETag".as_slice()), value: etag });
        }
        if let Some(link) = response.link {
            headers.push(skein_http::Header { name: Box::from(b"Link".as_slice()), value: link });
        }
        peer.reply = response.body.into_vec();
        peer.offset = 0;
        if !peer.delayed {
            peer.action(
                env,
                http::Request::Respond(http::Response {
                    status: response.status,
                    headers: headers.into_boxed_slice(),
                    body: http::Body::Length(u64::try_from(peer.reply.len()).expect("reply length")),
                    close: false,
                }),
            );
            if !peer.reply.is_empty() {
                peer.action(env, http::Request::Reply(Down::Demand { read: Read::Nothing, room: env.limits.send }));
            }
        }
    }
    fn continue_call(&mut self, pending: Pending, event: fake::Event) -> Outcome {
        let result = self.domain_call(event);
        self.service.finish(pending, result, ReplyTo::new(Token::new(77)), &self.domain)
    }
    fn domain_call(&mut self, event: fake::Event) -> Result<raw::Answer, raw::Error> {
        self.release_hooks();
        let mut out = Queue::uncapped();
        fake::step_admitted(&mut self.domain, &self.fake_env, event, &mut out);
        for _ in 0..64 {
            if !out.is_empty() {
                break;
            }
            fake::fire(&mut self.domain, &self.fake_env, &mut out);
        }
        let Some(fake::Request::Reply { result, .. }) = out.pop() else { panic!("zero-latency reply") };
        assert!(out.is_empty());
        self.domain.reclaim();
        result
    }
    pub fn direct(&mut self, op: raw::Op) -> Result<raw::Answer, raw::Error> {
        self.domain_call(fake::Event::Call {
            reply_to: ReplyTo::new(Token::new(77)),
            user: 2,
            repository: Box::from(fixture::PATH),
            op,
        })
    }
    fn up(&mut self, event: skein_io::Event) {
        self.component.up(&self.env, event, &mut self.above, &mut self.below);
    }
    fn io(&mut self, request: skein_io::Request) {
        match request {
            skein_io::Request::Connect { owner, .. } => {
                let cut =
                    if self.rng.chance(self.cut_chance) { Cut::At(self.rng.between(0, 2048)) } else { Cut::Never };
                let env = self.http_env();
                let mut peer = Peer {
                    pipe: Pipe::new(self.rng.next_u64(), cut),
                    server: http::Server::new(&env.limits),
                    above: Queue::with_capacity(64),
                    below: Queue::with_capacity(64),
                    head: None,
                    body: Vec::new(),
                    reply: Vec::new(),
                    offset: 0,
                    traces: Vec::new(),
                    entrance: 0,
                    sent: Vec::new(),
                    remaining: 0,
                    delayed: false,
                    received: Vec::new(),
                    receiving_body: 0,
                    delivered_heads: 0,
                };
                peer.action(&env, http::Request::Next);
                assert!(self.peers.insert(owner, peer).is_none());
                self.up(skein_io::Event::Connecting { owner, socket: owner });
                self.up(skein_io::Event::Connected { owner });
            }
            skein_io::Request::Stream { stream, down } => {
                if let Some(peer) = self.peers.get_mut(&stream) {
                    if let Down::Send(bytes) = &down {
                        peer.sent(bytes, &mut self.secrets);
                    }
                    peer.pipe.client_down(down);
                }
            }
            skein_io::Request::Abort { entity } | skein_io::Request::Close { entity } => self.end(entity),
            other @ (skein_io::Request::Listen { .. }
            | skein_io::Request::Bind { .. }
            | skein_io::Request::Reject { .. }
            | skein_io::Request::Output { .. }
            | skein_io::Request::Spawn { .. }
            | skein_io::Request::Signal { .. }
            | skein_io::Request::Usage { .. }
            | skein_io::Request::Window { .. }) => panic!("unexpected GitHub io {other:?}"),
        }
    }
    fn end(&mut self, owner: Token) {
        if let Some(peer) = self.peers.remove(&owner) {
            let counts = peer.pipe.request_counts();
            assert_eq!(counts.written, u64::try_from(peer.traces.len()).expect("trace count"));
            self.lines.extend_from_slice(peer.pipe.request_lines());
            for (index, trace) in peer.traces.into_iter().enumerate() {
                if !trace.own {
                    self.totals.written += 1;
                    self.totals.served += u64::from(trace.served);
                    self.totals.cut_in_head += u64::from(index >= peer.delivered_heads);
                }
            }
            self.up(skein_io::Event::Closed { owner });
        }
    }
    pub fn accounting(&self) -> Accounting {
        let mut a = Accounting {
            reported: 0,
            written: self.totals.written,
            served: self.totals.served,
            cut_in_head: self.totals.cut_in_head,
        };
        for peer in self.peers.values() {
            for trace in &peer.traces {
                if !trace.own {
                    a.written += 1;
                    a.served += u64::from(trace.served);
                }
            }
        }
        a
    }
    pub fn lines(&self) -> &[Vec<u8>] {
        &self.lines
    }
    pub fn lose_creation(&mut self) {
        self.lose_creation = true;
    }
    pub fn replace(&mut self, component: Component) {
        let owners: Vec<_> = self.peers.keys().copied().collect();
        for owner in owners {
            self.end(owner);
        }
        while let Some(event) = self.above.pop() {
            self.events.push(format!("{event:?}"));
        }
        while self.below.pop().is_some() {}
        self.component = component;
        self.component.begin(&self.env, &mut self.above, &mut self.below);
        self.action(Request::Installations { forge: 1, accepted: Box::from([1]) });
        self.action(Request::Connect { forge: 1, access: fixture::access(self.key) });
    }
    pub fn rotate(&mut self, key: &'static [u8]) {
        self.key = key;
        self.service = fixture::service(self.seed + 1, key);
        self.action(Request::Connect { forge: 1, access: fixture::access(key) });
    }
    pub fn call(&mut self, call: u64, repository: skein_forge::api::Repository, op: skein_forge::api::Op) {
        assert_eq!(repository, REPO);
        self.action(Request::Call { call: Token::new(call), repository, op });
    }
}
