//! Hosted GitHub stores filled over simulated loopback, with every owner iteration measured.
#[path = "../../fake-forge/tests/github/fixtures.rs"]
#[expect(dead_code, reason = "The service binary also uses this fixture's direct request helper.")]
mod f;

use skein_fake_forge_github as fake;
use skein_fake_peers::{
    Limits,
    forge::{Forge, Peer},
};
use skein_fake_peers_tests as support;
use skein_heap::{Counting, Meter};
use skein_io::kernel::{Complete, Done, Family, Fd, Op, Submit};
use skein_lib::{Duration, Queue, Secret, Token, bytes};
use skein_shell::Host;
use skein_sim::{Pid, Sim};
use std::{cell::RefCell, net::Ipv4Addr, rc::Rc};

#[global_allocator]
static HEAP: Counting = Counting;

// Bounds construction and each iteration of the full-store hosted scenario.
const FAKE_GITHUB_PEER_PEAK_CEILING: u64 = 1_100_000;

fn limits() -> Limits {
    Limits {
        io: skein_io::Limits {
            sockets: 6,
            refusals: 1,
            intake: 32768,
            receive: 1024,
            output: 32768,
            sends: 2,
            accepts: 1,
            backlog: 1,
            close_timeout: Duration::from_secs(1),
            names: 0,
            retry: Duration::from_millis(1),
        },
        connections: 2,
        queue: 64,
        plaintext: 32768,
        ciphertext: 32768,
        // Each added clone retains a head and one domain call observation.
        observations: 128 + 2 * (f::limits().git_records + 1),
        observation_bytes: 131_072,
    }
}
struct World {
    peer: Peer,
    forge: Rc<RefCell<Forge>>,
    sim: Sim,
    peer_pid: Pid,
    client_pid: Pid,
    serial: u64,
    passes: u32,
    meter: Meter,
    peak: u64,
}
impl World {
    fn new() -> Self {
        let meter = Meter::new();
        meter.start();
        let (domain, config) = f::domain();
        let forge = Rc::new(RefCell::new(Forge::new(domain, config)));
        let peer = Peer::github(
            (Ipv4Addr::LOCALHOST, 0).into(),
            (Ipv4Addr::LOCALHOST, 0).into(),
            limits(),
            f::config(),
            f::limits(),
            Rc::clone(&forge),
            f::SEED,
        )
        .expect("bounded hosted scenario");
        let mut sim = Sim::new(f::SEED, skein_sim::Config::calm());
        let (peer_pid, client_pid) = (sim.spawn_process(), sim.spawn_process());
        let peak = meter.check(meter.end(), FAKE_GITHUB_PEER_PEAK_CEILING, &"construction with full queues");
        let mut world = Self { peer, forge, sim, peer_pid, client_pid, serial: 0, passes: 0, meter, peak };
        while world.peer.address().is_none() {
            world.pass();
        }
        world
    }
    fn pass(&mut self) {
        self.passes += 1;
        assert!(self.passes <= 10000, "bounded simulated run");
        self.meter.start();
        self.sim.reap(self.peer_pid, self.peer.completions());
        self.peer.iterate(self.sim.now(), self.sim.wall());
        self.sim.submit(self.peer_pid, self.peer.submissions());
        let measured = self.meter.end();
        // The scenario includes simulator and client allocations retained across iterations.
        self.peak = self.peak.max(self.meter.check(measured, FAKE_GITHUB_PEER_PEAK_CEILING, &"host iteration"));
        if !self.peer.work_pending(self.sim.now())
            && let Some(next) = self.sim.next_due()
        {
            self.sim.advance_to(next);
        }
    }
    fn client(&mut self, kind: Op) -> Complete {
        self.serial += 1;
        let mut output = Queue::with_capacity(1);
        let mut input = Queue::with_capacity(1);
        output.push(Submit { op: Token::new(self.serial), kind });
        self.sim.submit(self.client_pid, &mut output);
        loop {
            self.sim.reap(self.client_pid, &mut input);
            if let Some(complete) = input.pop() {
                return complete;
            }
            self.pass();
        }
    }
    fn open(&mut self) -> Fd {
        self.open_at(self.peer.address().expect("API listener"))
    }
    fn open_at(&mut self, addr: std::net::SocketAddr) -> Fd {
        let Ok(Done::Fd(fd)) = self.client(Op::Socket { family: Family::Ipv4 }).result else { panic!("client socket") };
        assert!(self.client(Op::Connect { fd, addr }).result.is_ok(), "client connects");
        fd
    }
    fn send(&mut self, fd: Fd, wire: &[u8]) {
        let mut from = 0;
        while usize::try_from(from).expect("send offset") < wire.len() {
            let complete = self.client(Op::Send { fd, bytes: wire.into(), from });
            let Ok(Done::Count(sent)) = complete.result else { panic!("client send") };
            assert!(sent > 0);
            from += sent;
        }
    }
    fn receive(&mut self, fd: Fd) -> Vec<u8> {
        let mut wire = Vec::new();
        loop {
            let complete = self.client(Op::Recv { fd, buf: Box::new([0; 1024]) });
            let Ok(Done::Count(count)) = complete.result else { panic!("client receive") };
            if count == 0 {
                break;
            }
            let Op::Recv { buf, .. } = complete.kind else { panic!("receive buffer") };
            wire.extend_from_slice(&buf[..usize::try_from(count).expect("receive count")]);
        }
        support::call(&mut self.sim, self.client_pid, 3, Op::Close { fd });
        wire
    }
    fn fetch(&mut self, method: &str, target: &[u8], body: &[u8], auth: Option<&Secret>, status: u16) -> Vec<u8> {
        let fd = self.open();
        self.send(fd, &request(method, target, body, auth));
        let wire = self.receive(fd);
        let actual = if wire.starts_with(b"HTTP/1.1 ") {
            std::str::from_utf8(&wire[9..12]).expect("numeric HTTP status").parse::<u16>().expect("HTTP status")
        } else {
            0
        };
        assert_eq!(actual, status, "{method}, target bytes {}", target.len());
        wire
    }
    fn jwt(&self) -> Secret {
        f::jwt(8, fake::keys::KEYS[0], self.sim.wall())
    }
    fn convert(&mut self) {
        let page = self.fetch("POST", b"/settings/apps/new?state=conversion", &f::form(), None, 200);
        let at = bytes::find(&page, b"?code=").expect("conversion link") + 6;
        let end = page[at..].iter().position(|b| *b == b'&').expect("state separator") + at;
        let mut target = b"/app-manifests/".to_vec();
        target.extend_from_slice(&page[at..end]);
        target.extend_from_slice(b"/conversions");
        self.fetch("POST", &target, b"", None, 201);
    }
    fn settle(&mut self) {
        self.peer.shutdown();
        while !self.peer.is_empty() {
            self.pass();
        }
        for pid in [self.peer_pid, self.client_pid] {
            self.sim.assert_quiescent(pid);
            self.sim.assert_no_open_fds(pid);
        }
    }
}
fn request(method: &str, target: &[u8], body: &[u8], auth: Option<&Secret>) -> Vec<u8> {
    let mut wire = method.as_bytes().to_vec();
    wire.push(b' ');
    wire.extend_from_slice(target);
    wire.extend_from_slice(b" HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: example-app\r\nX-GitHub-Api-Version: ");
    wire.extend_from_slice(skein_forge::github::pin::API_VERSION.as_bytes());
    if let Some(auth) = auth {
        wire.extend_from_slice(b"\r\nAuthorization: ");
        wire.extend_from_slice(auth.expose());
    }
    wire.extend_from_slice(format!("\r\nConnection: close\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
    wire.extend_from_slice(body);
    wire
}
fn token(wire: &[u8]) -> Secret {
    let at = bytes::find(wire, b"\"token\":\"").expect("minted token field") + 9;
    let end = wire[at..].iter().position(|byte| *byte == b'"').expect("token delimiter") + at;
    f::bearer(&wire[at..end])
}
#[test]
fn github_peer_full_stores_stay_under_its_peak_ceiling() {
    let mut world = World::new();
    world.convert();
    for (account, ids) in [("acme", "1,2"), ("example", "3,4")] {
        let target =
            format!("/apps/example-app/installations/new?state=installation&account={account}&repository_ids={ids}");
        world.fetch("GET", target.as_bytes(), b"", None, 302);
    }
    world.fetch("GET", b"/apps/example-app/installations/new?state=full&account=alice", b"", None, 503);
    let mut tokens = Vec::new();
    for n in 0..f::limits().installations.tokens {
        let target = format!("/app/installations/{}/access_tokens", 1 + n % 2);
        tokens.push(token(&world.fetch("POST", target.as_bytes(), b"", Some(&world.jwt()), 201)));
    }
    world.fetch("POST", b"/app/installations/1/access_tokens", b"", Some(&world.jwt()), 503);
    for (state, status) in [("one", 200), ("two", 200), ("three", 503)] {
        let target = format!("/settings/apps/new?state={state}");
        world.fetch("POST", target.as_bytes(), &f::form(), None, status);
    }
    world.fetch("GET", b"/app", b"", Some(&world.jwt()), 200);
    for (index, token) in tokens.iter().enumerate() {
        world.fetch("GET", b"/installation/repositories?per_page=100&page=1", b"", Some(token), 200);
        let target = format!(
            "/repos/{}/{}",
            if index % 2 == 0 { "acme" } else { "example" },
            if index % 2 == 0 { "widgets" } else { "third" }
        );
        world.fetch("GET", target.as_bytes(), b"", Some(token), 200);
    }
    for id in 1..=4 {
        let forge = world.forge.borrow();
        let skein_fake_forge_domain::api::Answer::Pull(pull) = forge
            .domain
            .inspect(&forge.config, &f::path(id), &skein_fake_forge_domain::api::Read::Pull { number: 1 })
            .unwrap()
        else {
            panic!("queued pull")
        };
        assert!(pull.queued.is_some(), "queue remains full during the hosted run");
    }
    let mut target = vec![b'x'; usize::try_from(f::limits().http.head).expect("head length") - 256];
    target[0] = b'/';
    for _ in 0..=f::limits().blob_records {
        let fd = world.open_at(world.peer.blob_address().expect("blob listener"));
        world.send(fd, &request("GET", &target, b"", None));
        assert!(world.receive(fd).starts_with(b"HTTP/1.1 404 "));
    }
    assert_eq!(world.peer.blob_records().expect("GitHub records").len(), f::limits().blob_records);
    let bearer = tokens.first().unwrap().expose().strip_prefix(b"Bearer ").unwrap();
    let mut login = b"x-access-token:".to_vec();
    login.extend_from_slice(bearer);
    let count =
        skein_lib::base64::encoded_len(skein_lib::base64::Alphabet::Standard, u32::try_from(login.len()).unwrap())
            .unwrap();
    let mut header = skein_lib::Writer::new(usize::try_from(count).unwrap().saturating_add(6));
    header.put(b"Basic ").unwrap();
    skein_lib::base64::encode(skein_lib::base64::Alphabet::Standard, &login, &mut header).unwrap();
    let auth = Secret::new(header.finish());
    for _ in 0..=f::limits().git_records {
        world.fetch("POST", b"/.fake/git/clone", br#"{"repository":"acme/widgets"}"#, Some(&auth), 200);
    }
    assert_eq!(world.peer.git_calls().unwrap().len(), f::limits().git_records);

    // Both connection slots retain parsed heads and partially received bodies.
    let clients = [world.open(), world.open()];
    for fd in clients {
        world.send(fd, b"POST /repos/acme/widgets/issues HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 4\r\nConnection: close\r\n\r\nx");
    }
    for _ in 0..20 {
        world.pass();
    }
    for fd in clients {
        world.send(fd, b"xxx");
        drop(world.receive(fd));
    }
    world.settle();
    assert!(world.peak <= FAKE_GITHUB_PEER_PEAK_CEILING);
}
