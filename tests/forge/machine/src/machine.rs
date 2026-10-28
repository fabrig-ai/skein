//! Scripted HTTP responses, seeded streams and the forge component's io.
use crate::{fixtures, github, peer::Peer};
use skein_forge::api;
use skein_forge_connection::{Answered, Component, Event, Limits, Request, Settings, Stopped};
use skein_lib::{
    Duration, Env, Queue, Time, Token, Wall,
    stream::{Down, Fault, Up},
};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::Path,
};
/// The request's `METHOD target` and its scripted response.
#[derive(Debug)]
pub struct Answer {
    pub request: &'static str,
    pub response: Response,
}
impl Answer {
    /// A recorded response, served byte for byte.
    #[must_use]
    pub fn case(request: &'static str, case: &'static str) -> Self {
        Self { request, response: Response::Case(case) }
    }
    /// Answers from the fake GitHub layer.
    #[must_use]
    pub fn served(request: &'static str) -> Self {
        Self { request, response: Response::Served }
    }
}
/// The bytes or fault one request receives.
#[derive(Debug)]
pub enum Response {
    /// A recorded HTTP response.
    Case(&'static str),
    /// Computed by the fake GitHub layer from the request the component sent (`Machine::github`).
    Served,
    /// A literal HTTP response.
    Bytes(Vec<u8>),
    /// A reset after the request but before its response.
    Reset,
    /// A recorded response's prefix, followed by the stream's end.
    Cut { case: &'static str, at: usize },
    /// Wait for the call's deadline.
    Silent,
}
/// The next connection's outcome.
#[derive(Clone, Copy, Debug)]
pub enum Connect {
    Made,
    Refused,
}
/// An outside boundary held for a deadline or owner action.
#[derive(Clone, Copy, Debug)]
pub enum Pause {
    Connecting,
    /// The server handshake flight, before the client can become ready.
    Handshaking,
    /// Bytes of the next request delivered to the peer before holding io.
    Writing(usize),
    /// Bytes of the next response delivered before the peer goes silent.
    Reading(usize),
}
/// A byte fault in one generated request or response.
#[derive(Clone, Copy, Debug)]
pub struct Cut {
    pub request: usize,
    pub response: bool,
    pub at: usize,
    pub reset: bool,
}
#[derive(Debug)]
struct AccessGeneration {
    number: u64,
    connected: bool,
    stopped: bool,
}
impl AccessGeneration {
    fn new(number: u64) -> Self {
        Self { number, connected: false, stopped: false }
    }
}
/// The real component, with a scripted peer and io played by the world.
pub struct Machine {
    component: Component,
    env: Env<Limits>,
    above: Queue<Event>,
    below: Queue<skein_io::Request>,
    peers: BTreeMap<Token, Peer>,
    script: VecDeque<Answer>,
    connects: VecDeque<Connect>,
    answers: BTreeMap<u64, Answered>,
    requests: Vec<Vec<u8>>,
    stopped: Vec<(u16, Stopped)>,
    connected_forges: Vec<u16>,
    git_tokens: Vec<Event>,
    installation_events: Vec<Event>,
    facts: Vec<Event>,
    hints: Vec<skein_forge_connection::hooks::Hinted>,
    log_script: VecDeque<String>,
    access_generations: BTreeMap<u16, Option<AccessGeneration>>,
    forge_addresses: Vec<(skein_io::Addr, u16)>,
    connection_generations: BTreeMap<Token, (u16, u64)>,
    event_source: Option<(u16, u64)>,
    opened: u32,
    aborts: u32,
    closed: bool,
    seed: u64,
    pause: Option<(usize, Pause)>,
    paused: Option<Token>,
    cut: Option<Cut>,
    deliveries: Vec<Answered>,
    closes: u32,
    held_response: Option<(Token, Vec<u8>)>,
    written: Vec<usize>,
    closed_events: u32,
    hold_settlement: bool,
    settling: Vec<Token>,
    event_order: Vec<&'static str>,
    tls_addresses: Vec<skein_io::Addr>,
    fake: Option<github::Fake>,
    wall_base: Option<Wall>,
    hold_output: bool,
    outputs_before_requests: Vec<u32>,
    handshake_cut: Option<crate::tls::HandshakeCut>,
    flight_length: Option<usize>,
    flight_status: Option<crate::tls::CutStatus>,
}
impl std::fmt::Debug for Machine {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Machine")
            .field("now", &self.env.now)
            .field("connections", &self.opened)
            .field("requests", &self.requests.len())
            .field("answers", &self.answers.len())
            .field("stopped", &self.stopped)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}
impl Machine {
    /// Starts a component using the world's small fixture limits.
    #[must_use]
    pub fn new(seed: u64, script: Vec<Answer>) -> Self {
        Self::with(seed, fixtures::limits(), fixtures::settings(), script)
    }
    /// Prepends the recorded start reads.
    #[must_use]
    pub fn ready(seed: u64, mut script: Vec<Answer>) -> Self {
        let mut start =
            vec![Answer::case("GET /api/v1/user", "user"), Answer::case("GET /api/v1/settings/api", "settings-api")];
        start.append(&mut script);
        Self::new(seed, start)
    }
    /// Starts with other component limits or settings; shared bounds stay the fixture's.
    #[must_use]
    pub fn with(seed: u64, limits: Limits, settings: Settings, script: Vec<Answer>) -> Self {
        Self::build(seed, limits, settings, script, false)
    }
    /// Starts with a fake GitHub peer and a wall that follows the clock.
    #[must_use]
    pub fn github(seed: u64, limits: Limits, settings: Settings, script: Vec<Answer>) -> Self {
        Self::build(seed, limits, settings, script, true)
    }
    fn build(seed: u64, limits: Limits, settings: Settings, script: Vec<Answer>, github: bool) -> Self {
        let tls_addresses = settings
            .forges
            .iter()
            .filter_map(|forge| {
                matches!(forge.transport, skein_forge_connection::settings::Transport::Tls { .. })
                    .then_some(forge.address)
            })
            .collect::<Vec<_>>();
        assert!(!github || tls_addresses.is_empty(), "Machine::github serves plaintext forges");
        let wall_base = if github {
            Some(github::WALL_BASE)
        } else if tls_addresses.is_empty() {
            None
        } else {
            Some(skein_tls_world::pki::VALID)
        };
        let wall = wall_base.unwrap_or(Wall::EPOCH);
        let access_generations = settings
            .forges
            .iter()
            .map(|forge| (forge.number, forge.access.as_ref().map(|_| AccessGeneration::new(1))))
            .collect();
        let forge_addresses = settings.forges.iter().map(|forge| (forge.address, forge.number)).collect();
        let mut world = Self {
            component: Component::new(settings, &limits, &fixtures::shared()).expect("fixture bounds agree"),
            env: Env { now: Time::ZERO, wall, limits },
            above: Queue::with_capacity(256),
            below: Queue::with_capacity(256),
            peers: BTreeMap::new(),
            script: script.into(),
            connects: VecDeque::new(),
            answers: BTreeMap::new(),
            requests: Vec::new(),
            stopped: Vec::new(),
            connected_forges: Vec::new(),
            git_tokens: Vec::new(),
            installation_events: Vec::new(),
            facts: Vec::new(),
            hints: Vec::new(),
            log_script: VecDeque::new(),
            access_generations,
            forge_addresses,
            connection_generations: BTreeMap::new(),
            event_source: None,
            opened: 0,
            aborts: 0,
            closed: false,
            seed,
            pause: None,
            paused: None,
            cut: None,
            deliveries: Vec::new(),
            closes: 0,
            held_response: None,
            written: Vec::new(),
            closed_events: 0,
            hold_settlement: false,
            settling: Vec::new(),
            event_order: Vec::new(),
            tls_addresses,
            fake: github.then(|| github::Fake::new(seed)),
            wall_base,
            hold_output: false,
            outputs_before_requests: Vec::new(),
            handshake_cut: None,
            flight_length: None,
            flight_status: None,
        };
        world.component.begin(&world.env, &mut world.above, &mut world.below);
        world
    }
    /// Expected fake log reads, whose opaque ids and cursors are drawn by the fake.
    pub fn log_requests(&mut self, requests: Vec<String>) {
        self.log_script.extend(requests);
    }

    #[must_use]
    pub fn hints(&self) -> &[skein_forge_connection::hooks::Hinted] {
        &self.hints
    }

    /// Scenario access to the world's GitHub peer before its queued startup runs.
    pub fn github_fake_mut(&mut self) -> &mut github::Fake {
        self.fake.as_mut().expect("GitHub fixture")
    }

    #[must_use]
    pub fn blob_records(&self) -> &Queue<skein_fake_forge_github::logs::Record> {
        self.fake.as_ref().expect("GitHub fixture").blob_records()
    }

    /// A wall-clock adjustment leaves monotonic call deadlines running as before.
    pub fn jump_wall(&mut self, span: Duration) {
        let base = self.wall_base.as_mut().expect("world with a wall clock");
        *base = Wall::from_nanos(base.as_nanos().checked_add(span.as_nanos()).expect("fixture wall fits"));
    }

    /// Installation lifecycle events, independently of token deliveries.
    #[must_use]
    pub fn installation_events(&self) -> &[Event] {
        &self.installation_events
    }
    /// Capacity diagnostics from the component.
    #[must_use]
    pub fn facts(&self) -> &[Event] {
        &self.facts
    }
    /// Scenario access to the fake's installation store.
    pub fn github_installations_mut(&mut self) -> &mut skein_fake_forge_github::installations::Installations {
        self.fake.as_mut().expect("GitHub fixture").installations_mut()
    }
    /// Overrides the next connect outcomes; later ones are made.
    pub fn connects(&mut self, connects: &[Connect]) {
        self.connects.extend(connects);
    }
    /// Queues one domain operation.
    pub fn call(&mut self, call: u64, repository: api::Repository, op: api::Op) {
        self.component.down(
            &self.env,
            Request::Call { call: Token::new(call), repository, op },
            &mut self.above,
            &mut self.below,
        );
    }
    /// Ticks streams and timers until this span has elapsed on the virtual clock.
    pub fn run_for(&mut self, span: Duration) {
        let end = self.env.now.saturating_add(span);
        while self.env.now < end {
            self.tick();
            let busy = self.peers.values().any(Peer::busy);
            self.env.now = if busy || !self.below.is_empty() {
                self.env.now.saturating_add(Duration::from_nanos(10_000)).min(end)
            } else {
                self.component
                    .next_deadline()
                    .unwrap_or(end)
                    .max(self.env.now.saturating_add(Duration::from_nanos(10_000)))
                    .min(end)
            };
        }
        self.tick();
    }
    /// The wall of the last tick.
    #[must_use]
    pub fn wall(&self) -> Wall {
        self.env.wall
    }
    /// Holds the next exchange at an observable io boundary.
    pub fn pause(&mut self, pause: Pause) {
        self.pause_at(self.requests.len(), pause);
    }
    /// Holds one exchange, counting the start reads from zero.
    pub fn pause_at(&mut self, request: usize, pause: Pause) {
        self.pause = Some((request, pause));
    }
    /// Cuts the named generated exchange, counting start requests too.
    pub fn cut(&mut self, cut: Cut) {
        self.cut = Some(cut);
    }
    /// Cuts this trial's own captured TLS handshake flight.
    pub(crate) fn cut_handshake(&mut self, cut: crate::tls::HandshakeCut) {
        self.handshake_cut = Some(cut);
    }
    #[must_use]
    pub fn flight_length(&self) -> usize {
        self.flight_length.expect("captured flight")
    }
    pub(crate) fn flight_status(&self) -> crate::tls::CutStatus {
        self.flight_status.expect("captured cut position")
    }
    /// Drains bounded same-time work, then fires timers at exactly this time.
    pub fn advance_to(&mut self, time: Time) {
        self.settle();
        assert!(time >= self.env.now);
        self.env.now = time;
        self.settle();
    }
    fn settle(&mut self) {
        for _ in 0..1000 {
            self.tick();
            if self.closed
                || (!self.settling.is_empty()
                    && self.settling.len() == self.peers.len()
                    && self.below.is_empty()
                    && self.above.is_empty())
            {
                break;
            }
        }
    }
    /// Delivers an owner action, preserving every answer offer in the history.
    pub fn action(&mut self, request: Request) {
        self.events();
        if let Request::Connect { forge, .. } = &request
            && !self.closed
            && let Some(generation) = self.access_generations.get_mut(forge)
        {
            let next = generation.as_ref().map_or(1, |generation| generation.number + 1);
            *generation = Some(AccessGeneration::new(next));
        }
        if let Request::Refused(answer) | Request::Returned(answer) = &request {
            assert!(self.answers.remove(&answer.call.raw()).is_some(), "owner holds a delivered answer");
        }
        self.component.down(&self.env, request, &mut self.above, &mut self.below);
        self.settle();
    }
    /// All answer deliveries, including refused answers offered again.
    #[must_use]
    pub fn deliveries(&self) -> &[Answered] {
        &self.deliveries
    }
    /// Live socket destinations observed from io requests.
    #[must_use]
    pub fn addresses(&self) -> Vec<skein_io::Addr> {
        self.peers.values().map(|peer| peer.address).collect()
    }
    /// Component Closed events observed so far.
    #[must_use]
    pub fn closed_events(&self) -> u32 {
        self.closed_events
    }
    /// Defers io Closed acknowledgments, keeping each closing socket owned.
    pub fn hold_settlement(&mut self) {
        self.hold_settlement = true;
    }
    /// Completes every requested socket close, then drains bounded work.
    pub fn release_settlement(&mut self) {
        self.hold_settlement = false;
        for owner in std::mem::take(&mut self.settling) {
            self.peers.remove(&owner).expect("closing socket remains owned");
            self.up(skein_io::Event::Closed { owner });
        }
        self.settle();
    }
    /// Component events in owner-delivery order, independently of answer storage.
    #[must_use]
    pub fn event_order(&self) -> &[&'static str] {
        &self.event_order
    }
    /// Request bytes accepted by io, including a request cut before its framing ends.
    #[must_use]
    pub fn written_bytes(&self) -> &[usize] {
        &self.written
    }
    /// Sockets whose close or abort was requested.
    #[must_use]
    pub fn closes(&self) -> u32 {
        self.closes
    }
    /// Releases the rest of a response held at its byte boundary.
    pub fn release_response(&mut self) {
        let (owner, bytes) = self.held_response.take().expect("held response");
        let peer = self.peers.get_mut(&owner).expect("held peer");
        peer.write(&bytes);
        self.pause = None;
    }
    fn up(&mut self, event: skein_io::Event) {
        self.events();
        let owner = match &event {
            skein_io::Event::Listening { owner, .. }
            | skein_io::Event::Accepted { owner, .. }
            | skein_io::Event::Connecting { owner, .. }
            | skein_io::Event::Connected { owner }
            | skein_io::Event::Stream { owner, .. }
            | skein_io::Event::Output { owner, .. }
            | skein_io::Event::Spawned { owner, .. }
            | skein_io::Event::Exited { owner, .. }
            | skein_io::Event::Usage { owner, .. }
            | skein_io::Event::Window { owner, .. }
            | skein_io::Event::Failed { owner, .. }
            | skein_io::Event::Closed { owner } => Some(*owner),
            skein_io::Event::Resized | skein_io::Event::Shutdown { .. } => None,
        };
        self.event_source = owner.and_then(|owner| self.connection_generations.get(&owner).copied());
        self.component.up(&self.env, event, &mut self.above, &mut self.below);
        self.events();
        self.event_source = None;
    }
    fn access_event(&mut self, forge: u16, connected: bool) {
        let generation = self
            .access_generations
            .get_mut(&forge)
            .expect("configured forge")
            .as_mut()
            .expect("an access generation is running");
        if let Some((source, number)) = self.event_source
            && source == forge
        {
            assert_eq!(number, generation.number, "a replaced generation cannot answer");
        }
        if connected {
            assert!(!generation.connected && !generation.stopped, "Connected once before Stopped per generation");
            generation.connected = true;
        } else {
            assert!(!generation.stopped, "Stopped once per generation");
            generation.stopped = true;
        }
    }
    fn tick(&mut self) {
        for peer in self.peers.values_mut() {
            peer.begin_tick();
        }
        if let Some(base) = self.wall_base {
            self.env.wall = Wall::from_nanos(base.as_nanos().saturating_add(self.env.now.as_nanos()));
        }
        self.events();
        while let Some(request) = self.below.pop() {
            self.io(request);
        }
        let owners: Vec<_> = self.peers.keys().copied().collect();
        for owner in owners {
            if self.settling.contains(&owner) {
                continue;
            }
            let request = self.peers.get_mut(&owner).expect("peer").request();
            if let Some(request) = request {
                self.respond(owner, request);
            }
            if self.paused == Some(owner) {
                continue;
            }
            let peer = self.peers.get(&owner).expect("peer");
            let handshake = peer.handshaking();
            if self.pause.is_some_and(|(at, pause)| {
                at == self.requests.len()
                    && (matches!(pause, Pause::Writing(0)) && !handshake
                        || matches!(pause, Pause::Handshaking) && handshake)
            }) {
                continue;
            }
            let peer = self.peers.get_mut(&owner).expect("peer");
            if peer.connected
                && let Some(up) = peer.wire.answer()
            {
                peer.answered();
                if matches!(up, Up::Room) && peer.handshake_flight_delivered() {
                    if self
                        .pause
                        .is_some_and(|(at, pause)| at == self.requests.len() && matches!(pause, Pause::Writing(0)))
                    {
                        self.paused = Some(owner);
                        continue;
                    }
                    if let Some(cut) = self.cut
                        && !cut.response
                        && cut.at == 0
                        && cut.request == self.requests.len()
                    {
                        self.cut = None;
                        peer.terminate();
                        self.up(skein_io::Event::Stream { owner, up: cut.end() });
                        continue;
                    }
                }
                let up = if matches!(up, Up::End) && peer.reset { Up::Failed(Fault::Reset) } else { up };
                if let Up::Bytes(bytes) = &up {
                    peer.remaining = peer.remaining.saturating_sub(bytes.len());
                }
                self.up(skein_io::Event::Stream { owner, up });
            }
        }
        if self.component.next_deadline().is_some_and(|deadline| deadline <= self.env.now) {
            self.component.fire(&self.env, &mut self.above, &mut self.below);
        }
        while let Some(request) = self.below.pop() {
            self.io(request);
        }
        self.events();
        self.component.reclaim();
    }
    /// Delays the owner taking events while the component and peer continue.
    pub fn hold_output(&mut self) {
        self.hold_output = true;
    }
    /// Takes every output that has reached the owner's queue.
    pub fn release_output(&mut self) {
        self.hold_output = false;
        self.events();
    }
    /// Outputs emitted before each peer request, including output the owner has not taken.
    #[must_use]
    pub fn outputs_before_requests(&self) -> &[u32] {
        &self.outputs_before_requests
    }
    fn events(&mut self) {
        if self.hold_output {
            return;
        }
        while let Some(event) = self.above.pop() {
            if let Event::Connected { forge } = event {
                self.access_event(forge, true);
                self.connected_forges.push(forge);
                continue;
            }
            let name = match &event {
                Event::Installation { .. } => "installation",
                Event::Fact { .. } => "fact",
                Event::GitToken { .. } => "git-token",
                Event::Answered(_) => "answered",
                Event::Stopped { .. } => "stopped",
                Event::Hint(_) => "hint",
                Event::Listening { .. } => "listening",
                Event::ListenerLost => "lost",
                Event::Closed => "closed",
                Event::Connected { .. } => unreachable!("connected taken"),
            };
            self.event_order.push(name);
            match event {
                event @ Event::Installation { .. } => {
                    assert!(self.fake.is_some(), "GitHub fixture");
                    self.installation_events.push(event);
                }
                event @ Event::Fact { .. } => {
                    assert!(self.fake.is_some(), "GitHub fixture");
                    self.facts.push(event);
                }
                event @ Event::GitToken { .. } => {
                    assert!(self.fake.is_some(), "GitHub fixture");
                    self.git_tokens.push(event);
                }
                Event::Answered(answer) => {
                    self.deliveries.push(answer.clone());
                    assert!(self.answers.insert(answer.call.raw(), answer).is_none(), "one answer per call");
                }
                Event::Stopped { forge, why } => {
                    self.access_event(forge, false);
                    self.stopped.push((forge, why));
                }
                Event::Connected { .. } => unreachable!("connected taken"),
                Event::Hint(hint) => {
                    assert!(self.fake.is_some(), "GitHub log fixture");
                    self.hints.push(hint);
                }
                Event::Listening { .. } | Event::ListenerLost => panic!("webhooks absent"),
                Event::Closed => {
                    assert!(!self.closed, "one close");
                    self.closed = true;
                    self.closed_events += 1;
                }
            }
        }
    }
    fn remember_connection_generation(&mut self, owner: Token, addr: skein_io::Addr) {
        if let Some((_, forge)) = self.forge_addresses.iter().find(|(address, _)| *address == addr)
            && let Some(Some(generation)) = self.access_generations.get(forge)
        {
            self.connection_generations.insert(owner, (*forge, generation.number));
        }
    }
    fn io(&mut self, request: skein_io::Request) {
        match request {
            skein_io::Request::Connect { owner, addr } => {
                self.opened += 1;
                self.remember_connection_generation(owner, addr);
                self.peers.insert(
                    owner,
                    Peer::new(
                        self.seed.wrapping_add(owner.raw()),
                        addr,
                        self.tls_addresses.contains(&addr),
                        self.handshake_cut,
                    ),
                );
                self.up(skein_io::Event::Connecting { owner, socket: owner });
                match self.connects.pop_front().unwrap_or(Connect::Made) {
                    Connect::Made => {
                        if self.pause.is_some_and(|(_, pause)| matches!(pause, Pause::Connecting)) {
                            self.paused = Some(owner);
                            return;
                        }
                        self.peers.get_mut(&owner).expect("peer").connected = true;
                        self.up(skein_io::Event::Connected { owner });
                    }
                    Connect::Refused => {
                        self.up(skein_io::Event::Failed { owner, error: skein_io::Error::Refused });
                        self.peers.remove(&owner);
                        self.up(skein_io::Event::Closed { owner });
                    }
                }
            }
            skein_io::Request::Stream { stream, down } => {
                let peer = self.peers.get_mut(&stream).expect("stream peer");
                let sent = match &down {
                    Down::Send(bytes) => bytes.len(),
                    Down::Demand { .. } | Down::Finish => 0,
                };
                let sending = sent > 0;
                let writing = matches!(&down, Down::Demand { room, .. } if *room > 0);
                let plain = peer.take(down);
                self.flight_length = peer.flight_length.or(self.flight_length);
                self.flight_status = peer.flight_status.or(self.flight_status);
                if let Some(reset) = peer.failure.take() {
                    self.up(skein_io::Event::Stream {
                        owner: stream,
                        up: if reset { Up::Failed(Fault::Reset) } else { Up::End },
                    });
                    return;
                }
                if writing
                    && !peer.is_tls()
                    && !peer.handshaking()
                    && let Some(cut) = self.cut
                    && !cut.response
                    && cut.request == self.requests.len()
                    && cut.at == 0
                {
                    self.cut = None;
                    peer.terminate();
                    self.up(skein_io::Event::Stream { owner: stream, up: cut.end() });
                    return;
                }
                if sending && plain > 0 {
                    let ordinal = self.requests.len();
                    self.written.resize(ordinal + 1, 0);
                    self.written[ordinal] += plain;
                    if let Some((at, Pause::Writing(bytes))) = self.pause
                        && at == self.requests.len()
                    {
                        peer.truncate(bytes);
                        self.written[ordinal] = self.written[ordinal].min(bytes);
                        self.paused = Some(stream);
                    }
                    if let Some(cut) = self.cut
                        && !cut.response
                        && cut.request == self.requests.len()
                        && peer.pending_bytes() >= cut.at
                    {
                        peer.truncate(cut.at);
                        self.written[ordinal] = self.written[ordinal].min(cut.at);
                        self.cut = None;
                        peer.terminate();
                        self.up(skein_io::Event::Stream { owner: stream, up: cut.end() });
                    }
                }
            }
            skein_io::Request::Abort { entity } => {
                self.aborts += 1;
                self.end(entity);
            }
            skein_io::Request::Close { entity } => self.end(entity),
            other @ (skein_io::Request::Listen { .. }
            | skein_io::Request::Bind { .. }
            | skein_io::Request::Reject { .. }
            | skein_io::Request::Output { .. }
            | skein_io::Request::Spawn { .. }
            | skein_io::Request::Signal { .. }
            | skein_io::Request::Usage { .. }
            | skein_io::Request::Window { .. }) => panic!("unexpected io {other:?}"),
        }
    }
    fn end(&mut self, owner: Token) {
        if self.hold_settlement && self.peers.contains_key(&owner) {
            assert!(!self.settling.contains(&owner), "one close request per socket");
            self.settling.push(owner);
            self.closes += 1;
            return;
        }
        if self.peers.remove(&owner).is_some() {
            self.closes += 1;
            self.up(skein_io::Event::Closed { owner });
        }
    }
    fn respond(&mut self, owner: Token, request: Vec<u8>) {
        let taken = self.installation_events.len() + self.facts.len() + self.git_tokens.len();
        self.outputs_before_requests.push(u32::try_from(taken).expect("fixture output count") + self.above.len());
        let log_request;
        let (expected, response) = if let Some(answer) = self.script.pop_front() {
            (answer.request, answer.response)
        } else {
            log_request = self.log_script.pop_front().expect("a scripted answer for each request");
            assert!(log_request.starts_with("GET /app/"), "forge log or installation read");
            (log_request.as_str(), Response::Served)
        };
        let line = request.split(|byte| *byte == b'\r').next().expect("request line");
        let asked = line.strip_suffix(b" HTTP/1.1").expect("HTTP version");
        let blob = self.fake.is_some()
            && self.peers.get(&owner).expect("peer").address == skein_io::Addr::Ip(github::BLOB_ADDRESS);
        // The fake checks the signed query; the script names the stable log path.
        let script_path = if blob { asked.split(|byte| *byte == b'?').next().expect("blob path") } else { asked };
        assert_eq!(script_path, expected.as_bytes(), "script request");
        let ordinal = self.requests.len();
        self.requests.push(request);
        let peer = self.peers.get_mut(&owner).expect("peer");
        peer.responding();
        let response = match response {
            Response::Case(case) => Response::Bytes(case_bytes(case)),
            Response::Served => {
                let fake = self.fake.as_mut().expect("Answer::served needs Machine::github");
                let request = self.requests.last().expect("captured request");
                Response::Bytes(if blob {
                    fake.blob(request, self.env.wall)
                } else {
                    fake.serve(request, self.env.wall)
                })
            }
            other @ (Response::Bytes(_) | Response::Reset | Response::Cut { .. } | Response::Silent) => other,
        };
        if let Response::Bytes(bytes) = &response {
            if let Some((at, Pause::Reading(length))) = self.pause
                && at == ordinal
            {
                peer.write(&bytes[..length]);
                self.held_response = Some((owner, bytes[length..].to_vec()));
                return;
            }
            if let Some(cut) = self.cut
                && cut.response
                && cut.request == ordinal
            {
                peer.write(&bytes[..cut.at]);
                peer.wire.eof = true;
                peer.terminate();
                peer.reset = cut.reset;
                self.cut = None;
                return;
            }
        }
        match response {
            Response::Case(_) | Response::Served => unreachable!("converted above"),
            Response::Bytes(bytes) => {
                peer.write(&bytes);
            }
            Response::Cut { case, at } => {
                peer.write(&case_bytes(case)[..at]);
                peer.wire.eof = true;
                peer.terminate();
            }
            Response::Reset => {
                peer.terminate();
                self.up(skein_io::Event::Stream { owner, up: Up::Failed(Fault::Reset) });
            }
            Response::Silent => {}
        }
    }
    /// The call's single terminal answer, if it arrived.
    #[must_use]
    pub fn answered(&self, call: u64) -> Option<&Answered> {
        self.answers.get(&call)
    }
    /// Whether startup stopped, and its reason.
    #[must_use]
    pub fn stopped(&self) -> Option<(u16, Stopped)> {
        self.stopped.last().copied()
    }
    /// Forges whose starts completed, in delivery order.
    #[must_use]
    pub fn connected_forges(&self) -> &[u16] {
        &self.connected_forges
    }
    /// Contents tokens handed to the owner, with Secret redaction.
    #[must_use]
    pub fn git_tokens(&self) -> &[Event] {
        &self.git_tokens
    }
    /// A configured forge's public start phase.
    #[must_use]
    pub fn start(&self, forge: u16) -> Option<skein_forge_connection::start::Start> {
        self.component.start(forge)
    }
    /// Every complete request, in arrival order.
    #[must_use]
    pub fn requests(&self) -> &[Vec<u8>] {
        &self.requests
    }
    /// The number of connect attempts.
    #[must_use]
    pub fn connections_opened(&self) -> u32 {
        self.opened
    }
    /// The number of sockets aborted.
    #[must_use]
    pub fn aborts(&self) -> u32 {
        self.aborts
    }
    /// Whether every scripted answer was assigned to a request.
    #[must_use]
    pub fn script_done(&self) -> bool {
        self.script.is_empty() && self.log_script.is_empty()
    }
    /// Closes the component, allowing at most 1,000 ticks for its terminal.
    pub fn close(&mut self) -> bool {
        self.component.down(&self.env, Request::Close, &mut self.above, &mut self.below);
        for _ in 0..1000 {
            self.tick();
            if self.closed {
                return true;
            }
        }
        false
    }
}
/// Loads an untouched recorded HTTP response, including its headers and framing.
#[must_use]
pub fn case_bytes(case: &str) -> Vec<u8> {
    fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/transcripts").join(case).join("response"))
        .expect("recorded response")
}

impl Cut {
    fn end(self) -> Up {
        if self.reset { Up::Failed(Fault::Reset) } else { Up::End }
    }
}
