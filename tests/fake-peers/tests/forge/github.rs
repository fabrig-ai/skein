//! Both dialects over loopback byte streams on the same replaying kernel.
use super::fixtures as f;
use skein_fake_forge_domain::{self as domain, api as da};
use skein_fake_forge_github::{
    self as fake,
    service::{Account, AppFacts, Row},
};
use skein_fake_peers::forge::{Forge, Observation, Peer};
use skein_fake_peers_tests as support;
use skein_forge::{
    api,
    github::{
        self,
        document::{Document, Kind},
        plans::{
            Body, Heard,
            plan::{Bounds, Plan, Step},
            tables::Tables,
        },
        request::Method,
    },
};
use skein_io::kernel::{Complete, Done, Family, Fd, Op, Submit};
use skein_json::tokenizer;
use skein_lib::{
    Env, Intake, List, Queue, ReplyTo, Secret, Time, Token, Wall, bytes,
    stream::{Down, Up},
};
use skein_shell::Host;
use skein_sim::{Pid, Sim};
use std::{cell::RefCell, rc::Rc};

const BOT: u64 = 9;
const APP: u64 = 7;
const PERSON: u64 = 31;
const OWNER: u64 = 22;
const SLUG: &[u8] = b"example-app";
const ISSUE: &[u8] = br#"{"title":"A shared issue","body":"A shared body"}"#;
const MANIFEST: &[u8] = br#"{"name":"example-app","url":"http://127.0.0.1/","redirect_url":"http://127.0.0.1/callback","setup_url":"http://127.0.0.1/setup","public":true}"#;
// One bootstrap plus two write/readback sequences fit 128 records and 128 KiB.
const RECORDS: u32 = 128;
const RECORD_BYTES: u32 = 131_072;
const PASSES: u32 = 10_000;
const BODY_BYTES: u32 = 4096;
const RESPONSE_BYTES: u32 = 8192;

pub(super) fn limits() -> fake::Limits {
    fake::Limits {
        deliveries: fake::delivery::Limits { entries: 64, payload_bytes: 32768 },
        http: skein_http::server::Limits {
            head: 4096,
            headers: 16,
            body: u64::from(BODY_BYTES),
            read: 128,
            response: 16384,
            send: 128,
        },
        github: github::limits::Limits {
            json: tokenizer::Limits { depth: 16, string: 4096, number: 32, chunk: 128, length: 16384 },
            writer: skein_json::writer::Limits { depth: 16, length: RESPONSE_BYTES },
            string_bytes: 256,
            body_bytes: BODY_BYTES,
            // The App's wanted grants are its longest list.
            inner: u32::try_from(skein_forge::github::setup::PERMISSIONS.len()).expect("wanted count"),
            target_bytes: 1024,
        },
        installations: fake::installations::Limits { installations: 3, tokens: 4, listed: 2 },
        pages: fake::pages::Limits { states: 3, manifest_bytes: 2048 },
        page: 1,
        body: BODY_BYTES,
        response: RESPONSE_BYTES,
        blob_records: 16,
        git_records: 16,
    }
}
pub(super) fn config() -> fake::Config {
    let mut repositories = List::with_capacity(1);
    repositories
        .push(Row {
            id: 2,
            owner: f::boxed(b"org"),
            owner_id: OWNER,
            organization: true,
            name: f::boxed(b"repo"),
            private: true,
        })
        .expect("repository row fits");
    let mut accounts = List::with_capacity(3);
    for (login, id, organization) in [(b"org".as_slice(), OWNER, true), (b"alice", PERSON, false), (b"bob", 32, false)]
    {
        accounts.push(Account { login: f::boxed(login), id, organization }).expect("account row fits");
    }
    fake::Config {
        rate_quota: 5000,
        rate_window: skein_lib::Duration::from_secs(3600),
        token_life: skein_fake_forge_github::installations::TOKEN_LIFE,
        deliveries: fake::delivery::Config::standard(),
        base: f::boxed(b"http://127.0.0.1:1"),
        blob: f::boxed(b"http://logs.example.test"),
        third_origin: None,
        blob_redirect: None,
        app: fake::app::App {
            id: APP,
            slug: f::boxed(SLUG),
            owner: PERSON,
            permissions: fake::app::wanted(),
            events: Box::new([]),
            key: fake::keys::KEYS[0],
            webhook: Secret::new(f::boxed(b"invented-hook-secret")),
        },
        app_facts: AppFacts::Complete,
        shown: fake::service::Shown::Ruleset,
        ruleset_id: 17,
        pull_requests_only: false,
        required_signatures: false,
        tokens: List::with_capacity(0),
        faults: skein_fake_forge_github::Faults { reset: 0, drop: 0, server_error: 0 },
        bot: BOT,
        hook: f::boxed(b"http://127.0.0.1/hooks"),
        repositories,
        accounts,
        person: f::boxed(b"alice"),
    }
}
struct Reply {
    status: u16,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    body: Vec<u8>,
}
impl Reply {
    fn parse(wire: &[u8]) -> Self {
        let split = bytes::find(wire, b"\r\n\r\n").expect("response head");
        let mut lines = wire[..split].split(|b| *b == b'\n');
        let first = lines.next().expect("response status line");
        let status = std::str::from_utf8(&first[9..12]).expect("ASCII status").parse().expect("numeric status");
        let headers = lines
            .map(|line| {
                let at = line.iter().position(|b| *b == b':').expect("header field");
                (line[..at].to_vec(), line[at + 2..line.len() - 1].to_vec())
            })
            .collect();
        Self { status, headers, body: wire[split + 4..].to_vec() }
    }
    fn header(&self, name: &[u8]) -> Option<&[u8]> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_slice())
    }
    fn decode(&self, kind: Kind) -> (Vec<github::document::Row>, Document) {
        let mut bounds = limits().github;
        // Conversion readers admit the complete PEM within the JSON scalar cap.
        if kind == Kind::Conversion {
            bounds.string_bytes = bounds.json.string;
        }
        self.decode_with(github::decode::decoder(kind, &bounds), &bounds)
    }
    fn decode_with(
        &self,
        mut decoder: github::decode::Decoder,
        bounds: &github::limits::Limits,
    ) -> (Vec<github::document::Row>, Document) {
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: bounds.json };
        let mut tokenizer = tokenizer::Tokenizer::new(&bounds.json);
        let mut above = Queue::with_capacity(1);
        let mut below = Queue::with_capacity(1);
        let mut intake = Intake::with_capacity(RESPONSE_BYTES);
        intake.append(&self.body).expect("response fits intake");
        let mut rows = Vec::new();
        tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
        for _ in 0..self.body.len().saturating_mul(8).saturating_add(64) {
            if let Some(event) = above.pop() {
                let (request, step) = decoder.step(event);
                match step {
                    github::decode::Step::Demand => {}
                    github::decode::Step::Row(row) => rows.push(row),
                    github::decode::Step::Done(doc) => return (rows, doc),
                    github::decode::Step::Malformed(_) => panic!("response document must decode"),
                }
                tokenizer::down(&mut tokenizer, &env, request, &mut above, &mut below);
            } else {
                let Down::Demand { read, .. } = below.pop().expect("JSON input demand") else {
                    panic!("JSON reads only")
                };
                let up = intake.meet(read).map_or(Up::End, Up::Bytes);
                tokenizer::up(&mut tokenizer, &env, up, &mut above, &mut below);
            }
        }
        panic!("response decoding settles")
    }
}
struct World {
    github: Peer,
    forgejo: Peer,
    forge: Rc<RefCell<Forge>>,
    sim: Sim,
    github_pid: Pid,
    forgejo_pid: Pid,
    client_pid: Pid,
    serial: u64,
    passes: u32,
}
impl World {
    fn new(seed: u64) -> Self {
        Self::with_hooks(seed, config(), false)
    }
    fn configured(seed: u64, config: fake::Config) -> Self {
        Self::with_hooks(seed, config, true)
    }
    fn with_hooks(seed: u64, config: fake::Config, hooked: bool) -> Self {
        Self::configured_checks(
            seed,
            config,
            hooked,
            da::Checks {
                cues: Box::new([]),
                contexts: Box::new([]),
                latency_min: skein_lib::Duration::ZERO,
                latency_max: skein_lib::Duration::ZERO,
                silent: 0,
                passes: 0,
                reruns: 0,
                cue: None,
            },
        )
    }
    fn with_checks(seed: u64, checks: da::Checks) -> Self {
        Self::configured_checks(seed, config(), false, checks)
    }
    fn configured_checks(seed: u64, config: fake::Config, hooked: bool, checks: da::Checks) -> Self {
        let forge = f::configured_checks_forge(seed, f::fake_config(), hooked, checks);
        domain::grant(&mut forge.borrow_mut().domain, f::REPOSITORY, BOT, da::Permission::Write);
        let peer_limits = skein_fake_peers::Limits {
            io: skein_io::Limits { sockets: 6, ..f::limits().io },
            observations: RECORDS,
            observation_bytes: RECORD_BYTES,
            ..f::limits()
        };
        let github = Peer::github(f::address(), f::address(), peer_limits, config, limits(), Rc::clone(&forge), seed)
            .expect("bounded GitHub peer");
        let forgejo = Peer::new(f::address(), peer_limits, f::config(), f::protocol_limits(), Rc::clone(&forge), seed)
            .expect("bounded Forgejo peer");
        let mut sim = Sim::new(seed, skein_sim::Config::calm());
        let (github_pid, forgejo_pid, client_pid) = (sim.spawn_process(), sim.spawn_process(), sim.spawn_process());
        let mut world = Self { github, forgejo, forge, sim, github_pid, forgejo_pid, client_pid, serial: 0, passes: 0 };
        while world.github.address().is_none()
            || world.github.blob_address().is_none()
            || world.forgejo.address().is_none()
        {
            world.pass();
        }
        world
    }
    fn pass(&mut self) {
        self.passes += 1;
        assert!(self.passes <= PASSES, "GitHub world settles within its pass bound");
        for (pid, peer) in [(self.github_pid, &mut self.github), (self.forgejo_pid, &mut self.forgejo)] {
            self.sim.reap(pid, peer.completions());
            peer.iterate(self.sim.now(), self.sim.wall());
            self.sim.submit(pid, peer.submissions());
        }
        if !self.github.work_pending(self.sim.now())
            && !self.forgejo.work_pending(self.sim.now())
            && let Some(next) = [self.sim.next_due(), self.github.next_deadline(), self.forgejo.next_deadline()]
                .into_iter()
                .flatten()
                .min()
        {
            self.sim.advance_to(next);
        }
    }
    fn client(&mut self, kind: Op) -> Complete {
        self.serial += 1;
        let token = Token::new(self.serial);
        let mut output = Queue::with_capacity(1);
        let mut input = Queue::with_capacity(1);
        output.push(Submit { op: token, kind });
        self.sim.submit(self.client_pid, &mut output);
        loop {
            self.sim.reap(self.client_pid, &mut input);
            if let Some(complete) = input.pop() {
                return complete;
            }
            self.pass();
        }
    }
    fn open(&mut self, github: bool) -> Fd {
        let Done::Fd(fd) = support::call(&mut self.sim, self.client_pid, 1, Op::Socket { family: Family::Ipv4 }) else {
            panic!("client socket")
        };
        let addr = if github { self.github.address() } else { self.forgejo.address() }.expect("bound peer address");
        support::call(&mut self.sim, self.client_pid, 2, Op::Connect { fd, addr });
        fd
    }
    fn send(&mut self, fd: Fd, wire: &[u8]) {
        let mut from = 0;
        while usize::try_from(from).expect("send offset fits usize") < wire.len() {
            let complete = self.client(Op::Send { fd, bytes: f::boxed(wire), from });
            let Ok(Done::Count(sent)) = complete.result else { panic!("client sends") };
            assert!(sent > 0);
            from += sent;
        }
    }
    fn receive(&mut self, fd: Fd) -> Reply {
        let mut wire = Vec::new();
        loop {
            let complete = self.client(Op::Recv { fd, buf: Box::new([0; 1024]) });
            let Ok(Done::Count(count)) = complete.result else { panic!("client receives") };
            if count == 0 {
                break;
            }
            let Op::Recv { buf, .. } = complete.kind else { panic!("receive buffer") };
            wire.extend_from_slice(&buf[..usize::try_from(count).expect("received count fits usize")]);
        }
        support::call(&mut self.sim, self.client_pid, 3, Op::Close { fd });
        Reply::parse(&wire)
    }
    fn fetch(&mut self, method: Method, target: &[u8], body: &[u8], auth: Option<&Secret>) -> Reply {
        let fd = self.open(true);
        self.send(fd, &request(method, target, body, auth));
        self.receive(fd)
    }
    fn fetch_blob(&mut self, target: &[u8], range: bool) -> Reply {
        let Done::Fd(fd) = support::call(&mut self.sim, self.client_pid, 1, Op::Socket { family: Family::Ipv4 }) else {
            panic!("blob client socket")
        };
        let address = self.github.blob_address().expect("bound blob address");
        support::call(&mut self.sim, self.client_pid, 2, Op::Connect { fd, addr: address });
        let mut wire = b"GET ".to_vec();
        wire.extend_from_slice(target);
        wire.extend_from_slice(b" HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        if range {
            wire.extend_from_slice(b"Range: bytes=0-0\r\n");
        }
        wire.extend_from_slice(b"Connection: close\r\n\r\n");
        self.send(fd, &wire);
        self.receive(fd)
    }
    fn manifest(&mut self) -> Reply {
        let mut form = b"manifest=".to_vec();
        for byte in MANIFEST {
            form.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
        self.fetch(Method::Post, b"/settings/apps/new?state=example-state", &form, None)
    }
    fn convert(&mut self) -> Reply {
        let page = self.manifest();
        let at = bytes::find(&page.body, b"?code=").expect("manifest create link") + b"?code=".len();
        let end = page.body[at..].iter().position(|b| *b == b'&').expect("conversion code delimiter") + at;
        let mut target = b"/app-manifests/".to_vec();
        target.extend_from_slice(&page.body[at..end]);
        target.extend_from_slice(b"/conversions");
        self.fetch(Method::Post, &target, b"", None)
    }
    fn jwt(&self) -> Secret {
        let now = self.sim.wall();
        let expires = Wall::from_nanos(now.as_nanos().checked_add(600_000_000_000).expect("JWT expiration fits clock"));
        let input = github::jwt::signing_input(8, now, expires).expect("JWT signing input");
        let signature = skein_tls::sign_rs256(fake::keys::KEYS[0], &input).expect("fixture key signs JWT");
        let token = github::jwt::token(&input, &signature).expect("JWT encoding");
        bearer(token.expose())
    }
    fn install(&mut self, account: &[u8]) -> Reply {
        let mut target = b"/apps/example-app/installations/new?state=install-state&account=".to_vec();
        target.extend_from_slice(account);
        self.fetch(Method::Get, &target, b"", None)
    }
    fn token(&mut self) -> Secret {
        self.convert();
        self.install(b"org");
        let response = self.fetch(Method::Post, b"/app/installations/1/access_tokens", b"", Some(&self.jwt()));
        let (_, Document::Minted(minted)) = response.decode(Kind::Minted) else { panic!("minted installation token") };
        bearer(minted.token.expose())
    }
    fn plan(&mut self, token: &Secret, op: api::Op) -> Result<api::Answer, api::Error> {
        let bounds = Bounds { inner: 8, page_size: 1, rows: 8, answer_bytes: RESPONSE_BYTES, body_bytes: BODY_BYTES };
        let mut plan = Plan::new(api::Repository { forge: 1, repository: 2 }, op);
        let mut tables = Tables::new(4, 8);
        let mut step = plan.begin(bounds, SLUG, &mut tables).expect("plan begins");
        for _ in 0..32 {
            let events = match step {
                Step::Done(result) => return result,
                Step::Send(exchange) => {
                    let encoded =
                        github::request::encode(&exchange.request, &limits().github).expect("bounded request encoding");
                    let response = self.fetch(
                        encoded.method,
                        &encoded.target,
                        encoded.body.as_deref().unwrap_or(b""),
                        Some(token),
                    );
                    let mut head = github::header::Head::new();
                    for (name, value) in &response.headers {
                        github::header::field(&mut head, name, value, &limits().github);
                    }
                    let mut events = vec![Heard::Head { status: response.status, head }];
                    if response.status != 302 {
                        let kind = if response.status < 300 {
                            github::decode::kind(&exchange.request).expect("request document kind")
                        } else {
                            Kind::Error
                        };
                        let (rows, document) = response.decode(kind);
                        events.extend(rows.into_iter().map(Heard::Row));
                        events.push(Heard::Done(Body::Document(document)));
                    }
                    events
                }
                Step::Redirect { redirect, retention } => {
                    let address = self.github.blob_address().expect("bound blob origin");
                    assert_eq!(redirect.origin.port, address.port());
                    assert!(!redirect.origin.tls);
                    assert_eq!(redirect.origin.host.as_ref(), address.ip().to_string().as_bytes());
                    let response = self.fetch_blob(&redirect.target, false);
                    assert_eq!(response.status, 200);
                    let github::plans::plan::Retention::Tail { bytes } = retention else {
                        panic!("job log retains tail")
                    };
                    let cap = usize::try_from(bytes).expect("u32 fits usize");
                    let start = response.body.len().saturating_sub(cap);
                    vec![
                        Heard::Head { status: response.status, head: github::header::Head::new() },
                        Heard::Done(Body::Raw { bytes: f::boxed(&response.body[start..]), more: start != 0 }),
                    ]
                }
            };
            let mut next = None;
            for event in events {
                if let Some(advance) = plan.heard(event, bounds, SLUG, &mut tables).expect("plan accepts response") {
                    next = Some(advance);
                    break;
                }
            }
            step = next.expect("exchange advances plan");
        }
        panic!("bounded plan settles")
    }
    fn settle(&mut self) {
        self.github.shutdown();
        self.forgejo.shutdown();
        while !self.github.is_empty() || !self.forgejo.is_empty() {
            self.pass();
        }
        for pid in [self.github_pid, self.forgejo_pid, self.client_pid] {
            self.sim.assert_quiescent(pid);
            self.sim.assert_no_open_fds(pid);
        }
    }
}
fn bearer(token: &[u8]) -> Secret {
    let mut value = b"Bearer ".to_vec();
    value.extend_from_slice(token);
    Secret::new(value.into_boxed_slice())
}
fn request(method: Method, target: &[u8], body: &[u8], auth: Option<&Secret>) -> Vec<u8> {
    request_reuse(method, target, body, auth, false)
}
fn request_reuse(method: Method, target: &[u8], body: &[u8], auth: Option<&Secret>, keep: bool) -> Vec<u8> {
    let method = match method {
        Method::Get => b"GET".as_slice(),
        Method::Post => b"POST",
        Method::Put => b"PUT",
        Method::Patch => b"PATCH",
        Method::Delete => b"DELETE",
    };
    let mut wire = method.to_vec();
    wire.push(b' ');
    wire.extend_from_slice(target);
    wire.extend_from_slice(b" HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: example-app\r\nX-GitHub-Api-Version: ");
    wire.extend_from_slice(github::pin::API_VERSION.as_bytes());
    if let Some(auth) = auth {
        wire.extend_from_slice(b"\r\nAuthorization: ");
        wire.extend_from_slice(auth.expose());
    }
    let connection = if keep { "keep-alive" } else { "close" };
    wire.extend_from_slice(
        format!("\r\nConnection: {connection}\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
    );
    wire.extend_from_slice(body);
    wire
}

#[test]
fn github_peer_head_is_a_bodyless_404_and_allows_reuse() {
    let mut world = World::new(f::SEED);
    let fd = world.open(true);
    world.send(fd, b"HEAD /app HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n");
    world.send(fd, &request(Method::Get, b"/unknown", b"", None));
    let replies = world.receive(fd);
    assert_eq!(replies.status, 404);
    assert!(replies.body.starts_with(b"HTTP/1.1 404 "), "the next response starts immediately after HEAD's head");
    assert_eq!(Reply::parse(&replies.body).status, 404);
    world.settle();
}

#[test]
fn github_peer_page_routes_choose_between_the_browser_and_api() {
    let mut world = World::new(f::SEED);
    let fd = world.open(true);
    world.send(fd, b"GET /apps/example-app/installations/new HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert_eq!(world.receive(fd).status, 422, "the page needs state, with no API headers required");
    for target in
        [b"/apps/example-app/api-only".as_slice(), b"/settings/api-only", b"/organizations/example-org/api-only"]
    {
        let fd = world.open(true);
        let mut wire = b"GET ".to_vec();
        wire.extend_from_slice(target);
        wire.extend_from_slice(b" HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        world.send(fd, &wire);
        assert_eq!(world.receive(fd).status, 403, "the API requires its User-Agent");
        let response = world.fetch(Method::Get, target, b"", None);
        assert_eq!(response.status, 404, "the API cannot route this target");
        assert!(bytes::find(&response.body, b"Not Found").is_some());
    }
    world.settle();
}

#[test]
fn github_peer_serves_repository_reads_and_writes_over_the_shared_forge() {
    let mut world = World::new(f::SEED);
    let token = world.token();
    let made = world.fetch(Method::Post, b"/repos/org/repo/issues", ISSUE, Some(&token));
    assert_eq!(made.status, 201);
    let read = world.plan(&token, api::Op::Read(api::Read::Item { number: 1, after: 0 })).expect("issue read");
    let api::Answer::Item { item, .. } = read else { panic!("issue read") };
    assert_eq!(item.title.as_ref(), b"A shared issue");
    let fd = world.open(false);
    world.send(fd, &f::request(b"/api/v1/repos/org/repo/issues/1"));
    let response = world.receive(fd);
    assert_eq!(response.status, 200);
    assert!(bytes::find(&response.body, b"A shared issue").is_some());
    assert!(world.forge.borrow().pending.is_empty());
    assert!(world.forge.borrow().now > Time::ZERO);
    for peer in [&world.github, &world.forgejo] {
        assert!(peer.observations().iter().all(|record| match record {
            Observation::Head { call, .. } => call.header(b"authorization").is_none_or(<[u8]>::is_empty),
            Observation::Connection { .. } | Observation::Call { .. } | Observation::Delivery { .. } => true,
        }));
    }
    world.settle();
}
#[test]
fn github_peer_pages_link_to_its_own_origin() {
    let mut world = World::new(f::SEED);
    let token = world.token();
    for _ in 0..2 {
        world.fetch(Method::Post, b"/repos/org/repo/issues", ISSUE, Some(&token));
    }
    let response = world.fetch(
        Method::Get,
        b"/repos/org/repo/issues?state=all&sort=updated&direction=asc&per_page=1&page=1",
        b"",
        Some(&token),
    );
    assert_eq!(response.status, 200);
    let link = response.header(b"Link").expect("next page");
    let expected = format!(
        "<http://{}/repos/org/repo/issues?state=all&sort=updated&direction=asc&per_page=1&page=2>; rel=\"next\"",
        world.github.address().expect("bound GitHub address")
    );
    assert_eq!(link, expected.as_bytes());
    let target = link
        .strip_prefix(format!("<http://{}", world.github.address().expect("bound GitHub address")).as_bytes())
        .expect("link uses peer origin")
        .split(|b| *b == b'>')
        .next()
        .expect("next page target");
    let next = world.fetch(Method::Get, target, b"", Some(&token));
    assert_eq!(next.status, 200);
    assert!(next.header(b"Link").is_none());
    world.settle();
}
#[test]
fn github_peer_serves_the_manifest_and_conversion_pages() {
    let mut world = World::new(f::SEED);
    let page = world.manifest();
    assert_eq!(page.status, 200);
    assert_eq!(page.header(b"content-type"), Some(b"text/html".as_slice()));
    assert!(bytes::find(&page.body, b"Create GitHub App").is_some());
    let conversion = world.convert();
    assert_eq!(conversion.status, 201);
    assert_eq!(conversion.header(b"content-type"), Some(b"application/json".as_slice()));
    let (_, Document::Conversion(app)) = conversion.decode(Kind::Conversion) else { panic!("converted App") };
    assert_eq!(app.slug.as_ref(), SLUG);
    let installed = world.install(b"org");
    assert_eq!(installed.status, 302);
    assert_eq!(
        installed.header(b"Location"),
        Some(b"http://127.0.0.1/setup?installation_id=1&setup_action=install&state=install-state".as_slice())
    );
    world.settle();
}
#[test]
fn github_peer_shutdown_retires_unlisted_log_work_while_io_drains() {
    let mut world = World::new(f::SEED);
    let wall = world.sim.wall();
    world
        .github
        .github_layer_mut()
        .expect("GitHub layer")
        .release(github::delivery::Delivery::Other, None, wall)
        .expect("bounded delivery log row");
    assert!(world.github.next_policy_deadline().is_some(), "unlisted row arms visibility");
    world.github.shutdown();
    assert_eq!(world.github.next_policy_deadline(), None, "shutdown retires visibility at its owner");
    world.pass();
    assert!(!world.github.is_empty(), "listener close still waits for its IO terminal");
    world.settle();
    assert!(!world.github.work_pending(world.sim.now()));
    assert_eq!(world.github.next_deadline(), None);
    assert_eq!(world.github.next_policy_deadline(), None);
}

#[test]
fn github_peer_shutdown_settles_a_mid_request_client() {
    let mut world = World::new(f::SEED);
    let token = world.token();
    let fd = world.open(true);
    let wire = request(Method::Post, b"/repos/org/repo/issues", ISSUE, Some(&token));
    let split = wire.len() - ISSUE.len() + ISSUE.len() / 2;
    world.send(fd, &wire[..split]);
    while !world.github.observations().iter().any(|record| {
        matches!(record,
        Observation::Head { call, .. } if call.target.as_ref() == b"/repos/org/repo/issues")
    }) {
        world.pass();
    }
    world.github.shutdown();
    world.pass();
    assert!(!world.github.is_empty());
    world.send(fd, &wire[split..]);
    let response = world.receive(fd);
    assert_eq!(response.status, 201);
    let calls = world.github.observations().iter().filter(|record| matches!(record, Observation::Call { .. })).count();
    assert_eq!(calls, f::ONE_REPLY);
    world.settle();
}
#[test]
fn github_peer_seed_replays_names_and_replies() {
    fn run() -> (World, Vec<u8>) {
        let mut world = World::new(f::SEED);
        let token = world.token();
        let response = world.fetch(Method::Post, b"/repos/org/repo/issues", ISSUE, Some(&token));
        world.settle();
        (world, response.body)
    }
    let (first, first_reply) = run();
    let (second, second_reply) = run();
    assert!(first_reply == second_reply, "reply bytes replay");
    assert!(first.github.observations() == second.github.observations(), "GitHub observations replay");
    assert!(first.forgejo.observations() == second.forgejo.observations(), "Forgejo observations replay");
    assert!(first.sim.render_trace() == second.sim.render_trace(), "byte trace replays");
}

#[test]
fn github_and_forgejo_route_concurrent_calls_to_their_owners() {
    let mut world = World::new(f::SEED);
    let token = world.token();
    let external = world.forge.borrow_mut().next_reply();
    world
        .forge
        .borrow_mut()
        .pending
        .push(domain::Request::Reply { to: ReplyTo::new(external), result: Ok(da::Answer::Done) });
    let github = world.open(true);
    let forgejo = world.open(false);
    world.send(github, &request(Method::Get, b"/repos/org/repo", b"", Some(&token)));
    world.send(forgejo, &f::request(b"/api/v1/repos/org/repo/branches/main"));
    let github = world.receive(github);
    let forgejo = world.receive(forgejo);
    assert_eq!((github.status, forgejo.status), (200, 200));
    assert!(matches!(github.decode(Kind::Repository).1, Document::Repository(_)));
    assert!(bytes::find(&forgejo.body, b"\"name\":\"main\"").is_some());
    let domain::Request::Reply { to, result } =
        world.forge.borrow_mut().pending.pop().expect("external reply retained")
    else {
        panic!("external terminal")
    };
    assert_eq!(to.into_token(), external);
    assert_eq!(result, Ok(da::Answer::Done));
    world.settle();
}

#[test]
fn github_peer_reuses_a_connection_after_its_response() {
    let mut world = World::new(f::SEED);
    world.convert();
    for (target, status) in [
        (b"/unknown".as_slice(), 404),
        (b"/apps/example-app/installations/new?state=reuse-state&account=org".as_slice(), 302),
    ] {
        let before = world.github.observations().len();
        let fd = world.open(true);
        let mut wire = request_reuse(Method::Get, target, b"", None, true);
        wire.extend_from_slice(&request(Method::Get, target, b"", None));
        world.send(fd, &wire);
        let response = world.receive(fd);
        assert_eq!(response.status, status);
        assert_eq!(bytes::count(&response.body, format!("HTTP/1.1 {status} ").as_bytes(), 2), 1);
        let heads = world.github.observations()[before..]
            .iter()
            .filter(|record| matches!(record, Observation::Head { .. }))
            .count();
        assert_eq!(heads, 2);
    }
    world.settle();
}

#[test]
fn github_peer_keeps_a_continuation_until_the_followup_reply() {
    let mut world = World::new(f::SEED);
    let token = world.token();
    world.fetch(Method::Post, b"/repos/org/repo/issues", ISSUE, Some(&token));
    let before = world.github.observations().len();
    // The author read succeeds; the domain then refuses reviewing an issue.
    let response = world.fetch(
        Method::Post,
        b"/repos/org/repo/pulls/1/reviews",
        br#"{"event":"COMMENT","body":"A review"}"#,
        Some(&token),
    );
    assert_eq!(response.status, 404);
    let calls: Vec<_> =
        world.github.observations()[before..]
            .iter()
            .filter_map(|record| {
                if let Observation::Call { connection, op, .. } = record { Some((connection, op)) } else { None }
            })
            .collect();
    assert_eq!(calls.len(), f::WRITE_AND_READBACK);
    assert!(matches!(calls[0].1, da::Op::Read(da::Read::Item { number: 1, .. })));
    assert!(matches!(calls[1].1, da::Op::Write(da::Write::Review { number: 1, .. })));
    assert_eq!(calls[0].0, calls[1].0);
    world.settle();
}

fn commit_id(number: u64) -> api::Commit {
    skein_fake_forge_protocol::routes::object(number).bytes
}
fn failed_job() -> (World, Secret, api::JobAttempt) {
    let mut world = World::with_checks(
        f::SEED,
        da::Checks {
            cues: Box::new([]),
            contexts: Box::new([f::boxed(b"ci")]),
            latency_min: skein_lib::Duration::ZERO,
            latency_max: skein_lib::Duration::ZERO,
            silent: 0,
            passes: 0,
            reruns: 0,
            cue: None,
        },
    );
    let token = world.token();
    world
        .plan(&token, api::Op::Write(api::Write::CreateBranch { branch: f::boxed(b"feature"), commit: commit_id(1) }))
        .expect("branch starts CI");
    let api::Answer::Checks(checks) =
        world.plan(&token, api::Op::Read(api::Read::Checks { commit: commit_id(1) })).expect("checks")
    else {
        panic!("checks answer")
    };
    (world, token, checks[0].job.expect("failed CI attempt"))
}
#[test]
fn github_peer_runs_the_dialects_job_log_redirect_plan_through_the_blob_face() {
    let (mut world, token, attempt) = failed_job();
    let expected =
        b"set up ci\nfailing check ci 0\nfailing check ci 1\nfailing check ci 2\nfailing check ci 3\njob ci failed\n";
    for max_bytes in [4096, 8] {
        let api::Answer::Job { log, truncated, .. } =
            world.plan(&token, api::Op::Read(api::Read::Job { attempt, max_bytes })).expect("job plan follows one 302")
        else {
            panic!("job answer")
        };
        let cap = usize::try_from(max_bytes).expect("test cap");
        assert_eq!(log.as_ref(), &expected[expected.len().saturating_sub(cap)..]);
        assert_eq!(truncated, cap < expected.len());
        let record = world.github.blob_records().expect("GitHub records").iter().last().expect("blob request");
        assert!(!record.authorization);
        assert!(!record.range);
    }
    world.settle();
}

#[test]
fn github_peer_charges_the_log_redirect_once_and_leaves_blob_fetches_free() {
    fn used(reply: &Reply) -> u32 {
        std::str::from_utf8(reply.header(b"X-RateLimit-Used").expect("rate usage"))
            .expect("decimal ASCII")
            .parse()
            .expect("rate count")
    }
    let (mut world, token, attempt) = failed_job();
    let before = world.fetch(Method::Get, b"/repos/org/repo", b"", Some(&token));
    assert_eq!(before.status, 200);
    let target = format!("/repos/org/repo/actions/jobs/{}/logs", attempt.job);
    let redirect = world.fetch(Method::Get, target.as_bytes(), b"", Some(&token));
    assert_eq!(redirect.status, 302);
    assert_eq!(used(&redirect), used(&before) + 1, "one admission for the log redirect");
    let location = github::redirect::parse(redirect.header(b"Location").expect("log location"), 1024, 1024)
        .expect("signed blob redirect");
    let blob = world.fetch_blob(&location.target, true);
    assert_eq!(blob.status, 206);
    assert_eq!(blob.body.len(), 1);
    assert!(blob.header(b"Content-Range").expect("partial content range").starts_with(b"bytes 0-0/"));
    assert!(blob.header(b"X-RateLimit-Used").is_none());
    let record = world.github.blob_records().expect("GitHub records").iter().last().expect("blob request");
    assert!(!record.authorization);
    let after = world.fetch(Method::Get, b"/repos/org/repo", b"", Some(&token));
    assert_eq!(after.status, 200);
    assert_eq!(used(&after), used(&redirect) + 1, "blob fetch consumes no token quota");
    world.settle();
}

impl World {
    fn recipient(&mut self, listener: Fd) -> Vec<u8> {
        let Ok(Done::Accepted { fd, .. }) = self.client(Op::Accept { fd: listener }).result else {
            panic!("recipient accepts")
        };
        let mut wire = Vec::new();
        loop {
            let complete = self.client(Op::Recv { fd, buf: Box::new([0; 1024]) });
            let Ok(Done::Count(count)) = complete.result else { panic!("recipient reads") };
            assert!(count > 0, "POST ends after its declared body");
            let Op::Recv { buf, .. } = complete.kind else { panic!("receive buffer") };
            wire.extend_from_slice(&buf[..usize::try_from(count).expect("read count")]);
            if let Some(split) = bytes::find(&wire, b"\r\n\r\n") {
                let header = &wire[..split];
                let at = bytes::find(header, b"Content-Length: ").expect("body length") + 16;
                let end = bytes::find(&header[at..], b"\r\n").map_or(split, |len| at + len);
                let length: usize =
                    std::str::from_utf8(&header[at..end]).expect("ASCII length").parse().expect("numeric length");
                if wire.len() == split + 4 + length {
                    break;
                }
            }
        }
        self.send(fd, b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        support::call(&mut self.sim, self.client_pid, 3, Op::Close { fd });
        wire
    }
    fn log_events<T>(&mut self, step: github::plans::start::Step<T>, auth: &Secret) -> Vec<Heard> {
        let github::plans::start::Step::Send(exchange) = step else { panic!("one log exchange") };
        let encoded = github::request::encode(&exchange.request, &limits().github).expect("bounded log target");
        let response = self.fetch(encoded.method, &encoded.target, b"", Some(auth));
        assert_eq!(response.status, 200);
        for name in [
            b"X-RateLimit-Limit".as_slice(),
            b"X-RateLimit-Remaining",
            b"X-RateLimit-Used",
            b"X-RateLimit-Reset",
            b"X-RateLimit-Resource",
        ] {
            assert!(response.header(name).is_none(), "App JWT has no rate heads");
        }
        let mut head = github::header::Head::new();
        for (name, value) in &response.headers {
            github::header::field(&mut head, name, value, &limits().github);
        }
        let (rows, document) =
            response.decode_with(exchange.decoder(&limits().github).expect("log decoder"), &limits().github);
        let mut events = vec![Heard::Head { status: response.status, head }];
        events.extend(rows.into_iter().map(Heard::Row));
        events.push(Heard::Done(Body::Document(document)));
        events
    }
    fn log_page(&mut self, ask: github::log::Ask, auth: &Secret) -> github::log::Page {
        let bounds = Bounds { inner: 8, page_size: 1, rows: 8, answer_bytes: RESPONSE_BYTES, body_bytes: BODY_BYTES };
        let mut plan = github::plans::start::LogPage::new(ask);
        let step = plan.begin(bounds).expect("page plan begins");
        for event in self.log_events(step, auth) {
            if let Some(github::plans::start::Step::Done(result)) =
                plan.heard(event, bounds).expect("page plan accepts response")
            {
                let github::log::Listed::Page(page) = result.expect("page result") else { panic!("changed page") };
                return page;
            }
        }
        panic!("page settles")
    }
    fn logged(&mut self, row: &github::log::Row, auth: &Secret) -> github::log::Logged {
        let bounds = Bounds { inner: 8, page_size: 1, rows: 8, answer_bytes: RESPONSE_BYTES, body_bytes: BODY_BYTES };
        let mut plan = github::plans::start::LogDelivery::new(row.id, row.event.clone());
        let step = plan.begin(bounds).expect("delivery plan begins");
        for event in self.log_events(step, auth) {
            if let Some(github::plans::start::Step::Done(result)) =
                plan.heard(event, bounds).expect("delivery plan accepts response")
            {
                return result.expect("delivery result");
            }
        }
        panic!("delivery settles")
    }
}

#[test]
fn github_fake_push_and_logged_payloads_decode_alike() {
    for pushing in [false, true] {
        let mut config = config();
        let recipient_address = "127.0.0.1:30001".parse().expect("loopback recipient");
        if pushing {
            config.deliveries.endpoint =
                Some(fake::delivery::Endpoint { host: f::boxed(b"127.0.0.1:30001"), target: f::boxed(b"/hooks/1") });
        }
        let mut world = World::configured(f::SEED, config);
        let listener = if pushing {
            let Ok(Done::Fd(fd)) = world.client(Op::Socket { family: Family::Ipv4 }).result else {
                panic!("recipient socket")
            };
            assert!(world.client(Op::Bind { fd, addr: recipient_address }).result.is_ok());
            assert!(world.client(Op::Listen { fd, backlog: 4 }).result.is_ok());
            Some(fd)
        } else {
            None
        };
        let token = world.token();
        if let Some(listener) = listener {
            world.recipient(listener);
        }
        let response = world.fetch(Method::Post, b"/repos/org/repo/issues", ISSUE, Some(&token));
        assert_eq!(response.status, 201);
        for name in [
            b"X-RateLimit-Limit".as_slice(),
            b"X-RateLimit-Remaining",
            b"X-RateLimit-Used",
            b"X-RateLimit-Reset",
            b"X-RateLimit-Resource",
        ] {
            assert!(response.header(name).is_some(), "installation token rate header");
        }
        let pushed = listener.map(|listener| world.recipient(listener));
        while world.github.next_policy_deadline().is_some() {
            world.pass();
        }
        let auth = world.jwt();
        let page = world.log_page(github::log::Ask::First { validator: None }, &auth);
        let row = page.rows.first().expect("newest issue hook");
        assert_eq!(row.event.as_ref(), b"issues");
        assert_eq!(row.status_code, if pushing { 207 } else { 502 });
        let keep = row.guid;
        let logged = world.logged(row, &auth);
        if let Some(wire) = pushed {
            let split = bytes::find(&wire, b"\r\n\r\n").expect("POST head");
            assert_eq!(
                github::delivery::decode(&logged.event, &wire[split + 4..], &limits().github).expect("pushed payload"),
                logged.delivery
            );
        }
        let before = world.github.observations().len();
        let next = world.log_page(github::log::Ask::First { validator: None }, &auth);
        assert_eq!(next.rows.first().expect("kept guid").guid, keep);
        let reads: Vec<_> = world.github.observations()[before..]
            .iter()
            .filter_map(|record| match record {
                Observation::Head { call, .. } => Some(call),
                Observation::Connection { .. } | Observation::Call { .. } | Observation::Delivery { .. } => None,
            })
            .collect();
        assert_eq!(reads.len(), 1, "the reader stops at its kept guid");
        assert_eq!(
            reads[0].header(b"authorization"),
            Some(b"".as_slice()),
            "the JWT is observed with its value redacted"
        );
        if let Some(listener) = listener {
            support::call(&mut world.sim, world.client_pid, 3, Op::Close { fd: listener });
        }
        world.settle();
    }
}
