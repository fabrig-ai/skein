//! Waiting calls and returned answers (forge-connection.md, sections 2 and 8).
use crate::{
    boundary::{Answered, Event, Request},
    connection::{self, Connection, Exchange, Phase},
    limits::{self, Limits, Shared, Unusable},
    settings::Settings,
    start,
    start::Start,
};
use alloc::boxed::Box;
use skein_lib::{Env, Id, List, Map, Queue, Slab, Time, Token};
mod calls;
mod github;
mod github_calls;
mod github_logs;
mod puller;
use skein_forge::api;
use skein_forge::forgejo::plans::{
    Body, Heard,
    plan::{Plan, Step},
    start::Read,
    tables::Tables,
};
/// Malformed answers and long bodies counted by the component.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Diagnostics {
    pub long_bodies: u64,
    pub malformed: u64,
}
#[derive(Debug)]
pub(crate) enum CallPlan {
    Forgejo(Plan),
    GitHub(skein_forge::github::plans::plan::Plan),
}
#[derive(Debug)]
pub(crate) enum OwnPlan {
    Forgejo(skein_forge::forgejo::plans::start::Plan),
    GitHubInstallations(skein_forge::github::plans::start::Installations),
    GitHubMinting(skein_forge::github::plans::start::Minting),
    GitHubIdentity(skein_forge::github::plans::start::Identity),
    GitHubLogPage(Box<skein_forge::github::plans::start::LogPage>),
    GitHubLogDelivery(Box<skein_forge::github::plans::start::LogDelivery>),
}
#[derive(Debug)]
pub(crate) struct Waiting {
    pub repository: api::Repository,
    pub op: Option<api::Op>,
    pub plan: Option<CallPlan>,
    pub route: Option<github_calls::Route>,
    pub discovery: Option<github_calls::Discovery>,
    pub discovered: Option<api::Answer>,
    pub page_size: u32,
    pub connection: Option<Token>,
    pub cost: u32,
    pub free: u32,
    pub rate: Option<api::Rate>,
    pub charged: bool,
    pub revoked: bool,
    pub deadline: Time,
}
impl Waiting {
    fn new(repository: api::Repository, op: api::Op, deadline: Time) -> Self {
        Self {
            repository,
            op: Some(op),
            plan: None,
            route: None,
            discovery: None,
            discovered: None,
            page_size: 1,
            connection: None,
            cost: 0,
            free: 0,
            rate: None,
            charged: false,
            revoked: false,
            deadline,
        }
    }
    fn origin(&self) -> Option<u32> {
        match &self.route {
            Some(route) => route.origin,
            None => None,
        }
    }
    fn answered_head(&mut self, status: u16, rate: Option<api::Rate>) {
        if status == 304 {
            self.free = self.free.checked_add(1).expect("bounded exchanges");
        }
        if let Some(rate) = rate {
            self.rate = Some(rate);
        }
    }
}
#[derive(Debug)]
pub(crate) struct Held {
    pub answer: Answered,
    pub offer: bool,
}
/// One component per deployment, owning every configured token and start.
#[derive(Debug)]
pub struct Component {
    hooks: crate::hooks::Hooks,
    settings: Settings,
    starts: List<Start>,
    github: List<Option<Box<crate::installations::GitHub>>>,
    forgejo_tables: Tables,
    github_tables: skein_forge::github::plans::tables::Tables,
    waiting: Slab<Waiting>,
    calls: Map<Token, Id<Waiting>>,
    held: Map<Token, Held>,
    holders: Map<Token, Token>,
    closed: bool,
    diagnostics: Diagnostics,
    shared: Shared,
    connections: Map<Token, Id<Connection>>,
    connection_slab: Slab<Connection>,
    own_tokens: List<Option<Token>>,
    own_plans: List<Option<OwnPlan>>,
    next_token: u64,
    wake: Option<Time>,
    ended: bool,
}
impl Component {
    pub fn new(settings: Settings, limits: &Limits, shared: &Shared) -> Result<Self, Unusable> {
        limits::check(limits, shared, &settings)?;
        let mut github = List::with_capacity(limits.forges);
        let mut reserved = 0_u32;
        let mut starts = List::with_capacity(limits.forges);
        let mut own_tokens = List::with_capacity(limits.forges);
        let mut own_plans = List::with_capacity(limits.forges);
        for forge in &settings.forges {
            let state =
                if forge.access.is_some() { initial(forge.kind, limits.start_backoff) } else { Start::Unconnected };
            let state_github = match forge.kind {
                crate::settings::Kind::Forgejo => None,
                crate::settings::Kind::GitHub => {
                    reserved = reserved.checked_add(1).expect("checked connection count");
                    let mut github = crate::installations::GitHub::new(limits.installations);
                    github.said_origins = Queue::with_capacity(limits.log_origins);
                    Some(Box::new(github))
                }
            };
            github.push(state_github).expect("checked forge count");
            starts.push(state).expect("checked forge count");
            own_tokens.push(None).expect("checked forge count");
            own_plans.push(None).expect("checked forge count");
        }
        let mut forgejo = false;
        let mut has_github = false;
        for forge in &settings.forges {
            match forge.kind {
                crate::settings::Kind::Forgejo => forgejo = true,
                crate::settings::Kind::GitHub => has_github = true,
            }
        }
        Ok(Self {
            hooks: crate::hooks::Hooks::new(limits),
            settings,
            starts,
            github,
            forgejo_tables: Tables::new(
                if forgejo { limits.repositories } else { 0 },
                if forgejo { limits.users } else { 0 },
            ),
            github_tables: skein_forge::github::plans::tables::Tables::new(
                if has_github { limits.repositories } else { 0 },
                if has_github { limits.users } else { 0 },
            ),
            waiting: Slab::with_capacity(shared.calls),
            calls: Map::with_capacity(shared.calls),
            held: Map::with_capacity(shared.calls),
            holders: Map::with_capacity(shared.calls),
            closed: false,
            diagnostics: Diagnostics { long_bodies: 0, malformed: 0 },
            shared: *shared,
            connections: Map::with_capacity(limits.connections.checked_add(reserved).expect("checked connections")),
            connection_slab: Slab::with_capacity(
                limits.connections.checked_add(reserved).expect("checked connections"),
            ),
            own_tokens,
            own_plans,
            next_token: 1,
            wake: None,
            ended: false,
        })
    }
    #[cfg(test)]
    pub(crate) fn test_calls_ready(&mut self) {
        *self.starts.get_mut(0).expect("fixture start") = Start::Ready { cap: skein_forge::github::pin::PAGE_CAP };
        self.settings.forges.get_mut(0).expect("forge").access = Some(crate::tests::fixtures::app_access(
            skein_fake_forge_github::keys::KEYS.first().expect("test key"),
            7,
            9,
        ));
        let gh = self.test_github_mut();
        gh.connected = true;
        gh.identity_checked = true;
        gh.baseline = false;
    }
    #[cfg(test)]
    pub(crate) fn test_github(&self) -> &crate::installations::GitHub {
        self.github.get(0).expect("fixture forge").as_ref().expect("GitHub fixture")
    }
    #[cfg(test)]
    pub(crate) fn test_github_mut(&mut self) -> &mut crate::installations::GitHub {
        self.github.get_mut(0).expect("fixture forge").as_mut().expect("GitHub fixture")
    }
    #[cfg(test)]
    pub(crate) fn test_local(&mut self, env: &Env<Limits>, up: &mut Queue<Event>) {
        let before = up.len();
        for _ in 0..64 {
            if !self.github_local(0, env, up) || up.len() != before {
                break;
            }
        }
    }
    /// Opens each forge's start within its connection slots.
    pub fn begin(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        if self.closed {
            return;
        }
        if let Some(settings) = &self.settings.webhooks {
            self.hooks.begin(settings, io);
        }
        for at in 0..self.starts.len() {
            if self.settings.forges.get(at).expect("configured forge").access.is_some() {
                *self.starts.get_mut(at).expect("configured start") =
                    initial(self.settings.forges.get(at).expect("forge").kind, env.limits.start_backoff);
            }
        }
        self.drive(env, up, io);
    }
    pub fn down(
        &mut self,
        env: &Env<Limits>,
        request: Request,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        if self.closed {
            return;
        }
        match request {
            Request::Connect { forge, access } => {
                self.connect(env, forge, access, up, io);
                self.drive(env, up, io);
            }
            Request::Installations { forge, accepted } => {
                self.installations(forge, &accepted);
                self.drive(env, up, io);
            }
            Request::Call { call, repository, op } => {
                assert!(!self.calls.contains_key(&call), "one live call per token");
                let id = self
                    .waiting
                    .insert(Waiting::new(repository, op, env.now.saturating_add(env.limits.deadline)))
                    .expect("client bounds admitted calls");
                let _previous: Option<Id<Waiting>> = self.calls.insert(call, id).expect("call index matches its slab");
                self.drive(env, up, io);
            }
            Request::RefusedHint(hint) => self.refused_hint(hint),
            Request::Refused(answer) => {
                drop(
                    self.held
                        .insert(answer.call, Held { answer, offer: true })
                        .expect("at most one held answer per client call"),
                );
            }
            Request::Returned(answer) => {
                drop(
                    self.held
                        .insert(answer.call, Held { answer, offer: false })
                        .expect("at most one held answer per client call"),
                );
            }
            Request::Offer => {
                self.hooks.offer();
                self.offer_pullers();
                if self.settings.webhooks.is_some() {
                    self.hooks.drive(env, &self.settings, up, io);
                }
                for _ in 0..self.held.len() {
                    let mut next = None;
                    for (call, held) in &self.held {
                        if held.offer {
                            next = Some(*call);
                            break;
                        }
                    }
                    let Some(call) = next else {
                        break;
                    };
                    if let Some(held) = self.held.remove(&call) {
                        up.push(Event::Answered(held.answer));
                    }
                }
                self.drive(env, up, io);
            }
            Request::Close => {
                for _ in 0..self.calls.len() {
                    let Some((call, id)) = self.calls.first() else {
                        break;
                    };
                    let (call, id) = (*call, *id);
                    self.waiting.retire(id);
                    let _removed: Option<Id<Waiting>> = self.calls.remove(&call);
                }
                self.held = Map::with_capacity(self.held.capacity());
                self.holders = Map::with_capacity(self.holders.capacity());
                for at in 0..self.starts.len() {
                    self.settings.forges.get_mut(at).expect("forge").access = None;
                    if let Some(github) = self.github.get_mut(at).expect("forge slot") {
                        github.forget();
                    }
                    *self.own_plans.get_mut(at).expect("forge plan") = None;
                }
                self.closed = true;
                self.hooks.close();
                self.drive(env, up, io);
            }
        }
    }
    fn offer_pullers(&mut self) {
        for at in 0..self.github.len() {
            if let Some(gh) = self.github.get_mut(at).expect("forge slot") {
                gh.puller.offer();
            }
        }
    }
    fn refused_hint(&mut self, hint: crate::hooks::Hinted) {
        let mut puller = None;
        for at in 0..self.github.len() {
            let source = match self.github.get(at).expect("forge slot") {
                Some(gh) => gh.puller.source,
                None => None,
            };
            if source == Some(hint.connection)
                && hint.hint.repository.forge == self.settings.forges.get(at).expect("forge").number
            {
                puller = Some(at);
                break;
            }
        }
        if let Some(at) = puller {
            if let Some(gh) = self.github.get_mut(at).expect("forge slot") {
                gh.puller.refused(hint.hint);
            }
        } else {
            self.hooks.refused(hint);
        }
    }
    fn installations(&mut self, forge: u16, accepted: &[u64]) {
        for at in 0..self.settings.forges.len() {
            if self.settings.forges.get(at).expect("forge").number == forge
                && let Some(github) = self.github.get_mut(at).expect("forge slot")
            {
                github.accept(accepted);
            }
        }
    }
    fn connect(
        &mut self,
        env: &Env<Limits>,
        number: u16,
        access: crate::Access,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        let mut position = None;
        for at in 0..self.settings.forges.len() {
            if self.settings.forges.get(at).expect("configured forge").number == number {
                position = Some(at);
                break;
            }
        }
        let Some(at) = position else {
            return;
        };
        let forge = self.settings.forges.get_mut(at).expect("configured forge");
        let mut checked = limits::access(&env.limits, forge.kind, self.settings.webhooks.is_some(), &access);
        if checked.is_ok()
            && let Some(held) = &forge.access
            && held.user != access.user
        {
            checked = Err(crate::Value::User);
        }
        for (_, id) in &self.calls {
            let call = self.waiting.get_mut(*id).expect("indexed call");
            if call.repository.forge == number && (call.plan.is_some() || call.cost > 0) {
                call.revoked = true;
            }
        }
        if let Some(token) = self.own_tokens.get_mut(at).expect("configured slot").take()
            && let Some(id) = self.connections.get(&token)
        {
            let c = self.connection_slab.get_mut(*id).expect("start connection");
            c.close(c.in_flight(), io);
        }
        *self.own_plans.get_mut(at).expect("configured plan") = None;
        if let Some(github) = self.github.get_mut(at).expect("forge slot") {
            github.forget();
        }
        match checked {
            Ok(()) => {
                forge.access = Some(access);
                *self.starts.get_mut(at).expect("configured start") = initial(forge.kind, env.limits.start_backoff);
            }
            Err(value) => {
                forge.access = None;
                let why = crate::Stopped::Invalid { value };
                *self.starts.get_mut(at).expect("configured start") = Start::Stopped(why);
                up.push(Event::Stopped { forge: number, why });
            }
        }
    }
    /// Routes io by this component's connection owner; stale owners are inert.
    pub fn up(
        &mut self,
        env: &Env<Limits>,
        event: skein_io::Event,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        let owner = match &event {
            skein_io::Event::Listening { owner, .. }
            | skein_io::Event::Accepted { owner, .. }
            | skein_io::Event::Stream { owner, .. }
            | skein_io::Event::Failed { owner, .. }
            | skein_io::Event::Closed { owner } => Some(*owner),
            skein_io::Event::Connecting { .. }
            | skein_io::Event::Connected { .. }
            | skein_io::Event::Output { .. }
            | skein_io::Event::Spawned { .. }
            | skein_io::Event::Exited { .. }
            | skein_io::Event::Usage { .. }
            | skein_io::Event::Resized
            | skein_io::Event::Window { .. }
            | skein_io::Event::Shutdown { .. } => None,
        };
        if let Some(owner) = owner
            && self.hooks.owns(owner)
        {
            self.hooks.up(env, event, up);
            self.drive(env, up, io);
            return;
        }
        match event {
            skein_io::Event::Connecting { owner, socket } => {
                if let Some(id) = self.connections.get(&owner)
                    && let Some(c) = self.connection_slab.get_mut(*id)
                {
                    c.socket = Some(socket);
                }
            }
            skein_io::Event::Connected { owner } => {
                if let Some(id) = self.connections.get(&owner)
                    && let Some(c) = self.connection_slab.get_mut(*id)
                    && !c.closing
                {
                    c.connect(env);
                }
            }
            skein_io::Event::Stream { owner, up } => {
                if let Some(id) = self.connections.get(&owner)
                    && let Some(c) = self.connection_slab.get_mut(*id)
                {
                    c.stream(env, up);
                }
            }
            skein_io::Event::Failed { owner, .. } => {
                if let Some(id) = self.connections.get(&owner)
                    && let Some(c) = self.connection_slab.get_mut(*id)
                {
                    c.failed(connection::Failure::Unseen);
                }
            }
            skein_io::Event::Closed { owner } => {
                if let Some(id) = self.connections.remove(&owner) {
                    let closing = self.connection_slab.get(id).expect("indexed connection").closing;
                    if let Some(call) = self.holders.remove(&owner)
                        && !self.closed
                    {
                        let call_id = *self.calls.get(&call).expect("holding call");
                        let c = self.connection_slab.get(id).expect("closed connection");
                        let error = if c.written { api::Error::Timeout } else { api::Error::Unavailable };
                        self.finish(call, call_id, Err(error), up);
                    }
                    self.connection_slab.retire(id);
                    self.own_closed(owner, closing, env, up);
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
            | skein_io::Event::Shutdown { .. } => {}
        }
        self.drive(env, up, io);
    }
    fn own_closed(&mut self, owner: Token, closing: bool, env: &Env<Limits>, up: &mut Queue<Event>) {
        for at in 0..self.own_tokens.len() {
            if self.own_tokens.get(at) == Some(&Some(owner)) {
                *self.own_tokens.get_mut(at).expect("configured forge") = None;
                if self.settings.forges.get(at).expect("forge").kind == crate::settings::Kind::Forgejo {
                    *self.own_plans.get_mut(at).expect("configured forge") = None;
                }
                if !self.closed && !closing {
                    match self.settings.forges.get(at).expect("forge").kind {
                        crate::settings::Kind::Forgejo => {
                            self.start_heard(at, skein_forge::forgejo::plans::start::Heard::Unanswered, env, up);
                        }
                        crate::settings::Kind::GitHub => self.github_wait(at, env),
                    }
                }
            }
        }
    }
    pub fn fire(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        if self.closed {
            self.drive(env, up, io);
            return;
        }
        for _ in 0..self.calls.len() {
            let mut due = None;
            for (call, id) in &self.calls {
                if self.waiting.get(*id).expect("live call").deadline <= env.now {
                    due = Some((*call, *id));
                    break;
                }
            }
            let Some((call, id)) = due else {
                break;
            };
            self.expire(call, id, up, io);
        }
        self.drive(env, up, io);
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        let mut earliest = self.wake;
        if let Some(time) = self.hooks.deadline()
            && (earliest.is_none() || Some(time) < earliest)
        {
            earliest = Some(time);
        }
        for slot in &self.github {
            if let Some(gh) = slot
                && let Some(time) = gh.puller.due
                && (earliest.is_none() || Some(time) < earliest)
            {
                earliest = Some(time);
            }
            if let Some(gh) = slot
                && let Some(time) = gh.deadline()
                && (earliest.is_none() || Some(time) < earliest)
            {
                earliest = Some(time);
            }
        }
        for (_, id) in &self.connections {
            let c = self.connection_slab.get(*id).expect("indexed connection");
            if let Some(time) = c.deadline
                && (earliest.is_none() || Some(time) < earliest)
            {
                earliest = Some(time);
            }
        }
        if !self.closed {
            for state in &self.starts {
                if let Some((until, _)) = backoff(*state)
                    && (earliest.is_none() || Some(until) < earliest)
                {
                    earliest = Some(until);
                }
            }
        }
        for (_, id) in &self.calls {
            let time = self.waiting.get(*id).expect("indexed live call").deadline;
            if earliest.is_none() || Some(time) < earliest {
                earliest = Some(time);
            }
        }
        earliest
    }
    fn start_heard(
        &mut self,
        at: u32,
        heard: skein_forge::forgejo::plans::start::Heard,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
    ) {
        let forge = self.settings.forges.get_mut(at).expect("configured forge");
        let state = self.starts.get_mut(at).expect("configured start");
        let previous = *state;
        *state = start::heard(previous, heard, forge.access.as_ref().expect("held access").user, env.now, &env.limits);
        if start_read(*state) == Some(Read::ApiSettings)
            && *state != previous
            && let Some(Some(token)) = self.own_tokens.get(at)
            && let Some(id) = self.connections.get(token)
            && let Some(c) = self.connection_slab.get_mut(*id)
        {
            c.deadline = Some(env.now.saturating_add(env.limits.deadline));
        }
        if let Some(reason) = stopped(*state)
            && *state != previous
        {
            forge.access = None;
            up.push(Event::Stopped { forge: forge.number, why: reason });
        } else if ready(*state) && *state != previous {
            up.push(Event::Connected { forge: forge.number });
        }
    }
    fn drive_own(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        self.wake = None;
        for at in 0..self.starts.len() {
            if self.settings.forges.get(at).expect("forge").kind == crate::settings::Kind::GitHub {
                self.drive_github(at, env, up, io);
                continue;
            }
            self.drive_forgejo(at, env, up, io);
        }
    }
    fn drive_forgejo(&mut self, at: u32, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        let state = self.starts.get_mut(at).expect("configured start");
        if let Some((until, next)) = backoff(*state)
            && until <= env.now
            && !self.closed
        {
            *state = Start::Checking { next };
        }
        let mut token = *self.own_tokens.get(at).expect("configured slot");
        if !self.closed
            && token.is_none()
            && start_read(*state).is_some()
            && self.call_connections() < env.limits.connections
            && self.connections.len() < self.connections.capacity()
        {
            token = Some(self.open_own(at, env, io));
        }
        let Some(token) = token else {
            return;
        };
        for _ in 0..64_u32 {
            let c = self
                .connection_slab
                .get_mut(*self.connections.get(&token).expect("indexed token"))
                .expect("indexed connection");
            if self.closed {
                *self.own_plans.get_mut(at).expect("configured forge") = None;
                c.close(c.in_flight(), io);
                break;
            }
            let state = *self.starts.get(at).expect("configured start");
            if let Some(deadline) = c.deadline
                && deadline <= env.now
            {
                if c.phase == Phase::Idle && ready(state) {
                    c.close(false, io);
                    break;
                }
                c.timeout();
            }
            if c.phase == Phase::Idle
                && let Some(read) = start_read(state)
            {
                let holder = self.own_plans.get_mut(at).expect("configured forge");
                if holder.is_none() {
                    *holder = Some(OwnPlan::Forgejo(skein_forge::forgejo::plans::start::Plan::new(read)));
                }
                let plan = holder.as_mut().expect("active start read");
                let forge = self.settings.forges.get(at).expect("configured forge");
                send_start(c, env, forge, &self.settings.user_agent, plan);
            }
            c.pump(env, io);
            if let Some(heard) = c.heard() {
                let heard = match heard {
                    connection::Heard::Forgejo(heard) => heard,
                    connection::Heard::GitHub(_) => unreachable!("Forgejo start has a Forgejo connection"),
                };
                let plan = self.own_plans.get_mut(at).expect("configured forge");
                let step = match plan {
                    Some(OwnPlan::Forgejo(plan)) => plan.heard(heard),
                    Some(
                        OwnPlan::GitHubInstallations(_)
                        | OwnPlan::GitHubMinting(_)
                        | OwnPlan::GitHubIdentity(_)
                        | OwnPlan::GitHubLogPage(_)
                        | OwnPlan::GitHubLogDelivery(_),
                    ) => {
                        unreachable!("Forgejo own plan")
                    }
                    None => None,
                };
                if let Some(skein_forge::forgejo::plans::start::Step::Done(heard)) = step {
                    *plan = None;
                    self.start_heard(at, heard, env, up);
                }
                let c = self
                    .connection_slab
                    .get_mut(*self.connections.get(&token).expect("indexed token"))
                    .expect("indexed connection");
                if backoff(*self.starts.get(at).expect("configured start")).is_some()
                    || stopped(*self.starts.get(at).expect("configured start")).is_some()
                {
                    c.close(true, io);
                }
                continue;
            }
            if c.phase == Phase::Closed {
                c.close(c.in_flight(), io);
            }
            if !c.ready() {
                break;
            }
            self.wake = Some(env.now);
        }
    }
    fn drive(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<skein_io::Request>) {
        if self.settings.webhooks.is_some() {
            self.hooks.drive(env, &self.settings, up, io);
        }
        self.drive_own(env, up, io);
        self.drive_calls(env, up, io);
        if self.closed && self.connections.is_empty() && self.hooks.closed() && !self.ended {
            self.ended = true;
            up.push(Event::Closed);
        }
    }
    fn call_connections(&self) -> u32 {
        let mut count = 0_u32;
        for (owner, _) in &self.connections {
            let mut reserved = false;
            for at in 0..self.own_tokens.len() {
                if self.own_tokens.get(at) == Some(&Some(*owner))
                    && self.settings.forges.get(at).expect("forge").kind == crate::settings::Kind::GitHub
                {
                    reserved = true;
                }
            }
            if !reserved {
                count = count.checked_add(1).expect("bounded connections");
            }
        }
        count
    }
    fn open_call(&mut self, at: u32, env: &Env<Limits>, io: &mut Queue<skein_io::Request>) -> Token {
        self.open_call_origin(at, None, env, io)
    }
    fn open_call_origin(
        &mut self,
        at: u32,
        origin: Option<u32>,
        env: &Env<Limits>,
        io: &mut Queue<skein_io::Request>,
    ) -> Token {
        assert!(self.next_token < 1_u64.checked_shl(59).expect("token prefix"), "connection token prefix");
        let owner = Token::new(self.next_token);
        self.next_token = self.next_token.checked_add(1).expect("connection token space");
        let forge = self.settings.forges.get(at).expect("configured forge");
        self.connection_slab.reclaim();
        let id = self
            .connection_slab
            .insert(Connection::at_origin(
                forge,
                origin,
                &env.limits,
                self.shared,
                env.now.saturating_add(env.limits.deadline),
            ))
            .expect("free connection slot");
        let _previous = self.connections.insert(owner, id).expect("connection index has room");
        let address = match origin {
            Some(at) => forge.log_origins.get(at).expect("origin").address,
            None => forge.address,
        };
        io.push(skein_io::Request::Connect { owner, addr: address });
        owner
    }
    fn open_own(&mut self, at: u32, env: &Env<Limits>, io: &mut Queue<skein_io::Request>) -> Token {
        let owner = self.open_call(at, env, io);
        *self.own_tokens.get_mut(at).expect("configured slot") = Some(owner);
        let plan = self.own_plans.get_mut(at).expect("configured forge");
        if self.settings.forges.get(at).expect("forge").kind == crate::settings::Kind::Forgejo {
            *plan = None;
        }
        if let Some(read) = start_read(*self.starts.get(at).expect("configured start")) {
            *plan = Some(OwnPlan::Forgejo(skein_forge::forgejo::plans::start::Plan::new(read)));
        }
        owner
    }
    #[must_use]
    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics
    }
    pub fn reclaim(&mut self) {
        self.waiting.reclaim();
        self.connection_slab.reclaim();
    }
    /// The bounded resolution tables owned by this component.
    pub fn forgejo_tables(&mut self) -> &mut Tables {
        &mut self.forgejo_tables
    }
    /// GitHub repository owners and user logins, shared by its configured forges.
    pub fn github_tables(&mut self) -> &mut skein_forge::github::plans::tables::Tables {
        &mut self.github_tables
    }
    /// The pure start, in configured forge order.
    #[must_use]
    pub fn start(&self, forge: u16) -> Option<Start> {
        for (at, settings) in self.settings.forges.as_slice().iter().enumerate() {
            if settings.number == forge {
                return self.starts.as_slice().get(at).copied();
            }
        }
        None
    }
}

fn send_start(
    connection: &mut Connection,
    env: &Env<Limits>,
    forge: &crate::settings::Forge,
    user_agent: &[u8],
    plan: &mut OwnPlan,
) {
    let step = match plan {
        OwnPlan::Forgejo(plan) => plan.begin(),
        OwnPlan::GitHubInstallations(_)
        | OwnPlan::GitHubMinting(_)
        | OwnPlan::GitHubIdentity(_)
        | OwnPlan::GitHubLogPage(_)
        | OwnPlan::GitHubLogDelivery(_) => {
            unreachable!("Forgejo sender")
        }
    };
    let skein_forge::forgejo::plans::start::Step::Send(exchange) = step else {
        unreachable!("a new start plan sends its read");
    };
    let authorization =
        skein_forge::forgejo::credential::authorization(&forge.access.as_ref().expect("held access").credential)
            .expect("checked credential length");
    connection
        .start(
            env,
            forge,
            user_agent,
            Exchange::Forgejo { request: exchange.request, range: exchange.range, authorization },
        )
        .expect("checked start request");
}

fn start_read(state: Start) -> Option<Read> {
    match state {
        Start::Checking { .. } => Some(Read::User),
        Start::Settings { .. } => Some(Read::ApiSettings),
        Start::Listing { .. }
        | Start::Minting { .. }
        | Start::Identity { .. }
        | Start::Unconnected
        | Start::Waiting { .. }
        | Start::Ready { .. }
        | Start::Stopped(_) => None,
    }
}
fn backoff(state: Start) -> Option<(Time, skein_lib::Duration)> {
    match state {
        Start::Waiting { until, next } => Some((until, next)),
        Start::Listing { .. }
        | Start::Minting { .. }
        | Start::Identity { .. }
        | Start::Unconnected
        | Start::Checking { .. }
        | Start::Settings { .. }
        | Start::Ready { .. }
        | Start::Stopped(_) => None,
    }
}
fn stopped(state: Start) -> Option<crate::Stopped> {
    match state {
        Start::Stopped(reason) => Some(reason),
        Start::Listing { .. }
        | Start::Minting { .. }
        | Start::Identity { .. }
        | Start::Unconnected
        | Start::Checking { .. }
        | Start::Settings { .. }
        | Start::Waiting { .. }
        | Start::Ready { .. } => None,
    }
}
fn ready(state: Start) -> bool {
    match state {
        Start::Ready { .. } => true,
        Start::Listing { .. }
        | Start::Minting { .. }
        | Start::Identity { .. }
        | Start::Unconnected
        | Start::Checking { .. }
        | Start::Settings { .. }
        | Start::Waiting { .. }
        | Start::Stopped(_) => false,
    }
}

#[cfg(test)]
mod tests;

fn initial(kind: crate::settings::Kind, next: skein_lib::Duration) -> Start {
    match kind {
        crate::settings::Kind::Forgejo => Start::Checking { next },
        crate::settings::Kind::GitHub => Start::Listing { next },
    }
}

#[cfg(test)]
impl Component {
    pub(crate) fn test_github_handoffs(&self) -> u32 {
        let mut count = 0_u32;
        for (_, id) in &self.connections {
            if self.connection_slab.get(*id).expect("indexed connection").test_github_handoff() {
                count = count.checked_add(1).expect("bounded connections");
            }
        }
        count
    }
}
