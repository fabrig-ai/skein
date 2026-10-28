//! A merge held by the forge's queue (forge-client.md, 7.7).
use super::{Domain, Env, Error, Limits, Outcome, Phase, Queue, Request, State, Time, api};

/// When the next look at a queued pull request is due (7.7).
#[derive(Debug)]
pub(super) struct Look {
    /// The time the armed look is due: `wait` sets it on every wait.
    at: Time,
    /// A hint arrived while a look was in flight.
    hinted: bool,
}
impl Look {
    /// A look due at `at`, with no hint waiting.
    pub(super) fn new(at: Time) -> Look {
        Look { at, hinted: false }
    }
    /// The time the armed look is due.
    pub(super) fn at(&self) -> Time {
        self.at
    }
    /// Record the time a look is now due.
    pub(super) fn due(&mut self, at: Time) {
        self.at = at;
    }
}
pub(super) fn queued(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get_mut(&number).expect("queued merge remains");
    w.awaiting_outcome = false;
    w.entry.queued = true;
    w.uncertain = true;
    out.push(Request::Progress { entry: w.entry.clone() });
    out.push(Request::Outcome { entry: number, task: w.entry.task, outcome: Outcome::Queued });
    super::wait(d, number, Phase::Queued, env.now.saturating_add(env.limits.poll));
}
pub(super) fn looked(d: &mut Domain, env: &Env<Limits>, number: u64, pull: api::Pull, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get_mut(&number).expect("queued look owns entry");
    w.restart_checked = true;
    if let Some(merged) = pull.merged {
        super::answer::merge_verified(d, env, number, pull, merged, false, out);
    } else if pull.state == api::State::Closed {
        super::finish(d, env, number, Outcome::Failed(Error::Closed), out);
    } else if pull.queued.is_some() {
        let delay = if w.look.hinted { env.limits.hinted } else { env.limits.poll };
        w.look.hinted = false;
        super::wait(d, number, Phase::Queued, env.now.saturating_add(delay));
    } else {
        let head = match w.entry.effect.write {
            api::Write::Merge { head, .. } => head,
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Labels { .. }
            | api::Write::CreateLabel { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. } => unreachable!("only a merge is queued"),
        };
        let error = if pull.commit != head {
            Error::Stale
        } else if !pull.mergeable {
            Error::Conflict
        } else {
            Error::Refused
        };
        super::finish(d, env, number, Outcome::Failed(error), out);
    }
}
pub(crate) fn hinted(d: &mut Domain, env: &Env<Limits>, hint: &api::Hint) {
    let number = match hint.change {
        api::Change::Item(number) => number,
        api::Change::Branch(_) | api::Change::BranchMoved { .. } | api::Change::Commit(_) => return,
    };
    let at = env.now.saturating_add(env.limits.hinted);
    let mut after = core::ops::Bound::Unbounded;
    for _ in 0..d.outbox.entries.len() {
        let Some((&entry, _)) = d.outbox.entries.range((after, core::ops::Bound::Unbounded)).next() else {
            break;
        };
        after = core::ops::Bound::Excluded(entry);
        let w = d.outbox.entries.get_mut(&entry).expect("bounded entry remains");
        if w.entry.repository != hint.repository || w.lane.number != Some(number) || !w.entry.queued {
            continue;
        }
        match w.state {
            State::Waiting(Phase::Queued) => {
                if at < w.look.at() {
                    super::wait(d, entry, Phase::Queued, at);
                }
            }
            State::Busy(Phase::Queued) => w.look.hinted = true,
            State::Due(_) | State::Busy(_) | State::Waiting(_) => {}
        }
    }
}
