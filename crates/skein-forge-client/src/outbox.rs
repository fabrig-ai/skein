//! In-flight write lanes and uncertain-effect recovery (forge-client.md,
//! sections 7–8).
//!
//! The client holds execution copies, not the top's durable outbox entries.
//! It never chooses effects or sends their news. `make` accepts a committed
//! entry; each lane moves Due → Busy → Due, Waiting, or settled. Recovery
//! searches by key or resulting state before retrying after the saved
//! lifetime. `Progress` gives the top a new durable position, and `Outcome`
//! reports settlement; the parent commits them before releasing calls.
pub(crate) mod answer;
pub(crate) mod find;
pub(crate) mod queue;
pub(crate) mod retry;
pub(crate) mod stop;
use crate::calls::{self, Owner};
use crate::domain::{Alarm, Domain};
use crate::{
    Attempt, Condition, Entry, Fact, Limits, Made, Outcome, Position, Priority, Recovery, RecoveryClock, Request,
    bounds, recovery,
};
use skein_forge::api::{self, Answer, Commit, Error, Op, Read, Repository, Write};
use skein_lib::{Duration, Env, Map, Queue, Time, Wall};

#[derive(Debug)]
pub(crate) struct Outbox {
    entries: Map<u64, Writing>,
    ready: bool,
    paused: bool,
    settling: bool,
    clock: RecoveryClock,
    stop: stop::State,
}
#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent recovery facts of one outbox write")]
struct Writing {
    entry: Entry,
    lane: Lane,
    clock: Time,
    state: State,
    look: queue::Look,
    uncertain: bool,
    restart_checked: bool,
    withdrawn: bool,
    /// A person asked for it again: not found after its lifetime, it is made anew.
    again: bool,
    awaiting_outcome: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct Lane {
    repository: Repository,
    number: Option<u64>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Due(Phase),
    Busy(Phase),
    Waiting(Phase),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    Clock,
    Check,
    Make,
    Queued,
    Find { page: u32, after: u64 },
    DispatchRuns { head: Commit, page: u32 },
    Compare { head: Commit, base: Commit },
    AfterUpdate,
    AfterMerge { merged: Commit },
}
impl Outbox {
    pub(crate) fn new(l: &Limits) -> Outbox {
        Outbox {
            entries: Map::with_capacity(l.entries),
            ready: false,
            paused: false,
            settling: false,
            clock: RecoveryClock::Monotonic,
            stop: stop::State::Running,
        }
    }
    pub(crate) fn is_ready(&self, keep: &crate::keep::Keep) -> bool {
        if !self.ready || self.paused {
            return false;
        }
        for (&number, writing) in &self.entries {
            match writing.state {
                State::Due(_) => {
                    if eligible(self, keep, number, writing) && stop::entry(self, number) {
                        return true;
                    }
                }
                State::Busy(_) | State::Waiting(_) => {}
            }
        }
        false
    }
}
pub(crate) fn worst_case(l: &Limits) -> Option<u64> {
    Map::<u64, Writing>::worst_case(l.entries)?.checked_add(u64::from(l.entries).checked_mul(u64::from(l.op_bytes))?)
}
fn valid(entry: &Entry, l: &Limits) -> bool {
    if entry.queued && (entry.attempt.is_none() || !is_merge(&entry.effect.write)) {
        return false;
    }
    if entry.attempt.is_some() && entry.start.is_none() {
        return false;
    }
    let Some(held) = bounds::write(&entry.effect.write, l) else {
        return false;
    };
    let condition = match &entry.effect.condition {
        Condition::None | Condition::Update { .. } | Condition::Review { .. } => 0,
        Condition::Merge { base } => base.len(),
    };
    let Some(held) = held.checked_add(u64::try_from(condition).expect("usize fits u64")) else {
        return false;
    };
    if held > u64::from(l.op_bytes) {
        return false;
    }
    match &entry.effect.write {
        Write::Merge { .. } => match entry.effect.condition {
            Condition::Merge { .. } => true,
            Condition::None | Condition::Update { .. } | Condition::Review { .. } => false,
        },
        Write::Update { .. } => match entry.effect.condition {
            Condition::Update { .. } => true,
            Condition::None | Condition::Merge { .. } | Condition::Review { .. } => false,
        },
        Write::Review { key, .. } => match entry.effect.condition {
            Condition::Review { .. } => !key.is_empty(),
            Condition::None | Condition::Merge { .. } | Condition::Update { .. } => false,
        },
        Write::CreateIssue { key, .. } | Write::Post { key, .. } => {
            entry.effect.condition == Condition::None && !key.is_empty()
        }
        Write::OpenPull { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::Dispatch { .. }
        | Write::Rerun { .. }
        | Write::DeleteBranch { .. } => entry.effect.condition == Condition::None,
    }
}
fn lane(entry: &Entry) -> Lane {
    let number = match entry.effect.write {
        Write::Post { number, .. }
        | Write::Review { number, .. }
        | Write::Edit { number, .. }
        | Write::SetReviewers { number, .. }
        | Write::Close { number }
        | Write::Reopen { number }
        | Write::Merge { number, .. }
        | Write::Update { number, .. } => Some(number),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::Dispatch { .. }
        | Write::Rerun { .. }
        | Write::DeleteBranch { .. } => None,
    };
    Lane { repository: entry.repository, number }
}
fn find(entry: &Entry) -> Phase {
    let position = entry.start.expect("an attempted write has a saved start");
    Phase::Find { page: position.review_page.max(1), after: position.comment }
}
pub(crate) fn make(d: &mut Domain, env: &Env<Limits>, entry: Entry, again: bool, out: &mut Queue<Request>) {
    let error = if !d.outbox.ready {
        Some(Error::Busy)
    } else if !crate::identity::connected(d, entry.repository) {
        Some(Error::Forbidden)
    } else if !valid(&entry, &env.limits) {
        Some(Error::TooLarge)
    } else if d.outbox.entries.len() >= env.limits.entries {
        Some(Error::Busy)
    } else {
        None
    };
    if let Some(error) = error {
        d.fact(Fact::Refused);
        out.push(Request::Outcome { entry: entry.number, task: entry.task, outcome: Outcome::Failed(error) });
        return;
    }
    assert!(!d.outbox.entries.contains_key(&entry.number), "an outbox entry is handed off once");
    if !again && let Some((number, _)) = d.outbox.entries.last() {
        assert!(*number < entry.number, "new entries arrive in commit order");
    }
    let number = entry.number;
    let lane = lane(&entry);
    let uncertain = entry.attempt.is_some();
    let phase = if entry.queued {
        Phase::Queued
    } else if uncertain {
        find(&entry)
    } else {
        Phase::Clock
    };
    let again = again && !entry.queued;
    d.outbox
        .entries
        .insert(
            number,
            Writing {
                entry,
                lane,
                clock: Time::ZERO,
                state: State::Due(phase),
                look: queue::Look::new(env.now),
                uncertain,
                restart_checked: !uncertain,
                withdrawn: false,
                again,
                awaiting_outcome: false,
            },
        )
        .expect("outbox admission checked");
}
pub(crate) fn restored(d: &mut Domain, clock: RecoveryClock) {
    assert!(!d.outbox.ready, "restored once");
    d.outbox.ready = true;
    d.outbox.clock = clock;
}
pub(crate) fn pause(d: &mut Domain) {
    assert!(d.outbox.ready, "pause after restore");
    d.outbox.paused = true;
}
pub(crate) fn start_settle(d: &mut Domain) {
    assert!(d.outbox.ready && d.outbox.paused, "settle follows restored fresh reads");
    d.outbox.paused = false;
    d.outbox.settling = true;
}
pub(crate) fn settle_done(d: &mut Domain) -> bool {
    if !d.outbox.settling {
        return false;
    }
    for (_, entry) in &d.outbox.entries {
        if !entry.restart_checked {
            return false;
        }
    }
    d.outbox.settling = false;
    true
}
pub(crate) fn withdraw(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let Some(writing) = d.outbox.entries.get(&number) else {
        return;
    };
    if writing.entry.attempt.is_some() {
        return;
    }
    match writing.state {
        State::Due(_) | State::Waiting(_) => {}
        State::Busy(_) => {
            if !calls::cancel_entry(&mut d.calls, number) {
                d.outbox.entries.get_mut(&number).expect("withdrawn entry remains").withdrawn = true;
                return;
            }
        }
    }
    finish(d, env, number, Outcome::Withdrawn, out);
}
pub(crate) fn pump(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) -> bool {
    if !d.outbox.ready || d.outbox.paused || d.calls.is_full() {
        return false;
    }
    let mut selected = None;
    for (&number, writing) in &d.outbox.entries {
        let phase = match writing.state {
            State::Due(phase) => phase,
            State::Busy(_) | State::Waiting(_) => continue,
        };
        if eligible(&d.outbox, &d.keep, number, writing) && stop::entry(&d.outbox, number) {
            selected = Some((number, phase));
            break;
        }
    }
    let Some((number, phase)) = selected else {
        return false;
    };
    match phase {
        Phase::Find { .. } => {
            let w = d.outbox.entries.get(&number).expect("selected entry remains");
            if is_set(&w.entry.effect.write) {
                retry::not_found(d, env, number, out);
                return true;
            }
        }
        Phase::Clock
        | Phase::Check
        | Phase::Queued
        | Phase::Make
        | Phase::DispatchRuns { .. }
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {}
    }
    let writing = d.outbox.entries.get_mut(&number).expect("selected entry remains");
    let op = operation(writing, phase);
    let repository = writing.entry.repository;
    let priority = match phase {
        Phase::Make => Priority::Write,
        Phase::Clock
        | Phase::Check
        | Phase::Queued
        | Phase::Find { .. }
        | Phase::DispatchRuns { .. }
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => Priority::Fresh,
    };
    writing.state = State::Busy(phase);
    calls::queue(&mut d.calls, Owner::Entry(number), repository, op, priority);
    false
}
fn operation(w: &Writing, phase: Phase) -> Op {
    let read = match phase {
        // An empty inclusive listing supplies Date without scanning history.
        Phase::Clock => Read::Items { since: Time::from_nanos(u64::MAX), page: 1, kind: None },
        Phase::Make => return Op::Write(w.entry.effect.write.clone()),
        Phase::DispatchRuns { head, page } => {
            let Write::Dispatch { workflow, .. } = &w.entry.effect.write else {
                unreachable!("dispatch run lookup");
            };
            Read::Runs { workflow: workflow.clone(), commit: head, page }
        }
        Phase::Compare { head, base } => Read::Compare { before: head, after: base },
        Phase::Check | Phase::Queued | Phase::AfterUpdate | Phase::AfterMerge { .. } => {
            Read::Pull { number: w.lane.number.expect("conditional effect names an item") }
        }
        Phase::Find { page, after } => match &w.entry.effect.write {
            Write::Dispatch { branch, .. } => Read::Branch { branch: branch.clone() },
            Write::Rerun { job } => Read::Run { run: job.run },
            Write::CreateIssue { .. } => Read::Items {
                since: w.entry.start.expect("find has saved start").at,
                page,
                kind: Some(api::Kind::Issue),
            },
            Write::OpenPull { head, base, .. } => Read::PullFor { head: head.clone(), base: base.clone() },
            Write::Post { number, .. } => Read::Item { number: *number, after },
            Write::Review { number, .. } => Read::Reviews { number: *number, page },
            Write::Merge { number, .. } | Write::Update { number, .. } => Read::Pull { number: *number },
            Write::CreateBranch { branch, .. } | Write::DeleteBranch { branch } => {
                Read::Branch { branch: branch.clone() }
            }
            Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. } => unreachable!("set recovery waits before entering make"),
        },
    };
    Op::Read(read)
}
/// Called at submission, before `Request::Call` is emitted. Progress joins
/// the decision, so the parent's commit gate makes both start fields
/// durable before the write can reach the forge.
pub(crate) fn sent(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let entry = &d.outbox.entries.get(&number).expect("call belongs to a live entry").entry;
    let position = crate::keep::echo::position(&d.keep, entry);
    let w = d.outbox.entries.get_mut(&number).expect("call belongs to a live entry");
    let phase = match w.state {
        State::Busy(phase) => phase,
        State::Due(_) | State::Waiting(_) => unreachable!("queued entry is busy"),
    };
    match phase {
        Phase::Make => {
            w.awaiting_outcome = true;
            if w.entry.start.is_none() {
                w.entry.start = Some(Position { at: w.clock, ..position });
            }
            let span = env.limits.lifetime.saturating_add(env.limits.clock_margin);
            let expires = Wall::from_nanos(env.wall.as_nanos().saturating_add(span.as_nanos()));
            w.entry.attempt =
                Some(Attempt { sent: env.now, deadline: env.now.saturating_add(span), wall: env.wall, expires });
            out.push(Request::Progress { entry: w.entry.clone() });
        }
        Phase::Clock
        | Phase::Check
        | Phase::Queued
        | Phase::Find { .. }
        | Phase::DispatchRuns { .. }
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {}
    }
}
pub(crate) fn due(d: &mut Domain, number: u64) {
    let w = d.outbox.entries.get_mut(&number).expect("entry alarm belongs to retained entry");
    match w.state {
        State::Waiting(phase) => w.state = State::Due(phase),
        State::Due(_) | State::Busy(_) => unreachable!("entry alarm fires while waiting"),
    }
}
fn check_phase(entry: &Entry) -> Phase {
    match entry.effect.condition {
        Condition::None => Phase::Make,
        Condition::Merge { .. } | Condition::Update { .. } | Condition::Review { .. } => Phase::Check,
    }
}
fn finished(d: &mut Domain, env: &Env<Limits>, number: u64, made: Made, found: bool, out: &mut Queue<Request>) {
    finish(d, env, number, Outcome::Made { made, found }, out);
}
fn finish(d: &mut Domain, env: &Env<Limits>, number: u64, outcome: Outcome, out: &mut Queue<Request>) {
    let w = d.outbox.entries.remove(&number).expect("settled entry lives");
    match outcome {
        Outcome::Made { made, .. } | Outcome::Raced { made, .. } => {
            crate::keep::echo::made(d, env, &w.entry, made, out);
        }
        Outcome::Failed(_) | Outcome::Uncertain | Outcome::Queued | Outcome::Held | Outcome::Withdrawn => {}
    }
    d.alarms.cancel(Alarm::Entry(number));
    out.push(Request::Outcome { entry: number, task: w.entry.task, outcome });
}
fn next(d: &mut Domain, number: u64, phase: Phase) {
    d.outbox.entries.get_mut(&number).expect("entry remains").state = State::Due(phase);
}
fn wait(d: &mut Domain, number: u64, phase: Phase, until: Time) {
    let w = d.outbox.entries.get_mut(&number).expect("entry remains");
    w.state = State::Waiting(phase);
    if phase == Phase::Queued {
        w.look.due(until);
    }
    d.alarms.arm(Alarm::Entry(number), until).expect("one alarm per entry fits");
}
fn is_set(write: &Write) -> bool {
    match write {
        Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Status { .. } => true,
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Merge { .. }
        | Write::Update { .. }
        | Write::CreateBranch { .. }
        | Write::Dispatch { .. }
        | Write::Rerun { .. }
        | Write::DeleteBranch { .. } => false,
    }
}
fn key(write: &Write) -> Option<&[u8]> {
    match write {
        Write::CreateIssue { key, .. } | Write::Post { key, .. } | Write::Review { key, .. } => Some(key),
        Write::OpenPull { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Merge { .. }
        | Write::Update { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::Dispatch { .. }
        | Write::Rerun { .. }
        | Write::DeleteBranch { .. } => None,
    }
}
fn same_key(value: Option<&[u8]>, expected: &[u8]) -> bool {
    match value {
        Some(value) => value == expected,
        None => false,
    }
}

fn eligible(o: &Outbox, keep: &crate::keep::Keep, number: u64, writing: &Writing) -> bool {
    for (&other, previous) in &o.entries {
        if other >= number {
            break;
        }
        if previous.lane == writing.lane {
            return false;
        }
    }
    match writing.state {
        State::Due(Phase::Make | Phase::Find { .. }) => crate::keep::echo::echo_room(keep, &writing.entry),
        State::Due(_) => true,
        State::Busy(_) | State::Waiting(_) => false,
    }
}
pub(crate) fn echo(d: &Domain, repository: Repository, key: &[u8]) -> bool {
    for (_, writing) in &d.outbox.entries {
        if writing.entry.repository != repository {
            continue;
        }
        match &writing.entry.effect.write {
            Write::CreateIssue { key: own, .. } | Write::Post { key: own, .. } | Write::Review { key: own, .. } => {
                if **own == *key {
                    return true;
                }
            }
            Write::OpenPull { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Merge { .. }
            | Write::Update { .. }
            | Write::Status { .. }
            | Write::CreateBranch { .. }
            | Write::Dispatch { .. }
            | Write::Rerun { .. }
            | Write::DeleteBranch { .. } => {}
        }
    }
    false
}

fn is_merge(write: &Write) -> bool {
    match write {
        Write::Merge { .. } => true,
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Update { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::Dispatch { .. }
        | Write::Rerun { .. }
        | Write::DeleteBranch { .. } => false,
    }
}
