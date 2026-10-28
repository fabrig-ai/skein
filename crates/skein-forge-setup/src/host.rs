//! The host component: the plans run inside a service (forge-setup.md, 6.3).
mod boundary;
pub(crate) mod connection;
#[cfg(test)]
#[path = "tests/github_host.rs"]
mod install_tests;
mod limits;
mod memory;
pub use memory::worst_case;
pub(crate) mod run;
mod settings;
#[cfg(test)]
#[path = "tests/github_states.rs"]
mod state_tests;
mod states;
use crate::item::Bounds;
use crate::reconcile::{self, Failure, Reply, Stage, Why};
pub use boundary::{Answer, Event, File, Need, Plan, Request};
use connection::{Connection, Refused};
pub use limits::{Limits, Stacked, Unusable, documents, github, http};
use run::{Run, Send, forgejo_pair};
pub use settings::{Forge, Kind, Settings, Transport};
use skein_lib::{CryptoRng, Env, List, Map, Queue, Secret, Time, Token, bytes};

/// A service's setup plans: one writing run and one Reachable per forge (6.3).
pub struct Host {
    settings: Settings,
    crypto: CryptoRng,
    states: states::States,
    runs: Map<Token, Run>,
    connections: Map<Token, Connection>,
    plan_out: Queue<reconcile::Need>,
    closing_tokens: List<Token>,
    next_token: u64,
    wake: Option<Time>,
    closing: bool,
    ended: bool,
}
impl Host {
    pub fn new(limits: &Limits, settings: Settings, crypto: CryptoRng) -> Result<Self, Unusable> {
        limits::check(limits, &settings)?;
        Ok(Self {
            settings,
            crypto,
            states: states::States::new(limits.states),
            runs: Map::with_capacity(limits.runs),
            connections: Map::with_capacity(limits.runs),
            plan_out: Queue::uncapped(),
            closing_tokens: List::with_capacity(limits.runs),
            next_token: 1,
            wake: None,
            closing: false,
            ended: false,
        })
    }
    pub fn down(
        &mut self,
        env: &Env<Limits>,
        request: Request,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        if self.closing {
            return;
        }
        match request {
            Request::Run { run, plan } => {
                let forge = plan.forge();
                assert!(!self.runs.contains_key(&run), "one live run per owner token");
                let configured = configured(&self.settings, forge);
                if let (Kind::Forgejo(given), Plan::Create { .. }) = (&configured.kind, &plan) {
                    crate::given::check_create(&given.limits, env.limits.string_bytes, &plan)
                        .expect("the owner checks a creation plan before sending it");
                }
                let refused = if configured.takes(&plan) {
                    let mut busy = self.slots() == self.runs.capacity();
                    for (_, live) in &self.runs {
                        busy |= live.forge == forge && live.writes == plan.writes();
                    }
                    if busy { Some(Why::Busy) } else { None }
                } else {
                    Some(Why::Unexpected)
                };
                if let Some(why) = refused {
                    up.push(Event::Stop { run, failure: about(configured, &plan, why) });
                } else {
                    match Run::new(
                        configured,
                        plan,
                        bounds(&env.limits, configured),
                        env,
                        &mut self.crypto,
                        &mut self.states,
                        &mut self.plan_out,
                    ) {
                        Err(failure) => up.push(Event::Stop { run, failure }),
                        Ok(mut live) => match live.pending.take() {
                            Some(reconcile::Need::Stop(failure)) => up.push(Event::Stop { run, failure }),
                            Some(reconcile::Need::Done(outcome)) => up.push(Event::Done { run, outcome }),
                            pending @ (Some(
                                reconcile::Need::Read(_)
                                | reconcile::Need::Write { .. }
                                | reconcile::Need::Send { .. }
                                | reconcile::Need::Draw(_),
                            )
                            | None) => {
                                live.pending = pending;
                                let previous = self.runs.insert(run, live).expect("admitted run");
                                assert!(previous.is_none(), "fresh owner token");
                            }
                        },
                    }
                }
            }
            Request::Answer { run, answer } => {
                if let Some(live) = self.runs.get_mut(&run) {
                    live.answer(answer, &mut self.plan_out);
                }
            }
            Request::Close => {
                self.closing = true;
                self.states.clear();
                self.runs = Map::with_capacity(self.runs.capacity());
                self.closing_tokens.clear();
                for (token, _) in &self.connections {
                    self.closing_tokens.push(*token).expect("bounded connections");
                }
                for token in &self.closing_tokens {
                    self.connections.get_mut(token).expect("owned connection").abort();
                }
            }
        }
        self.drive(env, up, io);
    }
    pub fn up(
        &mut self,
        env: &Env<Limits>,
        event: skein_io::Event,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        match event {
            skein_io::Event::Connecting { owner, socket } => {
                if let Some(c) = self.connections.get_mut(&owner) {
                    c.socket = Some(socket);
                }
            }
            skein_io::Event::Connected { owner } => {
                if let Some(c) = self.connections.get_mut(&owner) {
                    c.connect(env);
                }
            }
            skein_io::Event::Stream { owner, up } => {
                if let Some(c) = self.connections.get_mut(&owner) {
                    c.stream(env, up);
                }
            }
            skein_io::Event::Failed { owner, .. } => {
                if let Some(c) = self.connections.get_mut(&owner) {
                    c.fail();
                }
            }
            skein_io::Event::Closed { owner } => {
                if let Some(c) = self.connections.get_mut(&owner) {
                    c.closed = true;
                }
            }
            skein_io::Event::Listening { .. }
            | skein_io::Event::Accepted { .. }
            | skein_io::Event::Output { .. }
            | skein_io::Event::Spawned { .. }
            | skein_io::Event::Exited { .. }
            | skein_io::Event::Usage { .. }
            | skein_io::Event::Resized
            | skein_io::Event::Window { .. }
            | skein_io::Event::Shutdown { .. } => (),
        }
        self.drive(env, up, io);
    }
    pub fn fire(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        self.drive(env, up, io);
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut earliest = self.wake;
        for (_, run) in &self.runs {
            earlier(&mut earliest, run.deadline);
            if let Some(wait) = run.retry_at
                && wait < run.deadline
            {
                earlier(&mut earliest, wait);
            }
        }
        earliest
    }
    fn drive(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        self.wake = None;
        // The routing pass paces the admitted runs and connections (section 6.3).
        for _ in 0_u32..256 {
            let mut connection = None;
            for (token, c) in &self.connections {
                if c.ready() || c.result.is_some() || c.closed {
                    connection = Some(*token);
                    break;
                }
            }
            if let Some(token) = connection {
                self.drive_connection(token, env, io);
                continue;
            }
            let mut next = None;
            for (token, run) in &self.runs {
                if run.need_pending() || run.deadline <= env.now || self.can_send(run, env.now) {
                    next = Some(*token);
                    break;
                }
            }
            if let Some(token) = next {
                self.drive_run(token, env, up, io);
                continue;
            }
            if self.closing && self.connections.is_empty() && !self.ended {
                self.ended = true;
                up.push(Event::Closed);
            }
            return;
        }
        if self.ready(env.now) {
            self.wake = Some(env.now);
        }
    }
    fn slots(&self) -> u32 {
        let mut slots = self.runs.len();
        for (token, connection) in &self.connections {
            let owned = match self.runs.get(&connection.run) {
                Some(run) => run.connection == Some(*token),
                None => false,
            };
            if !owned {
                slots = slots.checked_add(1).expect("admitted slots");
            }
        }
        slots
    }
    fn can_send(&self, run: &Run, now: Time) -> bool {
        let ready = run.send.is_some()
            && run.connection.is_none()
            && match run.retry_at {
                Some(at) => at <= now,
                None => true,
            }
            && now < run.deadline;
        if ready {
            assert!(self.connections.len() < self.connections.capacity(), "an admitted run owns its connection slot");
        }
        ready
    }
    fn ready(&self, now: Time) -> bool {
        for (_, c) in &self.connections {
            if c.ready() || c.result.is_some() || c.closed {
                return true;
            }
        }
        for (_, run) in &self.runs {
            if run.need_pending() || run.deadline <= now || self.can_send(run, now) {
                return true;
            }
        }
        self.closing && !self.ended && self.connections.is_empty()
    }

    fn drive_connection(&mut self, token: Token, env: &Env<Limits>, io: &mut Queue<skein_io::Request>) {
        let c = self.connections.get_mut(&token).expect("owned connection");
        c.pump(env, io);
        if let Some(reply) = c.result.take()
            && let Some(run) = self.runs.get_mut(&c.run)
            && run.connection == Some(token)
        {
            run.reply(reply, &mut self.plan_out);
        }
        if c.closed && !c.ready() {
            let c = self.connections.remove(&token).expect("settled connection");
            if let Some(run) = self.runs.get_mut(&c.run)
                && run.connection == Some(token)
            {
                run.closed(env.now);
            }
        }
    }
    fn drive_run(&mut self, token: Token, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        let run = self.runs.get_mut(&token).expect("live run");
        if let Some(need) = run.pending.take() {
            match need {
                reconcile::Need::Read(item) => {
                    let need = run.owner_need(Need::Read(item));
                    up.push(Event::Need { run: token, need });
                }
                reconcile::Need::Write { item, bytes } => {
                    let need = run.owner_need(Need::Write { item, bytes });
                    up.push(Event::Need { run: token, need });
                }
                reconcile::Need::Draw(count) => {
                    let mut drawn = bytes::zeroed(usize::try_from(count).expect("draw count fits"));
                    self.crypto.fill(&mut drawn);
                    run.step(reconcile::Event::Drawn(Secret::new(drawn)), &mut self.plan_out);
                }
                reconcile::Need::Send { request, auth } => {
                    run.send = Some(Send { call: request, auth });
                }
                reconcile::Need::Done(outcome) => {
                    up.push(Event::Done { run: token, outcome });
                    let _ended = self.runs.remove(&token);
                }
                reconcile::Need::Stop(failure) => {
                    if let Some((purpose, state)) = run.issued_state.take() {
                        let _taken = self.states.take(run.forge, purpose, &state, env.now);
                    }
                    up.push(Event::Stop { run: token, failure });
                    let _ended = self.runs.remove(&token);
                }
            }
            return;
        }
        if run.deadline <= env.now {
            if let Some(connection) = run.connection
                && let Some(c) = self.connections.get_mut(&connection)
            {
                c.abort();
            }
            if run.send.is_some() {
                run.step(reconcile::Event::Reply(Reply::Down), &mut self.plan_out);
            }
            run.halt(Why::Down, &mut self.plan_out);
            return;
        }
        let forge = configured(&self.settings, run.forge);
        let send = run.send.as_ref().expect("ready send");
        let connection = match open(token, forge, &self.settings.user_agent, send, env) {
            Ok(connection) => connection,
            Err(Refused::Unexpected) => {
                run.halt(Why::Unexpected, &mut self.plan_out);
                return;
            }
            Err(Refused::Unsigned) => {
                run.reply(Reply::Unsigned, &mut self.plan_out);
                return;
            }
        };
        let owner = Token::new(self.next_token);
        self.next_token = self.next_token.checked_add(1).expect("connection tokens never repeat");
        run.connection = Some(owner);
        run.retry_at = None;
        let previous = self.connections.insert(owner, connection).expect("connection room reserved");
        assert!(previous.is_none(), "fresh owner token");
        io.push(skein_io::Request::Connect { owner, addr: forge.address });
    }
}
fn open(run: Token, forge: &Forge, agent: &[u8], send: &Send, env: &Env<Limits>) -> Result<Connection, Refused> {
    // A call's rows stay within the page the host's heap priced (6.5).
    match (connection::row_bound(&send.call), memory::largest_page(&env.limits, &forge.kind)) {
        (Some(rows), Some(priced)) if rows <= priced => {}
        _ => return Err(Refused::Unexpected),
    }
    match &forge.kind {
        Kind::Forgejo(_) => {
            let (request, credential) =
                forgejo_pair(send.call.clone(), send.auth.clone()).or(Err(Refused::Unexpected))?;
            Connection::new(run, forge, agent, &request.into(), &credential, &env.limits).ok_or(Refused::Unexpected)
        }
        Kind::GitHub(_) => match &send.call {
            crate::call::Call::GitHub(_) => {
                Connection::github(run, forge, agent, &send.call, &send.auth, env.wall, &env.limits)
            }
            crate::call::Call::Forgejo(_) => Err(Refused::Unexpected),
        },
    }
}
fn earlier(earliest: &mut Option<Time>, time: Time) {
    if match *earliest {
        Some(before) => time < before,
        None => true,
    } {
        *earliest = Some(time);
    }
}
impl core::fmt::Debug for Host {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Host")
            .field("runs", &self.runs.len())
            .field("connections", &self.connections.len())
            .field("closing", &self.closing)
            .finish_non_exhaustive()
    }
}

fn configured(settings: &Settings, number: u16) -> &Forge {
    for forge in &settings.forges {
        if forge.number() == number {
            return forge;
        }
    }
    unreachable!("owner names a configured forge")
}

pub(super) fn bounds(env_limits: &Limits, forge: &Forge) -> Bounds {
    match &forge.kind {
        Kind::Forgejo(_) => {
            Bounds { token_bytes: env_limits.token_bytes, name_bytes: 0, key_bytes: 0, installations: 0 }
        }
        Kind::GitHub(given) => Bounds {
            token_bytes: env_limits.token_bytes,
            name_bytes: given.limits.name_bytes,
            key_bytes: given.limits.key_bytes,
            installations: given.limits.installations,
        },
    }
}

/// A host failure is about a Forgejo's admin or a GitHub App, naming the plan.
pub(super) fn about(forge: &Forge, plan: &Plan, why: Why) -> Failure {
    match &forge.kind {
        Kind::Forgejo(given) => {
            Failure { forge: given.forge, step: Stage::Admin, subject: given.admin_name.clone(), why }
        }
        Kind::GitHub(given) => Failure { forge: given.forge, step: Stage::App, subject: plan.name().into(), why },
    }
}

#[cfg(test)]
#[path = "host/tests.rs"]
mod memory_tests;
