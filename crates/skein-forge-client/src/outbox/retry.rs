use super::{
    Domain, Duration, Entry, Env, Error, Limits, Made, Outcome, Phase, Queue, Recovery, RecoveryClock, Request, Write,
    recovery,
};

#[expect(clippy::too_many_lines, reason = "each refusal class declares recovery for every write phase")]
pub(super) fn failed(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    phase: Phase,
    error: Error,
    out: &mut Queue<Request>,
) {
    if phase == Phase::Make && stopped_write(d, number, error, out) {
        return;
    }
    if phase == Phase::Queued {
        d.outbox.entries.get_mut(&number).expect("queued look owns entry").restart_checked = true;
    }
    if retain_uncertain(d, env, number, phase, error, out) {
        return;
    }
    match error {
        Error::RateLimited { .. } => unreachable!("request budget handles rate resets without completing owner"),
        Error::Timeout | Error::InvalidAnswer => match phase {
            Phase::Make => write_uncertain(d, number, out),
            Phase::Clock
            | Phase::Check
            | Phase::Queued
            | Phase::Find { .. }
            | Phase::DispatchRuns { .. }
            | Phase::Compare { .. }
            | Phase::AfterUpdate
            | Phase::AfterMerge { .. } => {
                backoff(d, env, number, phase, out);
            }
        },
        Error::Unavailable => backoff(d, env, number, phase, out),
        Error::TooLarge => match phase {
            Phase::Clock | Phase::Check | Phase::Make => super::finish(d, env, number, Outcome::Failed(error), out),
            Phase::Queued
            | Phase::Find { .. }
            | Phase::DispatchRuns { .. }
            | Phase::Compare { .. }
            | Phase::AfterUpdate
            | Phase::AfterMerge { .. } => {
                backoff(d, env, number, phase, out);
            }
        },
        Error::Missing => {
            let w = d.outbox.entries.get(&number).expect("missing answer belongs to live entry");
            match phase {
                Phase::Find { .. } => match w.entry.effect.write {
                    Write::DeleteBranch { .. } => super::finished(d, env, number, Made::Set, true, out),
                    Write::Dispatch { .. }
                    | Write::Rerun { .. }
                    | Write::CreateBranch { .. }
                    | Write::OpenPull { .. }
                    | Write::CreateIssue { .. }
                    | Write::Post { .. }
                    | Write::Review { .. }
                    | Write::Merge { .. }
                    | Write::Update { .. } => not_found(d, env, number, out),
                    Write::Edit { .. }
                    | Write::SetReviewers { .. }
                    | Write::Close { .. }
                    | Write::Reopen { .. }
                    | Write::Status { .. } => super::finish(d, env, number, Outcome::Failed(error), out),
                },
                Phase::Make => match w.entry.effect.write {
                    Write::DeleteBranch { .. } => super::finished(d, env, number, Made::Set, false, out),
                    Write::Dispatch { .. }
                    | Write::Rerun { .. }
                    | Write::CreateIssue { .. }
                    | Write::OpenPull { .. }
                    | Write::Post { .. }
                    | Write::Review { .. }
                    | Write::Edit { .. }
                    | Write::SetReviewers { .. }
                    | Write::Close { .. }
                    | Write::Reopen { .. }
                    | Write::Merge { .. }
                    | Write::Update { .. }
                    | Write::Status { .. }
                    | Write::CreateBranch { .. } => super::finish(d, env, number, Outcome::Failed(error), out),
                },
                Phase::DispatchRuns { .. } => not_found(d, env, number, out),
                Phase::Queued
                | Phase::Clock
                | Phase::Check
                | Phase::Compare { .. }
                | Phase::AfterUpdate
                | Phase::AfterMerge { .. } => {
                    super::finish(d, env, number, Outcome::Failed(error), out);
                }
            }
        }
        Error::Exists => {
            let w = d.outbox.entries.get(&number).expect("exists belongs to live entry");
            if w.entry.attempt.is_some() {
                let phase = super::find(&w.entry);
                super::next(d, number, phase);
            } else {
                super::finish(d, env, number, Outcome::Failed(error), out);
            }
        }
        Error::Forbidden
        | Error::MissingJob
        | Error::Empty
        | Error::Full
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Refused
        | Error::NotConnected
        | Error::NotInstalled
        | Error::Busy => super::finish(d, env, number, Outcome::Failed(error), out),
    }
}
fn stopped_write(d: &mut Domain, number: u64, error: Error, out: &mut Queue<Request>) -> bool {
    if !super::stop::active(&d.outbox) {
        return false;
    }
    let lost = match error {
        Error::Timeout | Error::InvalidAnswer | Error::Unavailable => true,
        Error::RateLimited { .. }
        | Error::TooLarge
        | Error::Missing
        | Error::Exists
        | Error::Forbidden
        | Error::MissingJob
        | Error::Empty
        | Error::Full
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Refused
        | Error::NotConnected
        | Error::NotInstalled
        | Error::Busy => false,
    };
    if lost {
        write_uncertain(d, number, out);
    }
    lost
}
fn write_uncertain(d: &mut Domain, number: u64, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get_mut(&number).expect("failed write remains");
    w.uncertain = true;
    w.awaiting_outcome = false;
    w.entry.failures = w.entry.failures.saturating_add(1);
    let phase = super::find(&w.entry);
    let task = w.entry.task;
    out.push(Request::Progress { entry: w.entry.clone() });
    out.push(Request::Outcome { entry: number, task, outcome: Outcome::Uncertain });
    super::next(d, number, phase);
}
fn backoff(d: &mut Domain, env: &Env<Limits>, number: u64, phase: Phase, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get_mut(&number).expect("retry belongs to retained entry");
    w.entry.failures = w.entry.failures.saturating_add(1);
    let uncertain = w.uncertain;
    if w.entry.failures >= env.limits.write_attempts && !uncertain {
        super::finish(d, env, number, Outcome::Failed(Error::Unavailable), out);
        return;
    }
    let mut delay = env.limits.backoff;
    for _ in 1..w.entry.failures {
        delay = delay.saturating_mul(2).min(env.limits.backoff_max);
        if delay == env.limits.backoff_max {
            break;
        }
    }
    let jitter = d.rng.below(delay.as_nanos());
    let delay = delay.saturating_add(Duration::from_nanos(jitter)).min(env.limits.backoff_max);
    out.push(Request::Progress { entry: w.entry.clone() });
    super::wait(d, number, phase, env.now.saturating_add(delay));
}

fn retain_uncertain(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    phase: Phase,
    error: Error,
    out: &mut Queue<Request>,
) -> bool {
    if phase == Phase::Queued && error == Error::Missing {
        return false;
    }
    let transient = match error {
        Error::Timeout | Error::InvalidAnswer | Error::Unavailable | Error::TooLarge | Error::RateLimited { .. } => {
            true
        }
        Error::Forbidden
        | Error::Missing
        | Error::MissingJob
        | Error::Empty
        | Error::Full
        | Error::Exists
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Refused
        | Error::NotConnected
        | Error::NotInstalled
        | Error::Busy => false,
    };
    if !transient {
        // Absence answers creation and deletion recovery lookups. Let
        // `failed` confirm deletion or apply a creation's saved deadline.
        if error == Error::Missing
            && let Phase::Find { .. } | Phase::DispatchRuns { .. } = phase
        {
            let w = d.outbox.entries.get(&number).expect("find owns entry");
            match w.entry.effect.write {
                Write::CreateBranch { .. }
                | Write::OpenPull { .. }
                | Write::CreateIssue { .. }
                | Write::Post { .. }
                | Write::Review { .. }
                | Write::Merge { .. }
                | Write::Update { .. }
                | Write::Dispatch { .. }
                | Write::Rerun { .. }
                | Write::DeleteBranch { .. } => return false,
                Write::Edit { .. }
                | Write::SetReviewers { .. }
                | Write::Close { .. }
                | Write::Reopen { .. }
                | Write::Status { .. } => {}
            }
        }
        match phase {
            Phase::AfterMerge { merged } => {
                super::finish(d, env, number, Outcome::Raced { made: Made::Merged(merged), why: error }, out);
                return true;
            }
            Phase::Clock
            | Phase::Check
            | Phase::Queued
            | Phase::Make
            | Phase::Find { .. }
            | Phase::DispatchRuns { .. }
            | Phase::Compare { .. }
            | Phase::AfterUpdate => {}
        }
        if d.outbox.entries.get(&number).expect("failed entry remains").uncertain {
            backoff(d, env, number, phase, out);
            return true;
        }
    }
    false
}
fn remaining(entry: &Entry, clock: RecoveryClock, env: &Env<Limits>) -> Duration {
    let attempt = entry.attempt.expect("uncertain entry has an attempt");
    match clock {
        RecoveryClock::Monotonic => attempt.deadline.saturating_since(env.now),
        RecoveryClock::Wall => Duration::from_nanos(attempt.expires.as_nanos().saturating_sub(env.wall.as_nanos())),
    }
}
pub(super) fn not_found(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    d.outbox.entries.get_mut(&number).expect("find owns entry").restart_checked = true;
    let w = d.outbox.entries.get(&number).expect("find owns entry");
    let left = remaining(&w.entry, d.outbox.clock, env);
    if left == Duration::ZERO {
        if recovery(&w.entry.effect.write) == Recovery::Unrecoverable && !w.again {
            super::finish(d, env, number, Outcome::Held, out);
        } else if w.entry.failures >= env.limits.write_attempts {
            super::finish(d, env, number, Outcome::Failed(Error::Timeout), out);
        } else {
            let phase = super::check_phase(&w.entry);
            d.outbox.entries.get_mut(&number).expect("entry remains").uncertain = false;
            super::next(d, number, phase);
        }
    } else {
        let phase = super::find(&w.entry);
        super::wait(d, number, phase, env.now.saturating_add(left));
    }
}
