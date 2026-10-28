use skein_fake_forge_domain::{self as domain, api};
use skein_fake_forge_protocol::{Config, Faults, Repository};
use skein_fake_peers::{
    Limits,
    forge::{self, Forge, Peer},
};
use skein_fake_peers_tests as support;
use skein_io::kernel::{Complete, Done, Family, Fd, Op, Submit};
use skein_lib::{Duration, List, Queue, Secret, Time, Token, bytes};
use skein_shell::Host;
use skein_sim::{Pid, Sim};
use std::{
    cell::RefCell,
    net::{Ipv4Addr, SocketAddr},
    rc::Rc,
};

pub const SEED: u64 = 23;
pub const MAX_PASSES: u32 = 1000;
pub const REPOSITORY: &[u8] = b"org/repo";
pub const BRANCH: &[u8] = b"work";
pub const USER: u64 = 1;
pub const ONE_REPLY: usize = 1;
// Setup creates commit 1; the tiny git caller stores and pushes the next commit.
pub const PUSHED_COMMIT: u64 = 2;
pub const WORK_BRANCH_BODY: &[u8] = br#"{"name":"work","commit":{"id":"975835de1c9756ce000000000000000200000000"},"effective_branch_protection_name":""}"#;
pub fn address() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, 0).into()
}
pub fn limits() -> Limits {
    Limits {
        io: skein_io::Limits {
            sockets: 5,
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
        observation_bytes: 32_768,
    }
}
pub const TOKEN: &[u8] = b"1111111111111111111111111111111111111111";
pub fn config() -> Config {
    let mut tokens = List::with_capacity(1);
    tokens.push((Secret::new(bytes::copy_of(TOKEN)), 1)).expect("one token");
    let mut users = List::with_capacity(2);
    users.push((1, bytes::copy_of(b"engine"))).expect("two users");
    users.push((2, bytes::copy_of(b"person"))).expect("two users");
    let mut repositories = List::with_capacity(1);
    repositories
        .push(Repository { id: 2, full: bytes::copy_of(b"org/repo"), protected: None })
        .expect("one repository");
    Config {
        base: bytes::copy_of(b"http://127.0.0.1:3000"),
        tokens,
        users,
        repositories,
        max_response_items: 2,
        default_paging_num: 30,
        hooks: None,
        faults: Faults { reset: 0, drop: 0, server_error: 0 },
    }
}
pub fn protocol_limits() -> skein_fake_forge_protocol::Limits {
    skein_fake_forge_protocol::Limits {
        calls: 4,
        connections: 4,
        raw_bytes: 4096,
        http: skein_http::server::Limits {
            head: 4096,
            headers: 16,
            body: 16384,
            read: 1024,
            response: 4096,
            send: 1024,
        },
        documents: skein_forge::forgejo::limits::Limits {
            json: skein_json::tokenizer::Limits { chunk: 1024, string: 16384, number: 32, depth: 16, length: 65536 },
            writer: skein_json::writer::Limits { depth: 16, length: 65536 },
            string_bytes: 512,
            body_bytes: 16384,
            inner: 32,
            target_bytes: 4096,
        },
    }
}
pub fn boxed(text: &[u8]) -> Box<[u8]> {
    bytes::copy_of(text)
}
pub fn fake_config() -> domain::Config {
    domain::Config {
        limits: domain::Limits {
            repositories: 1,
            users: 3,
            bypassing_users: 4,
            labels: 2,
            items: 64,
            comments: 32,
            dependencies: 2,
            reviews: 8,
            branches: 4,
            commits: 16,
            files: 4,
            statuses: 4,
            contexts: 4,
            pages: 1,
            name_bytes: 64,
            title_bytes: 64,
            body_bytes: 1024,
            content_bytes: 128,
            page_size: 2,
            calls: 4,
            hooks: 8,
            observations: 64,
        },
        latency_min: Duration::from_millis(10),
        latency_max: Duration::from_millis(50),
        settle_min: Duration::from_secs(2),
        settle_max: Duration::from_secs(2),
        late: 0,
        late_min: Duration::from_secs(1),
        late_max: Duration::from_secs(3),
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::from_secs(1),
        land_max: Duration::from_secs(3),
        rate_limit: 0,
        rate_window: Duration::from_secs(10),
        ci: 3,
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: domain::Skew::Ahead(Duration::from_secs(20)),
        status_updates: false,
        edit_updates: false,
    }
}

pub fn shared() -> Rc<RefCell<Forge>> {
    configured_forge(SEED, fake_config(), false)
}
pub(super) fn configured_forge(seed: u64, config: domain::Config, hooked: bool) -> Rc<RefCell<Forge>> {
    configured_checks_forge(
        seed,
        config,
        hooked,
        api::Checks {
            cues: Box::new([]),
            contexts: Box::new([]),
            latency_min: Duration::ZERO,
            latency_max: Duration::ZERO,
            silent: 0,
            passes: 0,
            reruns: 0,
            cue: None,
        },
    )
}
pub(super) fn configured_checks_forge(
    seed: u64,
    config: domain::Config,
    hooked: bool,
    checks: api::Checks,
) -> Rc<RefCell<Forge>> {
    let mut domain = domain::Domain::new(&config, seed);
    domain::repository(
        &mut domain,
        &config,
        api::Setup {
            id: 2,
            name: boxed(REPOSITORY),
            default: boxed(b"main"),
            tree: Box::new([]),
            labels: Box::new([]),
            checks,
            protection: None,
            hooked,
        },
    );
    domain::grant(&mut domain, REPOSITORY, USER, api::Permission::Admin);
    Rc::new(RefCell::new(Forge::new(domain, config)))
}

pub struct World {
    pub peer: Peer,
    pub forge: Rc<RefCell<Forge>>,
    pub sim: Sim,
    peer_pid: Pid,
    client_pid: Pid,
    passes: u32,
    serial: u64,
    pub replies: usize,
    receiver: Option<Receiver>,
    second_receiver: Option<Receiver>,
    pub connects: usize,
    posts: Vec<Vec<u8>>,
    first_delivery: Option<Time>,
}
impl World {
    pub fn new() -> Self {
        Self::configured(SEED, fake_config(), false)
    }
    fn configured(seed: u64, domain: domain::Config, hooked: bool) -> Self {
        let forge = configured_forge(seed, domain, hooked);
        let peer = Peer::new(address(), limits(), config(), protocol_limits(), Rc::clone(&forge), seed)
            .expect("checked forge fixture");
        let mut sim = Sim::new(seed, skein_sim::Config::calm());
        let peer_pid = sim.spawn_process();
        let client_pid = sim.spawn_process();
        Self {
            peer,
            forge,
            sim,
            peer_pid,
            client_pid,
            passes: 0,
            serial: 0,
            replies: 0,
            receiver: None,
            second_receiver: None,
            connects: 0,
            posts: Vec::new(),
            first_delivery: None,
        }
    }
    pub fn pass(&mut self) {
        self.passes += 1;
        assert!(self.passes <= MAX_PASSES, "forge world settles within its pass bound");
        self.sim.reap(self.peer_pid, self.peer.completions());
        self.peer.iterate(self.sim.now(), self.sim.wall());
        if self.first_delivery.is_none() && !self.delivery_outcomes().is_empty() {
            self.first_delivery = Some(self.sim.now());
        }
        self.connects +=
            self.peer.submissions().iter().filter(|submit| matches!(submit.kind, Op::Connect { .. })).count();
        self.sim.submit(self.peer_pid, self.peer.submissions());
        if let Some(receiver) = &mut self.receiver {
            receiver.reset_if_connecting(&mut self.sim, self.connects);
        }
        if !self.peer.work_pending(self.sim.now())
            && let Some(next) = [self.sim.next_due(), self.peer.next_deadline()].into_iter().flatten().min()
        {
            self.sim.advance_to(next);
        }
    }
    pub fn adopt_listener(&mut self) -> SocketAddr {
        let (fd, address) = support::held_listener(&mut self.sim, self.peer_pid);
        self.peer = Peer::new(address, limits(), config(), protocol_limits(), Rc::clone(&self.forge), SEED)
            .expect("forge configured at the held address");
        self.peer.adopt_listener(fd, address).expect("held listener before the first drive");
        address
    }
    pub fn bind(&mut self) {
        while self.peer.address().is_none() {
            self.pass();
        }
    }
    pub fn connect(&mut self) -> Fd {
        self.bind();
        let Done::Fd(fd) = support::call(&mut self.sim, self.client_pid, 1, Op::Socket { family: Family::Ipv4 }) else {
            panic!("socket descriptor");
        };
        assert_eq!(
            support::call(
                &mut self.sim,
                self.client_pid,
                2,
                Op::Connect { fd, addr: self.peer.address().expect("listening") }
            ),
            Done::Nothing
        );
        fd
    }
    pub fn open(&mut self) -> Fd {
        let fd = self.connect();
        while !self
            .peer
            .observations()
            .iter()
            .any(|record| matches!(record, forge::Observation::Connection { open: true, .. }))
        {
            self.pass();
        }
        fd
    }
    fn client(&mut self, kind: Op) -> Complete {
        self.serial += 1;
        let token = Token::new(self.serial);
        let mut out = Queue::with_capacity(1);
        let mut input = Queue::with_capacity(1);
        out.push(Submit { op: token, kind });
        self.sim.submit(self.client_pid, &mut out);
        loop {
            self.sim.reap(self.client_pid, &mut input);
            if let Some(complete) = input.pop() {
                assert_eq!(complete.op, token);
                return complete;
            }
            self.pass();
        }
    }
    pub fn exchange(&mut self, method: &str, target: &str, token: &[u8], body: &[u8]) -> Vec<u8> {
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: token {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            std::str::from_utf8(token).expect("fixture token"),
            body.len()
        );
        let fd = self.open();
        self.send(fd, request.as_bytes());
        self.send(fd, body);
        let response = self.response(fd);
        self.close(fd);
        response
    }
    pub fn send(&mut self, fd: Fd, bytes: &[u8]) {
        let mut from = 0;
        while usize::try_from(from).expect("request length") < bytes.len() {
            let complete = self.client(Op::Send { fd, bytes: boxed(bytes), from });
            let Ok(Done::Count(sent)) = complete.result else {
                panic!("send succeeds: {complete:?}");
            };
            assert!(sent > 0);
            from += sent;
        }
    }
    pub fn response(&mut self, fd: Fd) -> Vec<u8> {
        let mut response = Vec::new();
        loop {
            let complete = self.client(Op::Recv { fd, buf: Box::new([0; 1024]) });
            let Ok(Done::Count(count)) = complete.result else {
                panic!("read succeeds: {complete:?}");
            };
            if count == 0 {
                break;
            }
            let Op::Recv { buf, .. } = complete.kind else {
                panic!("receive buffer");
            };
            response.extend_from_slice(&buf[..usize::try_from(count).expect("read size")]);
        }
        self.replies += 1;
        response
    }
    pub fn close(&mut self, fd: Fd) {
        assert_eq!(support::call(&mut self.sim, self.client_pid, 3, Op::Close { fd }), Done::Nothing);
    }
    pub fn settle(&mut self) {
        if let Some(receiver) = &mut self.receiver {
            receiver.close(&mut self.sim);
        }
        if let Some(receiver) = &mut self.second_receiver {
            receiver.close(&mut self.sim);
        }
        self.peer.shutdown();
        while !self.peer.is_empty() {
            self.pass();
        }
        assert!(self.peer.is_empty());
        for pid in [self.peer_pid, self.client_pid] {
            self.sim.assert_quiescent(pid);
            self.sim.assert_no_open_fds(pid);
        }
    }
    pub fn git_push(&mut self) -> (u64, usize, Time) {
        let (token, commit) = {
            let mut forge = self.forge.borrow_mut();
            assert!(forge.pending.room() >= domain::MAX_OUT, "reserve external domain output");
            let config = forge.config;
            let commit = domain::commit(
                &mut forge.domain,
                &config,
                1,
                Box::new([api::File { path: boxed(b"file"), content: boxed(b"content") }]),
                b"message",
            )
            .expect("commit room")
            .expect("changed commit");
            let token = forge.next_reply();
            let env = skein_lib::Env { now: forge.now, wall: self.sim.wall(), limits: config };
            let Forge { domain, pending, .. } = &mut *forge;
            domain::step(
                domain,
                &env,
                domain::Event::Call {
                    reply_to: skein_lib::ReplyTo::new(token),
                    user: USER,
                    repository: boxed(REPOSITORY),
                    op: api::Op::Git(api::Git::Push { branch: boxed(BRANCH), commit, expected: None }),
                },
                pending,
            );
            (token, commit)
        };
        loop {
            self.pass();
            let mut forge = self.forge.borrow_mut();
            let mut replies = 0;
            for _ in 0..forge.pending.len() {
                let request = forge.pending.pop().expect("pending pass length");
                match request {
                    domain::Request::Reply { to, result } => {
                        let destination = to.into_token();
                        if destination == token {
                            assert_eq!(result, Ok(api::Answer::Pushed(api::Pushed::Pushed)));
                            replies += 1;
                        } else {
                            forge
                                .pending
                                .push(domain::Request::Reply { to: skein_lib::ReplyTo::new(destination), result });
                        }
                    }
                    request @ domain::Request::Hook { .. } => forge.pending.push(request),
                }
            }
            if replies > 0 {
                return (commit, replies, forge.now);
            }
        }
    }
}

pub fn request(path: &[u8]) -> Vec<u8> {
    let mut request = b"GET ".to_vec();
    request.extend_from_slice(path);
    request.extend_from_slice(b" HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: token ");
    request.extend_from_slice(TOKEN);
    request.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    request
}

pub struct Refusal {
    pub name: &'static str,
    pub address: SocketAddr,
    pub limits: Limits,
    pub protocol: skein_fake_forge_protocol::Limits,
    pub config: Config,
    pub shared: Rc<RefCell<Forge>>,
}
impl Refusal {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            address: address(),
            limits: limits(),
            protocol: protocol_limits(),
            config: config(),
            shared: shared(),
        }
    }
}
pub fn refusals() -> [Refusal; 10] {
    // Face's queue minimum is 64; one less cannot reserve a server pass.
    const SHORT_QUEUE: u32 = 63;
    // The protocol fixture admits four connections; five is one past it.
    const TOO_MANY_CONNECTIONS: u32 = 5;
    // One listener plus two server and two client slots require five sockets.
    const SHORT_SOCKETS: u32 = 4;
    // The shared fixture needs four calls plus eight hooks pending records.
    const SHORT_PENDING: u32 = 11;
    // HTTP head bytes 4096 require at least that much plaintext space.
    const SHORT_PLAINTEXT: u32 = 4095;
    // Chances are per thousand.
    const INVALID_CHANCE: u32 = 1001;
    let pending = shared();
    pending.borrow_mut().pending = Queue::with_capacity(SHORT_PENDING);
    let domain = shared();
    domain.borrow_mut().config.late = INVALID_CHANCE;
    [
        Refusal { address: (Ipv4Addr::new(192, 0, 2, 1), 80).into(), ..Refusal::new("non-loopback") },
        Refusal { limits: Limits { queue: SHORT_QUEUE, ..limits() }, ..Refusal::new("short queue") },
        Refusal {
            protocol: skein_fake_forge_protocol::Limits { calls: 0, ..protocol_limits() },
            ..Refusal::new("no protocol bound")
        },
        Refusal { limits: Limits { connections: TOO_MANY_CONNECTIONS, ..limits() }, ..Refusal::new("connections") },
        Refusal {
            limits: Limits { io: skein_io::Limits { sockets: SHORT_SOCKETS, ..limits().io }, ..limits() },
            ..Refusal::new("outbound sockets")
        },
        Refusal { shared: pending, ..Refusal::new("pending") },
        Refusal { limits: Limits { plaintext: SHORT_PLAINTEXT, ..limits() }, ..Refusal::new("wire") },
        Refusal { config: Config { base: Box::new([]), ..config() }, ..Refusal::new("config") },
        Refusal { shared: domain, ..Refusal::new("domain") },
        Refusal {
            // The configured base fits; a kernel-selected five-digit port needs one more byte.
            protocol: skein_fake_forge_protocol::Limits {
                documents: skein_forge::forgejo::limits::Limits {
                    target_bytes: u32::try_from(b"http://127.0.0.1:65535".len()).expect("literal origin") - 1,
                    ..protocol_limits().documents
                },
                ..protocol_limits()
            },
            ..Refusal::new("bound origin")
        },
    ]
}

// fake_config's late lower bound; chance knobs are per thousand.
pub const LATE_MIN: Duration = Duration::from_secs(1);
pub const CERTAIN: u32 = 1000;
pub const ONE_POST: usize = 1;
pub const NO_POSTS: usize = 0;
pub const WRITE_AND_READBACK: usize = 2;
pub const ISSUE_BODY: &[u8] = br#"{"title":"A kit issue","body":"A frozen body"}"#;
pub const SECRET: &[u8] = b"kit-webhook-secret";
// Two outbound slots hold their last admitted hooks; the third is one past that bound.
pub const HOOKS_WITHOUT_ROOM: usize = 3;
const RECEIVER_BACKLOG: u32 = 2;
const RECEIVE_BYTES: usize = 1024;
const RECEIVER_OP: u64 = 1;

pub fn endpoint(host: &[u8], secret: &[u8]) -> skein_fake_forge_protocol::delivery::Endpoint {
    skein_fake_forge_protocol::delivery::Endpoint { forge: 1, host: boxed(host), secret: Secret::new(boxed(secret)) }
}

struct Receiver {
    pid: Pid,
    listener: Option<Fd>,
    address: SocketAddr,
    reset: bool,
}
impl Receiver {
    fn bind(sim: &mut Sim, reset: bool) -> Self {
        let pid = sim.spawn_process();
        let Done::Fd(fd) = support::call(sim, pid, RECEIVER_OP, Op::Socket { family: Family::Ipv4 }) else {
            panic!("receiver socket");
        };
        let Done::Bound(address) = support::call(sim, pid, RECEIVER_OP, Op::Bind { fd, addr: address() }) else {
            panic!("receiver bind");
        };
        support::call(sim, pid, RECEIVER_OP, Op::Listen { fd, backlog: RECEIVER_BACKLOG });
        Self { pid, listener: Some(fd), address, reset }
    }
    fn reset_if_connecting(&mut self, sim: &mut Sim, connects: usize) {
        if self.reset && connects > 0 {
            self.close(sim);
        }
    }
    fn close(&mut self, sim: &mut Sim) {
        if let Some(fd) = self.listener.take() {
            support::call(sim, self.pid, RECEIVER_OP, Op::Close { fd });
        }
        sim.assert_quiescent(self.pid);
        sim.assert_no_open_fds(self.pid);
    }
}

impl World {
    pub fn delivery(seed: u64) -> Self {
        Self::configured(seed, fake_config(), true)
    }
    pub fn late_delivery(seed: u64) -> Self {
        Self::configured(seed, domain::Config { hooks_late: CERTAIN, ..fake_config() }, true)
    }
    pub fn lost_delivery(seed: u64) -> Self {
        Self::configured(seed, domain::Config { hooks_lost: CERTAIN, ..fake_config() }, true)
    }
    pub fn bind_receiver(&mut self, reset: bool) {
        self.bind();
        let receiver = Receiver::bind(&mut self.sim, reset);
        let host = receiver.address.to_string();
        self.forge.borrow_mut().hooks = Some(endpoint(host.as_bytes(), SECRET));
        self.receiver = Some(receiver);
    }
    pub fn bind_setup_receivers(&mut self) -> [SocketAddr; 2] {
        self.bind();
        let first = Receiver::bind(&mut self.sim, false);
        let second = Receiver::bind(&mut self.sim, false);
        let addresses = [first.address, second.address];
        self.receiver = Some(first);
        self.second_receiver = Some(second);
        addresses
    }
    pub fn finish_setup_deliveries(&mut self) {
        while self.delivery_outcomes().len() < 2 {
            self.pass();
        }
        self.receive_post_from(false);
        self.receive_post_from(true);
        self.settle();
    }
    pub fn create_issue_and_settle(&mut self) -> Vec<u8> {
        let fd = self.open();
        self.send(fd, &issue_request());
        let response = self.response(fd);
        self.close(fd);
        self.settle_hooks();
        response
    }
    pub fn settle_hooks(&mut self) {
        while self.peer.work_pending(self.sim.now()) || self.peer.next_policy_deadline().is_some() {
            self.pass();
        }
        // Connections with a write in flight need their io completions even after the policy has settled.
        while self.receiver.as_ref().is_some_and(|receiver| !receiver.reset)
            && self.delivery_outcomes().len() < self.connects
        {
            self.pass();
        }
        let posts = self.delivery_outcomes().iter().filter(|outcome| ***outcome == forge::Delivery::Written).count();
        for _ in 0..posts {
            self.receive_post();
        }
        self.settle();
    }
    fn receive_post(&mut self) {
        self.receive_post_from(false);
    }
    fn receive_post_from(&mut self, second: bool) {
        let receiver = if second { &self.second_receiver } else { &self.receiver };
        let receiver = receiver.as_ref().expect("a written delivery has its receiver");
        let pid = receiver.pid;
        let listener = receiver.listener.expect("receiver listens until posts are read");
        let Done::Accepted { fd, .. } = support::call(&mut self.sim, pid, RECEIVER_OP, Op::Accept { fd: listener })
        else {
            panic!("delivery accepted");
        };
        let mut post = Vec::new();
        loop {
            let complete = self.receiver_call(pid, Op::Recv { fd, buf: Box::new([0; RECEIVE_BYTES]) });
            let Ok(Done::Count(count)) = complete.result else {
                panic!("receiver read: {complete:?}");
            };
            if count == 0 {
                break;
            }
            let Op::Recv { buf, .. } = complete.kind else {
                unreachable!("receiver read");
            };
            post.extend_from_slice(buf.get(..usize::try_from(count).expect("read count")).expect("read bytes"));
        }
        support::call(&mut self.sim, pid, RECEIVER_OP, Op::Close { fd });
        self.posts.push(post);
    }
    fn receiver_call(&mut self, pid: Pid, kind: Op) -> Complete {
        let mut submissions = Queue::with_capacity(1);
        let mut completions = Queue::with_capacity(1);
        submissions.push(Submit { op: Token::new(RECEIVER_OP), kind });
        self.sim.submit(pid, &mut submissions);
        loop {
            self.sim.reap(pid, &mut completions);
            if let Some(complete) = completions.pop() {
                return complete;
            }
            self.pass();
        }
    }
    pub fn delivery_outcomes(&self) -> Vec<&forge::Delivery> {
        self.peer
            .observations()
            .iter()
            .filter_map(|record| match record {
                forge::Observation::Delivery { outcome, .. } => Some(outcome),
                forge::Observation::Connection { .. }
                | forge::Observation::Head { .. }
                | forge::Observation::Call { .. } => None,
            })
            .collect()
    }
    pub fn received_posts(&self) -> usize {
        self.posts.len()
    }
    pub fn first_delivery_at(&self) -> Duration {
        self.first_delivery.expect("delivery terminal").saturating_since(Time::ZERO)
    }
    pub fn domain_calls(&self) -> usize {
        self.peer.observations().iter().filter(|record| matches!(record, forge::Observation::Call { .. })).count()
    }
    pub fn frozen_trace(&self) -> String {
        format!("{:?}\n{:?}\n{}", self.peer.observations(), self.posts, self.sim.render_trace())
    }
    pub fn mutate_owner(&mut self) {
        self.forge.borrow_mut().hooks = Some(endpoint(b"127.0.0.1:1", b"changed-secret"));
        let mut forge = self.forge.borrow_mut();
        let config = forge.config;
        domain::commit(
            &mut forge.domain,
            &config,
            1,
            Box::new([api::File { path: boxed(b"changed"), content: boxed(b"changed content") }]),
            b"changed message",
        )
        .expect("commit room")
        .expect("changed content");
    }
    pub fn inject_hooks(&mut self, count: usize) {
        for _ in 0..count {
            self.forge.borrow_mut().pending.push(domain::Request::Hook {
                queue: None,
                repository: boxed(REPOSITORY),
                change: api::Change::Push,
                number: None,
                branch: Some(boxed(BRANCH)),
                commit: Some(PUSHED_COMMIT),
                by: Some(USER),
                body: None,
                ended: None,
            });
        }
        self.pass();
    }
    pub fn wire_posts(&self) -> &[Vec<u8>] {
        &self.posts
    }
}

fn issue_request() -> Vec<u8> {
    let mut request =
        b"POST /api/v1/repos/org/repo/issues HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: token ".to_vec();
    request.extend_from_slice(TOKEN);
    request.extend_from_slice(b"\r\nContent-Type: application/json\r\nContent-Length: ");
    request.extend_from_slice(ISSUE_BODY.len().to_string().as_bytes());
    request.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    request.extend_from_slice(ISSUE_BODY);
    request
}

type UnusableEndpoint = (&'static str, Option<&'static [u8]>, &'static [u8]);
pub fn unusable_endpoints() -> [UnusableEndpoint; 6] {
    [
        ("absent", None, SECRET),
        ("malformed", Some(b"kit.invalid:7200"), SECRET),
        ("non-loopback", Some(b"192.0.2.1:7200"), SECRET),
        ("empty host", Some(b""), SECRET),
        ("empty secret", Some(b"127.0.0.1:7200"), b""),
        ("injected host", Some(b"127.0.0.1:7200\r\nInjected: value"), SECRET),
    ]
}
pub fn refused_endpoint_updates_preserve_ownership() {
    let mut service =
        skein_fake_forge_protocol::Service::new(config(), &protocol_limits(), SEED).expect("fixture service");
    for (host, secret) in [(b"".as_slice(), SECRET), (b"127.0.0.1:7200".as_slice(), b"".as_slice())] {
        service.set_hooks(Some(endpoint(b"127.0.0.1:7200", SECRET))).expect("valid endpoint");
        assert_eq!(
            service.set_hooks(Some(endpoint(host, secret))).err(),
            Some(skein_fake_forge_protocol::Error::Limits)
        );
        let retained = service.set_hooks(None).expect("clear endpoint").expect("previous endpoint retained");
        assert_eq!(&*retained.host, b"127.0.0.1:7200");
        assert_eq!(retained.secret.expose(), SECRET);
    }
}
pub fn record_kinds(world: &World) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for record in world.peer.observations() {
        let kind = match record {
            forge::Observation::Connection { .. } => "connection",
            forge::Observation::Head { .. } => "head",
            forge::Observation::Call { .. } => "call",
            forge::Observation::Delivery { .. } => "delivery",
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}
#[derive(Clone, Copy)]
pub enum Unwritable {
    NoRoom,
    Refused,
    Reset,
}
pub struct UnwritableCase {
    pub name: &'static str,
    pub kind: Unwritable,
    pub hooks: usize,
    pub outcomes: Vec<&'static forge::Delivery>,
    pub connects: usize,
    pub posts: usize,
}
impl UnwritableCase {
    pub fn world(&self) -> World {
        let mut world = World::delivery(SEED);
        match self.kind {
            Unwritable::NoRoom => world.bind_receiver(false),
            Unwritable::Reset => world.bind_receiver(true),
            Unwritable::Refused => {
                world.bind();
                world.forge.borrow_mut().hooks = Some(endpoint(b"127.0.0.1:7200", SECRET));
            }
        }
        world
    }
}
pub fn unwritable_cases() -> [UnwritableCase; 3] {
    // The peer fixture reserves exactly two outbound slots; both admit a hook.
    const OUTBOUND_SLOTS: usize = 2;
    [
        UnwritableCase {
            name: "no room",
            kind: Unwritable::NoRoom,
            hooks: HOOKS_WITHOUT_ROOM,
            outcomes: vec![&forge::Delivery::NoRoom, &forge::Delivery::Written, &forge::Delivery::Written],
            connects: OUTBOUND_SLOTS,
            posts: OUTBOUND_SLOTS,
        },
        UnwritableCase {
            name: "refused",
            kind: Unwritable::Refused,
            hooks: ONE_POST,
            outcomes: vec![&forge::Delivery::Refused],
            connects: ONE_POST,
            posts: NO_POSTS,
        },
        UnwritableCase {
            name: "reset",
            kind: Unwritable::Reset,
            hooks: ONE_POST,
            outcomes: vec![&forge::Delivery::Reset],
            connects: ONE_POST,
            posts: NO_POSTS,
        },
    ]
}
pub fn recorded_delivery(seed: u64) -> World {
    let mut world = World::delivery(seed);
    world.bind_receiver(false);
    world.git_push();
    world.create_issue_and_settle();
    world
}

pub fn heads_hide_token(world: &World) -> bool {
    world.peer.observations().iter().all(|record| {
        if let forge::Observation::Head { call, .. } = record {
            !call.target.windows(TOKEN.len()).any(|window| window == TOKEN)
                && call.headers.iter().all(|header| {
                    !header.name.windows(TOKEN.len()).any(|window| window == TOKEN)
                        && !header.value.windows(TOKEN.len()).any(|window| window == TOKEN)
                })
        } else {
            true
        }
    })
}
