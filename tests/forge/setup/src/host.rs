//! The host over HTTP bytes, a fake Forgejo and io played by hand.
use crate::{fixtures, tls::Carrier, world::seeded_config};
use skein_fake_forge_protocol::{self as fake, Service};
use skein_forge_machine_world::github::{self, Fake};
use skein_forge_setup::{
    host::{Answer, Event, Host, Limits, Need, Plan, Request, Settings},
    item::Item,
    reconcile::{Failure, Outcome},
};
use skein_lib::{
    Duration, Env, Queue, Secret, Time, Token, Wall,
    stream::{Down, Read, Up},
};
use std::collections::{BTreeMap, VecDeque};
#[derive(Clone, Copy, Debug)]
pub enum Connect {
    Made,
    Refused,
    Silent,
    Unavailable,
    Answers(u16, &'static [u8]),
    /// Answers without ending its sending half; io must time out its close.
    AnswersOpen(u16, &'static [u8]),
    /// GitHub processes the request, then the connection loses its response.
    LostGitHubReply,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Heard {
    Done { run: Token, outcome: Outcome, at: Duration },
    Stop { run: Token, failure: Failure, at: Duration },
    Closed { at: Duration },
}
enum Api {
    GitHub,
    Forgejo,
}
enum PeerEnd {
    Ends,
    Open,
}
struct Peer {
    pipe: Carrier,
    server: Option<fake::Server>,
    github: bool,
    api: Api,
    above: Queue<fake::Event>,
    below: Queue<Down>,
    answer: Option<(u16, &'static [u8])>,
    head: Vec<u8>,
    responded: bool,
    lose_reply: bool,
    end: PeerEnd,
    closing: Option<closing::Closing>,
}
#[path = "closing.rs"]
mod closing;
#[path = "memory.rs"]
pub mod memory;
pub struct HostWorld {
    host: memory::Metered,
    tls_addresses: Vec<skein_io::Addr>,
    env: Env<Limits>,
    service: Service,
    github: Option<Fake>,
    wall_base: Option<Wall>,
    fake_env: Env<fake::Limits>,
    above: Queue<Event>,
    below: Queue<skein_io::Request>,
    peers: BTreeMap<Token, Peer>,
    script: VecDeque<Connect>,
    files: Vec<(Item, Secret)>,
    heard: Vec<Heard>,
    connects: Vec<Duration>,
    connection_addresses: Vec<skein_io::Addr>,
    aborts: u32,
    seed: u64,
    needs: Vec<Need>,
    needs_by_run: Vec<(Token, Need)>,
    reads: Vec<Item>,
    pause_writes: bool,
    pending_writes: Vec<(Token, Item, Secret)>,
    pause_reads: bool,
    pasted: Option<Secret>,
    pending_reads: Vec<(Token, Item)>,
    github_requests: Vec<GitHubRequest>,
}
/// Public API trace: browser URLs, credentials and bodies never enter it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GitHubRequest {
    pub method: String,
    pub target: String,
    pub auth: &'static str,
}
fn github_record(bytes: &[u8]) -> GitHubRequest {
    let text = core::str::from_utf8(bytes).expect("HTTP fixture text");
    let mut line = text.lines().next().expect("request line").split_whitespace();
    let method = line.next().expect("method").to_owned();
    let target = line.next().expect("target");
    let target = if target.starts_with("/app-manifests/") { "/app-manifests/<redacted>/conversions" } else { target };
    let auth = if text.lines().any(|line| line.starts_with("Authorization: Bearer eyJ")) {
        "App"
    } else if text.lines().any(|line| line.starts_with("Authorization:")) {
        "Bearer"
    } else {
        "None"
    };
    GitHubRequest { method, target: target.to_owned(), auth }
}
impl HostWorld {
    #[must_use]
    pub fn new(seed: u64, script: &[Connect]) -> Self {
        Self::new_with(seed, script, fixtures::host_settings(), fixtures::host_limits())
    }
    /// The two forge kinds, with the GitHub peer's App and accepted installations.
    #[must_use]
    pub fn github(seed: u64, script: &[Connect]) -> Self {
        let mut limits = fixtures::host_limits();
        // Conversion accepts PEM text up to twice the DER bound (forge-setup.md, 5.2).
        limits.string_bytes = fixtures::github_given().limits.key_bytes.checked_mul(2).expect("PEM bound");
        let mut world = Self::new_with(seed, script, fixtures::github_settings(), limits);
        world.github = Some(Fake::bare(seed));
        world.wall_base = Some(github::WALL_BASE);
        world.set_time(Time::ZERO);
        world.put(Item::AppId, fixtures::app_id(7));
        world.put(Item::AppKey, fixtures::secret(github::app_key()));
        world.put(Item::Accepted, fixtures::accepted(&[1, 2]));
        world
    }
    /// Configured fixture App, independent of conversion and signing its public key.
    #[must_use]
    pub fn github_scenario(
        seed: u64,
        script: &[Connect],
        facts: skein_fake_forge_github::service::AppFacts,
        hook: &[u8],
    ) -> Self {
        let mut world = Self::github(seed, script);
        world.github = Some(Fake::scenario(seed, facts, true, hook));
        world.put(Item::AppSlug, fixtures::secret(b"test-app"));
        world
    }
    #[must_use]
    pub fn github_requests(&self) -> &[GitHubRequest] {
        &self.github_requests
    }
    pub fn github_installation_ids(&mut self) -> Vec<u64> {
        self.github.as_mut().expect("GitHub scenario").installation_ids()
    }
    /// Selects all repositories of this fixture installation's account.
    pub fn github_all_repositories(&mut self, id: u64) {
        use skein_fake_forge_github::installations::Reach;
        assert!(self.github.as_mut().expect("GitHub scenario").installations_mut().set_reach(id, Reach::All));
    }
    pub fn github_remove(&mut self, id: u64) {
        assert!(self.github.as_mut().expect("GitHub scenario").installations_mut().remove(id));
    }
    /// Replaces one fixture row at a supplied wall date, using the fake's store.
    pub fn github_reinsert(&mut self, id: u64, created: Wall) -> u64 {
        use skein_fake_forge_github::installations::{Kind, Reach};
        let store = self.github.as_mut().expect("GitHub scenario").installations_mut();
        let row = store.get(id).expect("fixture installation");
        let (account, login) = (row.account, row.login.clone());
        assert!(store.remove(id));
        store.install(account, &login, Kind::User, Reach::All, created).expect("vacated fixture row")
    }
    /// Plays a browser request against the same fake used by the host's HTTP peer.
    pub fn github_page(&mut self, request: &[u8]) -> Vec<u8> {
        self.github.as_mut().expect("GitHub scenario").page(request, self.env.wall)
    }
    /// The browser's posted manifest, using the fake's read-only model.
    #[must_use]
    pub fn github_manifest(&self, state: &[u8]) -> Option<&skein_fake_forge_github::manifest::Manifest> {
        self.github.as_ref().expect("GitHub scenario").manifest(state)
    }
    #[must_use]
    pub fn new_with(seed: u64, script: &[Connect], settings: Settings, limits: Limits) -> Self {
        Self::with_settings_and_limits(seed, script, fixtures::config(), settings, limits)
    }
    #[must_use]
    pub fn with_config(seed: u64, script: &[Connect], config: fake::Config) -> Self {
        Self::with_settings(seed, script, config, fixtures::host_settings())
    }
    #[must_use]
    pub fn with_settings(
        seed: u64,
        script: &[Connect],
        config: fake::Config,
        settings: skein_forge_setup::host::Settings,
    ) -> Self {
        Self::with_settings_and_limits(seed, script, config, settings, fixtures::host_limits())
    }
    fn with_settings_and_limits(
        seed: u64,
        script: &[Connect],
        config: fake::Config,
        settings: Settings,
        limits: Limits,
    ) -> Self {
        let tls_addresses = settings
            .forges
            .as_slice()
            .iter()
            .filter_map(|forge| match &forge.transport {
                skein_forge_setup::host::Transport::Plaintext { .. } => None,
                skein_forge_setup::host::Transport::Tls { .. } => Some(forge.address),
            })
            .collect();
        let (service, fake_limits) = seeded_config(seed, false, config);
        let mut documents = skein_forge_setup::host::documents(&limits);
        // The host may retain a larger GitHub PEM scalar than Forgejo's peer emits.
        assert!(documents.string_bytes >= fake_limits.documents.string_bytes);
        documents.string_bytes = fake_limits.documents.string_bytes;
        assert_eq!(documents, fake_limits.documents);
        Self {
            host: memory::Metered::new(Host::new(&limits, settings, fixtures::crypto()).expect("host fixtures")),
            tls_addresses,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            service,
            github: None,
            wall_base: None,
            fake_env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: fake_limits },
            above: Queue::with_capacity(16),
            below: Queue::with_capacity(16),
            peers: BTreeMap::new(),
            script: script.iter().copied().collect(),
            files: vec![(Item::AdminToken, fixtures::secret(fixtures::ADMIN))],
            heard: Vec::new(),
            connects: Vec::new(),
            connection_addresses: Vec::new(),
            aborts: 0,
            seed,
            needs: Vec::new(),
            needs_by_run: Vec::new(),
            reads: Vec::new(),
            pause_writes: false,
            pending_writes: Vec::new(),
            pause_reads: false,
            pasted: None,
            pending_reads: Vec::new(),
            github_requests: Vec::new(),
        }
    }
    #[must_use]
    pub fn connection_addresses(&self) -> &[skein_io::Addr] {
        &self.connection_addresses
    }
    pub fn run(&mut self, run: u64) {
        self.run_adopted(run, &[]);
    }
    pub fn run_adopted(&mut self, run: u64, adopted: &[u64]) {
        self.run_plan(run, Plan::Reconcile { forge: 1, adopted: adopted.into() });
    }
    pub fn run_plan(&mut self, run: u64, plan: Plan) {
        self.action(Request::Run { run: Token::new(run), plan });
    }
    #[must_use]
    pub fn store(&self) -> &fake::setup::Store {
        self.service.setup()
    }
    pub fn store_mut(&mut self) -> &mut fake::setup::Store {
        self.service.setup_mut()
    }
    pub fn put(&mut self, item: Item, bytes: Secret) {
        if let Some((_, value)) = self.files.iter_mut().find(|(key, _)| *key == item) {
            *value = bytes;
        } else {
            self.files.push((item, bytes));
        }
    }
    pub fn forget(&mut self, item: Item) {
        self.files.retain(|(key, _)| *key != item);
    }
    #[must_use]
    pub fn reads(&self) -> &[Item] {
        &self.reads
    }
    #[must_use]
    pub fn needs(&self) -> &[Need] {
        &self.needs
    }
    /// Needs in delivery order with their originating run tokens.
    #[must_use]
    pub fn needs_by_run(&self) -> &[(Token, Need)] {
        &self.needs_by_run
    }
    /// Ephemeral owner input, consumed by its single Pasted read (5.8).
    pub fn paste(&mut self, pem: Secret) {
        self.pasted = Some(pem);
    }
    fn read_item(&mut self, item: Item) -> Option<Secret> {
        if item == Item::Pasted { self.pasted.take() } else { self.file(item).cloned() }
    }
    pub fn pause_reads(&mut self) {
        self.pause_reads = true;
    }
    #[must_use]
    pub fn pending_reads(&self) -> &[(Token, Item)] {
        &self.pending_reads
    }
    /// Answers the chosen run's held read from the owner's current items.
    pub fn read_pending(&mut self, run: u64) {
        let run = Token::new(run);
        let at = self.pending_reads.iter().position(|(token, _)| *token == run).expect("held read");
        let (_, item) = self.pending_reads.remove(at);
        let value = self.read_item(item);
        self.action(Request::Answer { run, answer: Answer::Read(value) });
    }
    pub fn resume_reads(&mut self) {
        self.pause_reads = false;
        for (run, item) in core::mem::take(&mut self.pending_reads) {
            let value = self.read_item(item);
            self.action(Request::Answer { run, answer: Answer::Read(value) });
        }
    }
    pub fn pause_writes(&mut self) {
        self.pause_writes = true;
    }
    /// Keeps each item whole before answering Written, as the owner must.
    pub fn write_pending(&mut self) {
        for (run, item, bytes) in core::mem::take(&mut self.pending_writes) {
            self.put(item, bytes);
            self.action(Request::Answer { run, answer: Answer::Written });
        }
    }
    #[must_use]
    pub fn requests(&self) -> &fake::Tally {
        self.service.tally()
    }
    pub fn close(&mut self) {
        self.action(Request::Close);
    }
    fn action(&mut self, request: Request) {
        self.host.down(&self.env, request, &mut self.above, &mut self.below);
    }
    fn up(&mut self, event: skein_io::Event) {
        self.host.up(&self.env, event, &mut self.above, &mut self.below);
    }
    #[must_use]
    pub fn heard(&self) -> &[Heard] {
        &self.heard
    }
    #[must_use]
    pub fn connects(&self) -> &[Duration] {
        &self.connects
    }
    #[must_use]
    pub fn aborts(&self) -> u32 {
        self.aborts
    }
    #[must_use]
    pub fn file(&self, item: Item) -> Option<&Secret> {
        self.files.iter().find(|(key, _)| *key == item).map(|(_, value)| value)
    }
    /// Completes all work at the present virtual time.
    pub fn settle(&mut self) {
        for _ in 0..100_000 {
            self.tick();
            if self.above.is_empty()
                && self.below.is_empty()
                && !self
                    .peers
                    .values()
                    .any(|peer| peer.pipe.has_work() || !peer.above.is_empty() || !peer.below.is_empty())
                && self.host.next_deadline().is_none_or(|at| at > self.env.now)
            {
                return;
            }
        }
        panic!("host world's same-time work did not settle");
    }
    /// Advances only to the host's next deadline until every run is idle.
    pub fn finish(&mut self) {
        for _ in 0..4096 {
            self.settle();
            let Some(at) = self.next_deadline() else {
                return;
            };
            assert!(at >= self.env.now);
            self.set_time(at);
            self.host.fire(&self.env, &mut self.above, &mut self.below);
        }
        panic!("host world's deadlines did not settle");
    }
    /// Advances the scenario clock and settles due host work.
    pub fn advance_to(&mut self, now: Time) {
        assert!(now >= self.env.now);
        self.set_time(now);
        self.host.fire(&self.env, &mut self.above, &mut self.below);
        self.settle();
    }
    fn set_time(&mut self, now: Time) {
        self.env.now = now;
        self.env.wall =
            self.wall_base.map_or(Wall::EPOCH, |base| Wall::from_nanos(base.as_nanos().saturating_add(now.as_nanos())));
        self.fake_env.now = now;
        self.fake_env.wall = self.env.wall;
    }
    fn next_deadline(&self) -> Option<Time> {
        self.peers
            .values()
            .filter_map(|p| p.closing.as_ref().and_then(closing::Closing::deadline))
            .chain(self.host.next_deadline())
            .min()
    }
    fn tick(&mut self) {
        let mut closed = Vec::new();
        for (owner, peer) in &mut self.peers {
            if let Some(closing) = &mut peer.closing
                && closing.advance(self.env.now)
            {
                assert!(closing.cancelled, "io's close deadline aborts the held receive");
                self.aborts += 1;
                closed.push(*owner);
            }
        }
        for owner in closed {
            self.end(owner);
        }
        self.events();
        while let Some(request) = self.below.pop() {
            self.io(request);
        }
        let owners: Vec<_> = self.peers.keys().copied().collect();
        for owner in owners {
            let peer = self.peers.get_mut(&owner).expect("peer");
            if let Some(server) = &mut peer.server {
                fake::resume(server, &mut self.service, &self.fake_env, &mut peer.above, &mut peer.below);
            }
            while let Some(down) = peer.below.pop() {
                peer.pipe.server_down(down);
            }
            if let Some(event) = peer.pipe.server_up() {
                if let Some(server) = &mut peer.server {
                    fake::up(server, &mut self.service, &self.fake_env, event, &mut peer.above, &mut peer.below);
                } else if peer.github {
                    github_answer(
                        peer,
                        event,
                        self.github.as_mut().expect("GitHub peer"),
                        self.env.wall,
                        &mut self.github_requests,
                    );
                } else if peer.answer.is_some() {
                    answer(peer, event, &mut self.github_requests);
                }
            }
            if let Some(up) = peer.pipe.client_up() {
                self.up(skein_io::Event::Stream { owner, up });
            }
            let peer = self.peers.get_mut(&owner).expect("peer");
            while let Some(event) = peer.above.pop() {
                match event {
                    fake::Event::Domain(_) => panic!("setup has no repository domain requests"),
                    fake::Event::Close => {
                        fake::closed(
                            peer.server.as_mut().expect("fake server"),
                            &mut self.service,
                            &self.fake_env,
                            &mut peer.above,
                            &mut peer.below,
                        );
                    }
                    fake::Event::Closed => (),
                }
            }
        }
        if self.host.next_deadline().is_some_and(|at| at <= self.env.now) {
            self.host.fire(&self.env, &mut self.above, &mut self.below);
        }
        self.service.reclaim();
    }
    fn events(&mut self) {
        while let Some(event) = self.above.pop() {
            let at = self.env.now.saturating_since(Time::ZERO);
            match event {
                Event::Need { run, need } => {
                    self.needs_by_run.push((run, need.clone()));
                    self.needs.push(need.clone());
                    let answer = match need {
                        Need::Read(item) => {
                            self.reads.push(item);
                            if self.pause_reads {
                                self.pending_reads.push((run, item));
                                continue;
                            }
                            Answer::Read(self.read_item(item))
                        }
                        Need::Write { item, bytes } => {
                            if self.pause_writes {
                                self.pending_writes.push((run, item, bytes));
                                continue;
                            }
                            self.put(item, bytes);
                            Answer::Written
                        }
                    };
                    self.action(Request::Answer { run, answer });
                }
                Event::Done { run, outcome } => self.heard.push(Heard::Done { run, outcome, at }),
                Event::Stop { run, failure } => self.heard.push(Heard::Stop { run, failure, at }),
                Event::Closed => self.heard.push(Heard::Closed { at }),
            }
        }
    }
    fn io(&mut self, request: skein_io::Request) {
        match request {
            skein_io::Request::Connect { owner, addr } => {
                self.connection_addresses.push(addr);
                self.connects.push(self.env.now.saturating_since(Time::ZERO));
                let connect = self.script.pop_front().unwrap_or(Connect::Made);
                self.up(skein_io::Event::Connecting { owner, socket: owner });
                if matches!(connect, Connect::Refused) {
                    self.up(skein_io::Event::Failed { owner, error: skein_io::Error::Refused });
                    self.up(skein_io::Event::Closed { owner });
                    return;
                }
                let mut peer = Peer {
                    pipe: Carrier::new(self.seed ^ owner.raw(), self.tls_addresses.contains(&addr)),
                    server: None,
                    api: if self.github.is_some() && addr == fixtures::github_forge().address {
                        Api::GitHub
                    } else {
                        Api::Forgejo
                    },
                    github: matches!(connect, Connect::Made | Connect::LostGitHubReply)
                        && self.github.is_some()
                        && addr == fixtures::github_forge().address,
                    above: Queue::with_capacity(16),
                    below: Queue::with_capacity(16),
                    answer: match connect {
                        Connect::Unavailable => Some((503, b"")),
                        Connect::Answers(status, body) | Connect::AnswersOpen(status, body) => Some((status, body)),
                        Connect::Made | Connect::Refused | Connect::Silent | Connect::LostGitHubReply => None,
                    },
                    head: Vec::new(),
                    responded: false,
                    lose_reply: matches!(connect, Connect::LostGitHubReply),
                    end: if matches!(connect, Connect::AnswersOpen(..)) { PeerEnd::Open } else { PeerEnd::Ends },
                    closing: None,
                };
                if peer.github {
                    peer.below.push(Down::Demand { read: Read::Arrived { max: 4096 }, room: 0 });
                } else if matches!(connect, Connect::Made) {
                    let mut server = fake::Server::new(owner, &self.fake_env.limits).expect("server");
                    fake::start(&mut server, &mut self.service, &self.fake_env, &mut peer.above, &mut peer.below);
                    peer.server = Some(server);
                } else if peer.answer.is_some() {
                    peer.below.push(Down::Demand { read: Read::Arrived { max: 4096 }, room: 0 });
                }
                assert!(self.peers.insert(owner, peer).is_none());
                if !matches!(connect, Connect::Silent) {
                    self.up(skein_io::Event::Connected { owner });
                }
            }
            skein_io::Request::Stream { stream, down } => {
                if let Some(peer) = self.peers.get_mut(&stream) {
                    peer.pipe.client_down(down);
                }
            }
            skein_io::Request::Abort { entity } => {
                self.aborts += 1;
                self.end(entity);
            }
            skein_io::Request::Close { entity } => {
                if let Some(peer) = self.peers.get_mut(&entity)
                    && matches!(peer.end, PeerEnd::Open)
                {
                    peer.closing = Some(closing::Closing::new(self.env.limits.io, entity, self.env.now));
                } else {
                    self.end(entity);
                }
            }
            skein_io::Request::Listen { .. }
            | skein_io::Request::Bind { .. }
            | skein_io::Request::Reject { .. }
            | skein_io::Request::Output { .. }
            | skein_io::Request::Spawn { .. }
            | skein_io::Request::Signal { .. }
            | skein_io::Request::Usage { .. }
            | skein_io::Request::Window { .. } => panic!("unexpected io request"),
        }
    }
    fn end(&mut self, owner: Token) {
        if let Some(mut peer) = self.peers.remove(&owner)
            && let Some(server) = &mut peer.server
        {
            fake::closed(server, &mut self.service, &self.fake_env, &mut peer.above, &mut peer.below);
        }
        self.up(skein_io::Event::Closed { owner });
    }
}
fn answer(peer: &mut Peer, event: Up, records: &mut Vec<GitHubRequest>) {
    match event {
        Up::Bytes(bytes) => {
            peer.head.extend_from_slice(&bytes);
            if peer.head.windows(4).any(|piece| piece == b"\r\n\r\n") {
                peer.below.push(Down::Demand { read: Read::Nothing, room: 4096 });
            } else {
                peer.below.push(Down::Demand { read: Read::Arrived { max: 4096 }, room: 0 });
            }
        }
        Up::Room => {
            assert!(!peer.responded);
            peer.responded = true;
            if matches!(peer.api, Api::GitHub) {
                records.push(github_record(&peer.head));
            }
            let (status, body) = peer.answer.expect("scripted answer");
            peer.below.push(Down::Send(skein_forge_machine_world::fixtures::response(status, body).into_boxed_slice()));
            if matches!(peer.end, PeerEnd::Ends) {
                peer.below.push(Down::Finish);
            }
        }
        Up::End => (),
        Up::Failed(_) => panic!("uncut scripted stream"),
    }
}

fn github_answer(peer: &mut Peer, event: Up, fake: &mut Fake, wall: Wall, records: &mut Vec<GitHubRequest>) {
    match event {
        Up::Bytes(bytes) => {
            peer.head.extend_from_slice(&bytes);
            if complete_request(&peer.head) {
                peer.below.push(Down::Demand { read: Read::Nothing, room: 4096 });
            } else {
                peer.below.push(Down::Demand { read: Read::Arrived { max: 4096 }, room: 0 });
            }
        }
        Up::Room => {
            assert!(!peer.responded);
            peer.responded = true;
            let request = core::mem::take(&mut peer.head);
            records.push(github_record(&request));
            let response = fake.serve(&request, wall);
            if !peer.lose_reply {
                peer.below.push(Down::Send(response.into_boxed_slice()));
            }
            peer.below.push(Down::Finish);
        }
        Up::End => (),
        Up::Failed(_) => panic!("uncut GitHub stream"),
    }
}
fn complete_request(bytes: &[u8]) -> bool {
    let Some(end) = bytes.windows(4).position(|piece| piece == b"\r\n\r\n") else {
        return false;
    };
    let mut length = 0;
    for line in bytes[..end].split(|byte| *byte == b'\n').skip(1) {
        let (name, value) = line.split_at(line.iter().position(|byte| *byte == b':').expect("header colon"));
        if name.eq_ignore_ascii_case(b"Content-Length") {
            length =
                core::str::from_utf8(value[1..].trim_ascii()).expect("decimal length").parse().expect("body length");
        }
    }
    bytes.len() >= end + 4 + length
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU32;
    use skein_forge_setup::host::Transport;
    use skein_tls_world::pki;

    #[test]
    fn the_github_picker_over_tls_reaches_the_same_done_as_plaintext() {
        let mut plain = HostWorld::github(7, &[]);
        let mut settings = fixtures::github_settings();
        settings.forges.get_mut(1).expect("GitHub forge").transport =
            Transport::Tls { server_name: pki::name(), trust: pki::client(&[]) };
        let mut tls = HostWorld::new_with(7, &[], settings, plain.env.limits);
        tls.github = Some(Fake::bare(7));
        tls.wall_base = plain.wall_base;
        tls.set_time(Time::ZERO);
        tls.files.clone_from(&plain.files);
        for world in [&mut plain, &mut tls] {
            world.wall_base = Some(pki::VALID);
            world.set_time(Time::ZERO);
            world.github_all_repositories(1);
            world.run_plan(
                1,
                Plan::Reachable { forge: 4, installation: Some(1), page: NonZeroU32::new(1).expect("page") },
            );
            world.finish();
            assert!(
                matches!(world.heard(), [Heard::Done { outcome: Outcome::Reachable { rows, more: true }, .. }] if rows.len() == 2),
                "{:?}",
                world.heard()
            );
        }
        assert_eq!(tls.heard(), plain.heard());
        assert_eq!(tls.github_requests(), plain.github_requests());
        assert_eq!(tls.connects().len(), 2);
        for world in [&mut plain, &mut tls] {
            world.close();
            world.finish();
            assert!(matches!(world.heard().last(), Some(Heard::Closed { .. })));
        }
    }
}
