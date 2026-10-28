//! Forge client entry points (forge-client.md, sections 5–6).
//!
//! `Domain` keeps calls, live resources, deadlines and active write lanes.
//! It never knows authorization, subscribers or the top's durable entries.
//! `step` changes state from a parent event; `fire` expires one deadline;
//! `resume` sends one eligible call. The parent commits emitted progress
//! before releasing any call from that decision.
use crate::{Event, Fact, Limits, Priority, Request, Resource, Stored, Writer, outbox};
use crate::{bounds, calls, keep};
use alloc::boxed::Box;
use skein_forge::api;
use skein_lib::{Deadlines, Env, Id, Map, Queue, Rng, Time};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) enum Alarm {
    Window,
    Reset,
    ReadRetry(Id<calls::Call>),
    Entry(u64),
    Resource(u64),
    Repository(api::Repository),
}
/// The forge client’s bounded working set and call scheduler.
#[derive(Debug)]
pub struct Domain {
    pub(crate) config: Option<crate::Config>,
    pub(crate) writers: Map<u16, u64>,
    pub(crate) calls: calls::Calls,
    pub(crate) alarms: Deadlines<Alarm>,
    pub(crate) rng: Rng,
    pub(crate) outbox: outbox::Outbox,
    pub(crate) keep: keep::Keep,
    identity_used: bool,
    facts: Queue<Fact>,
    lost: u64,
}
impl Domain {
    #[must_use]
    pub fn new(l: &Limits) -> Domain {
        Self::seeded(l, 0)
    }
    #[must_use]
    pub fn seeded(l: &Limits, seed: u64) -> Domain {
        Self::construct(l, seed, None)
    }
    /// Root configuration comes from its durable deployment identity and the
    /// authenticated writer account used for each forge.
    pub fn configured(l: &Limits, seed: u64, config: crate::Config) -> Result<Domain, api::Error> {
        if !crate::identity::valid(&config, l) {
            return Err(api::Error::TooLarge);
        }
        Ok(Self::construct(l, seed, Some(config)))
    }
    /// Rebind the key namespace to the deployment ID loaded from the store,
    /// before restored entries or live resources are handed to the client.
    pub fn bind_deployment(&mut self, id: [u8; 16], limits: &Limits) -> bool {
        if self.identity_used {
            return false;
        }
        let Some(config) = &mut self.config else {
            return false;
        };
        let prior = core::mem::replace(&mut config.namespace, Box::from(id));
        if crate::identity::valid(config, limits) {
            true
        } else {
            config.namespace = prior;
            false
        }
    }
    fn construct(l: &Limits, seed: u64, config: Option<crate::Config>) -> Domain {
        assert!(crate::valid(l), "forge client limits are valid");
        let alarms = l.pending.checked_add(l.entries).expect("validated alarm capacity");
        let alarms = alarms.checked_add(l.resources).expect("validated alarm capacity");
        let alarms = alarms.checked_add(l.repositories).expect("validated alarm capacity");
        let alarms = alarms.checked_add(2).expect("validated alarm capacity");
        let mut writers = Map::with_capacity(l.repositories);
        if let Some(config) = &config {
            for writer in &config.writers {
                writers.insert(writer.forge, writer.author).expect("configured writers fit");
            }
        }
        Domain {
            config,
            writers,
            identity_used: false,
            calls: calls::Calls::new(l),
            alarms: Deadlines::with_capacity(alarms),
            rng: Rng::new(seed),
            outbox: outbox::Outbox::new(l),
            keep: keep::Keep::new(l),
            facts: Queue::with_capacity(l.facts),
            lost: 0,
        }
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        match (self.alarms.next(), outbox::stop::deadline(&self.outbox)) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, None) => left,
            (None, right) => right,
        }
    }
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.next_deadline() {
            Some(at) => at <= now,
            None => false,
        }
    }
    #[must_use]
    pub fn is_ready(&self) -> bool {
        if outbox::stop::active(&self.outbox) {
            return calls::settling_ready(self) || (!self.calls.is_full() && self.outbox.is_ready(&self.keep));
        }
        self.calls.is_ready() || (!self.calls.is_full() && (self.outbox.is_ready(&self.keep) || self.keep.is_ready()))
    }
    /// Whether a read asked now finds a call slot.
    #[must_use]
    pub fn has_room(&self) -> bool {
        !self.calls.is_full()
    }
    #[must_use]
    pub fn cached(&self, resource: &Resource) -> Option<&crate::Cached> {
        self.keep.cached(resource)
    }
    /// A stored record exceeded this root's configured capacities or was
    /// structurally inconsistent. The root must reconcile the store/limits;
    /// diagnostics cannot be the only way it learns recovery was refused.
    #[must_use]
    pub fn restoration_failed(&self) -> bool {
        self.keep.failed()
    }
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
    }
    pub fn pop_fact(&mut self) -> Option<Fact> {
        self.facts.pop()
    }
    #[must_use]
    pub const fn facts_lost(&self) -> u64 {
        self.lost
    }
    pub(crate) fn fact(&mut self, value: Fact) {
        if self.facts.try_push(value).is_err() {
            self.lost = self.lost.saturating_add(1);
        }
    }
}
/// A call sent, or one logical read completed/refused.
#[must_use]
pub const fn max_out(l: &Limits) -> u32 {
    let batch = l.resources.saturating_mul(2).saturating_add(l.repositories.saturating_mul(2)).saturating_add(3);
    if batch > 4 { batch } else { 4 }
}
/// Give a forge's writer once its access is known (forge-client.md, 7.1).
/// It saves a new writer as a `Writer` row and returns true. The same
/// author again changes nothing and returns true. A writer `give` refuses
/// returns false and changes nothing.
pub fn writer(d: &mut Domain, env: &Env<Limits>, writer: Writer, out: &mut Queue<Request>) -> bool {
    match crate::identity::give(d, &env.limits, writer) {
        crate::identity::Given::New => {
            out.push(Request::Save { record: Stored::Writer(writer) });
            true
        }
        crate::identity::Given::Held => true,
        crate::identity::Given::Refused => false,
    }
}
/// Decide one parent event and emit the records and effects of that decision.
pub fn step(d: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Settle => outbox::stop::start(d, env, out),
        Event::Keep { owner, watches } => {
            d.identity_used = true;
            keep::replace(d, env, owner, watches, out);
            // A replacement that drops the fresh pass's last pending watch
            // finishes that pass, as an answer would.
            if keep::restore::fresh_done(d) {
                out.push(Request::ReadAfreshDone);
            }
        }
        Event::Hint { hint } => keep::poll::hint(d, env, hint),
        Event::Writer { resource, taken } => keep::poll::writer(d, env, &resource, taken),
        Event::Pushed { resource, commit } => keep::poll::pushed(d, env, &resource, commit, out),
        Event::Make { entry } => {
            d.identity_used = true;
            outbox::make(d, env, entry, false, out);
        }
        Event::Remake { entry } => {
            d.identity_used = true;
            outbox::make(d, env, entry, true, out);
        }
        Event::Withdraw { entry } => outbox::withdraw(d, env, entry, out),
        Event::Restore { record } => {
            d.identity_used = true;
            match record {
                Stored::Live(record) => keep::restore::restore_live(d, env, record),
                Stored::Repository(record) => keep::restore::restore_repository(d, env, record),
                Stored::Writer(writer) => keep::restore::restore_writer(d, env, writer),
            }
        }
        Event::Restored { clock } => {
            outbox::restored(d, clock);
            keep::restore::restored(d);
        }
        Event::PauseOutbox => outbox::pause(d),
        Event::ReadAfresh => {
            keep::restore::start_fresh(d);
            if keep::restore::fresh_done(d) {
                out.push(Request::ReadAfreshDone);
            }
        }
        Event::SettleOutbox => {
            outbox::start_settle(d);
            if outbox::settle_done(d) {
                out.push(Request::OutboxDone);
            }
        }
        Event::Read { owner, repository, read } => {
            let empty_prefix = if let api::Read::Branches { prefix } = &read { prefix.is_empty() } else { false };
            let op = api::Op::Read(read);
            let refusal = if empty_prefix {
                Some(api::Error::Empty)
            } else if !bounds::op(&op, &env.limits) {
                Some(api::Error::TooLarge)
            } else if d.calls.is_full() || (d.keep.is_ready() && calls::background_owed(&d.calls, env)) {
                Some(api::Error::Busy)
            } else {
                None
            };
            match refusal {
                Some(error) => {
                    d.fact(Fact::Refused);
                    out.push(Request::Read { owner, result: Err(error) });
                }
                None => calls::queue(&mut d.calls, calls::Owner::Read(owner), repository, op, Priority::Fresh),
            }
        }
        Event::Answered { call, cost, free, rate, result } => {
            calls::answered(d, env, call, cost, free, rate, result, out);
            if keep::restore::fresh_done(d) {
                out.push(Request::ReadAfreshDone);
            }
            if outbox::settle_done(d) {
                out.push(Request::OutboxDone);
            }
        }
    }
    outbox::stop::advance(d, env, out);
}
/// One queued call per loop iteration. Only uncertain outbox entries retain
/// their original lifetime; unrelated reads have no startup delay.
/// Send one eligible call after earlier progress has been committed.
pub fn resume(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    if outbox::stop::active(&d.outbox) {
        if !outbox::pump(d, env, out) {
            calls::send(d, env, out);
        }
        outbox::stop::advance(d, env, out);
        return;
    }
    if calls::background_owed(&d.calls, env) && keep::poll::pump(d, env, out) {
        calls::send(d, env, out);
        if outbox::settle_done(d) {
            out.push(Request::OutboxDone);
        }
        return;
    }
    if outbox::pump(d, env, out) {
        if outbox::settle_done(d) {
            out.push(Request::OutboxDone);
        }
        return;
    }
    if keep::poll::pump(d, env, out) {
        calls::send(d, env, out);
        if outbox::settle_done(d) {
            out.push(Request::OutboxDone);
        }
        return;
    }
    calls::send(d, env, out);
    if outbox::settle_done(d) {
        out.push(Request::OutboxDone);
    }
}
/// Expire one client deadline at the injected time.
pub fn fire(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) {
    outbox::stop::advance(d, env, out);
    let Some(alarm) = d.alarms.expire(env.now) else {
        return;
    };
    match alarm {
        Alarm::Window => calls::window(d, env),
        Alarm::Reset => calls::reset(d, env),
        Alarm::ReadRetry(id) => calls::retry(d, id),
        Alarm::Entry(number) => outbox::due(d, number),
        Alarm::Resource(resource) => keep::poll::due_resource(d, env, resource),
        Alarm::Repository(repository) => keep::poll::due_repository(d, repository),
    }
}
