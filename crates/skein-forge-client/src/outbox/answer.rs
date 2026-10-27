use super::{
    Answer, Commit, Condition, Domain, Env, Error, Limits, Made, Outcome, Phase, Queue, Request, State, Write, api,
};

pub(crate) fn answered(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let w = d.outbox.entries.get(&number).expect("call owner remains");
    if w.withdrawn {
        super::finish(d, env, number, Outcome::Withdrawn, out);
        return;
    }
    let phase = match w.state {
        State::Busy(phase) => phase,
        State::Due(_) | State::Waiting(_) => unreachable!("entry terminal while busy"),
    };
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            super::retry::failed(d, env, number, phase, error, out);
            return;
        }
    };
    match answer {
        Answer::Runs { runs, more } => super::find::runs_found(d, env, number, phase, &runs, more, out),
        Answer::Run(run) => super::find::rerun_found(d, env, number, &run, out),
        Answer::Items { items, more, now } => {
            super::find::listed(d, env, number, phase, &items, super::find::Listing { more, now }, out);
        }
        Answer::Item { comments, more, .. } => super::find::comments_found(d, env, number, phase, &comments, more, out),
        Answer::Reviews { reviews, more } => super::find::reviews_found(d, env, number, phase, &reviews, more, out),
        Answer::Pull(pull) => pulled(d, env, number, phase, pull, out),
        Answer::Compare { commits, .. } => super::find::compared(d, env, number, phase, commits.is_empty(), out),
        Answer::Commit(commit) => branch_found(d, env, number, commit, out),
        Answer::Created(item) => super::finished(d, env, number, Made::Created(item), false, out),
        Answer::Commented(comment) => super::finished(d, env, number, Made::Commented(comment), false, out),
        Answer::Reviewed(_) => super::next(d, number, super::find(&w.entry)),
        Answer::Merged(commit) => {
            d.outbox.entries.get_mut(&number).expect("merge entry remains").uncertain = true;
            super::next(d, number, Phase::AfterMerge { merged: commit });
        }
        Answer::Branch(created) => branch_created(d, env, number, created, out),
        Answer::Done => done(d, env, number, out),
        Answer::Queued => {
            assert!(phase == Phase::Make && super::is_merge(&w.entry.effect.write), "only a merge is queued");
            super::queue::queued(d, env, number, out);
        }
        Answer::Statuses { .. }
        | Answer::Branches(_)
        | Answer::Remarks { .. }
        | Answer::PullFiles { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Tree { .. }
        | Answer::Commits { .. }
        | Answer::Located { .. }
        | Answer::Permission(_) => unreachable!("outbox operation has another answer shape"),
    }
}
fn branch_found(d: &mut Domain, env: &Env<Limits>, number: u64, commit: Commit, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("find owns entry");
    match w.entry.effect.write {
        Write::Dispatch { .. } => super::next(d, number, Phase::DispatchRuns { head: commit, page: 1 }),
        Write::CreateBranch { commit: expected, .. } => {
            if commit == expected {
                super::finished(d, env, number, Made::Branch(commit), true, out);
            } else {
                super::finish(d, env, number, Outcome::Failed(Error::Exists), out);
            }
        }
        Write::Rerun { .. } | Write::DeleteBranch { .. } => super::retry::not_found(d, env, number, out),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Merge { .. }
        | Write::Update { .. }
        | Write::Status { .. } => unreachable!("branch read for branch effect"),
    }
}
fn branch_created(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    created: api::BranchCreation,
    out: &mut Queue<Request>,
) {
    let w = d.outbox.entries.get(&number).expect("creation owns entry");
    match created {
        api::BranchCreation::Created => {
            let commit = match w.entry.effect.write {
                Write::CreateBranch { commit, .. } => commit,
                Write::CreateIssue { .. }
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
                | Write::Dispatch { .. }
                | Write::Rerun { .. }
                | Write::DeleteBranch { .. } => unreachable!("branch creation terminal"),
            };
            super::finished(d, env, number, Made::Branch(commit), false, out);
        }
        api::BranchCreation::Exists => super::next(d, number, super::find(&w.entry)),
    }
}
fn done(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("terminal owns entry");
    match w.entry.effect.write {
        Write::Update { .. } => {
            d.outbox.entries.get_mut(&number).expect("update entry remains").uncertain = true;
            super::next(d, number, Phase::AfterUpdate);
        }
        Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Status { .. }
        | Write::Dispatch { .. }
        | Write::Rerun { .. }
        | Write::DeleteBranch { .. } => super::finished(d, env, number, Made::Set, false, out),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Merge { .. }
        | Write::CreateBranch { .. } => unreachable!("creation has its typed terminal"),
    }
}

#[expect(clippy::too_many_lines, reason = "pull observations check each write phase and condition explicitly")]
fn pulled(d: &mut Domain, env: &Env<Limits>, number: u64, phase: Phase, pull: api::Pull, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("pull belongs to retained entry");
    match phase {
        Phase::Check => {
            if pull.state == api::State::Closed {
                super::finish(d, env, number, Outcome::Failed(Error::Closed), out);
                return;
            }
            let valid = match &w.entry.effect.condition {
                Condition::Merge { base } => {
                    let head = match w.entry.effect.write {
                        Write::Merge { head, .. } => head,
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
                        | Write::DeleteBranch { .. } => unreachable!("merge condition checked at admission"),
                    };
                    pull.base == *base && pull.commit == head
                }
                Condition::Update { head, base } => pull.commit == *head && pull.base_commit == Some(*base),
                Condition::Review { head } => pull.commit == *head,
                Condition::None => unreachable!("unconditional effects do not check pulls"),
            };
            if valid {
                super::next(d, number, Phase::Make);
            } else {
                super::finish(d, env, number, Outcome::Failed(Error::Stale), out);
            }
        }
        Phase::Find { .. } => match &w.entry.effect.write {
            Write::OpenPull { head, base, .. } => {
                if pull.head == *head && pull.base == *base {
                    if pull.state == api::State::Closed {
                        super::finish(
                            d,
                            env,
                            number,
                            Outcome::Raced { made: Made::Created(pull.number), why: Error::Closed },
                            out,
                        );
                    } else {
                        super::finished(d, env, number, Made::Created(pull.number), true, out);
                    }
                } else {
                    super::retry::not_found(d, env, number, out);
                }
            }
            Write::Merge { head, .. } => {
                if let Some(merged) = pull.merged {
                    if pull.commit == *head {
                        merge_verified(d, env, number, pull, merged, true, out);
                    } else {
                        super::finish(d, env, number, Outcome::Failed(Error::Stale), out);
                    }
                } else {
                    super::retry::not_found(d, env, number, out);
                }
            }
            Write::Update { .. } => {
                let base = match w.entry.effect.condition {
                    Condition::Update { base, .. } => base,
                    Condition::None | Condition::Merge { .. } | Condition::Review { .. } => {
                        unreachable!("update condition admitted")
                    }
                };
                super::next(d, number, Phase::Compare { head: pull.commit, base });
            }
            Write::CreateIssue { .. }
            | Write::Post { .. }
            | Write::Review { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. }
            | Write::CreateBranch { .. }
            | Write::Dispatch { .. }
            | Write::Rerun { .. }
            | Write::DeleteBranch { .. } => unreachable!("pull find belongs to pull creation or transition"),
        },
        Phase::Queued => super::queue::looked(d, env, number, pull, out),
        Phase::AfterMerge { merged } => merge_verified(d, env, number, pull, merged, false, out),
        Phase::AfterUpdate => {
            let base = match w.entry.effect.condition {
                Condition::Update { base, .. } => base,
                Condition::None | Condition::Merge { .. } | Condition::Review { .. } => {
                    unreachable!("update completion has update condition")
                }
            };
            super::next(d, number, Phase::Compare { head: pull.commit, base });
        }
        Phase::Clock | Phase::Make | Phase::DispatchRuns { .. } | Phase::Compare { .. } => {
            unreachable!("pull answers conditional checks and recovery")
        }
    }
}
pub(super) fn merge_verified(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    pull: api::Pull,
    merged: Commit,
    found: bool,
    out: &mut Queue<Request>,
) {
    let w = d.outbox.entries.get(&number).expect("merge entry remains");
    let base = match &w.entry.effect.condition {
        Condition::Merge { base } => base,
        Condition::None | Condition::Update { .. } | Condition::Review { .. } => {
            unreachable!("merge condition admitted")
        }
    };
    let head = match w.entry.effect.write {
        Write::Merge { head, .. } => head,
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
        | Write::DeleteBranch { .. } => unreachable!("merge effect admitted"),
    };
    let made = Made::Merged(merged);
    if pull.base == *base && pull.commit == head && pull.merged == Some(merged) {
        super::finished(d, env, number, made, found, out);
    } else {
        super::finish(d, env, number, Outcome::Raced { made, why: Error::Stale }, out);
    }
}
