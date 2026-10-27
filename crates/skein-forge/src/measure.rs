//! Fixed payload accounting for forge values (forge.md, section 3.7).
use crate::api::{self, Answer, File, Op, Pull, Read, Summary, Write};
use alloc::boxed::Box;

/// Fixed bytes for one summary list element.
pub const SUMMARY_BYTES: usize = 128;
/// Fixed bytes for one review list element.
pub const REVIEW_BYTES: usize = 128;
/// Fixed bytes for one status list element.
pub const STATUS_BYTES: usize = 160;
/// Fixed bytes for one comment list element.
pub const COMMENT_BYTES: usize = 96;
/// Fixed bytes for one remark list element.
pub const REMARK_BYTES: usize = 64;
/// Fixed bytes for one file list element.
pub const FILE_BYTES: usize = 64;
/// Fixed bytes for one commit list element.
pub const COMMIT_BYTES: usize = 32;
/// Fixed bytes for one collaborator list element.
pub const COLLABORATOR_BYTES: usize = 16;
/// Fixed bytes for one string list element.
pub const STRING_BYTES: usize = 16;
/// Fixed bytes for one id list element.
pub const ID_BYTES: usize = 8;
/// Fixed bytes for one run list element.
pub const RUN_BYTES: usize = 128;
/// Fixed bytes for one job list element.
pub const RUN_JOB_BYTES: usize = 64;
/// Fixed bytes for one tree entry list element.
pub const ENTRY_BYTES: usize = 64;
/// Fixed bytes for one input list element.
pub const INPUT_BYTES: usize = 32;
const _: () = {
    assert!(RUN_BYTES >= size_of::<api::Run>(), "run width covers native storage");
    assert!(RUN_JOB_BYTES >= size_of::<api::RunJob>(), "job width covers native storage");
    assert!(ENTRY_BYTES >= size_of::<api::Entry>(), "entry width covers native storage");
    assert!(INPUT_BYTES >= size_of::<api::Input>(), "input width covers native storage");
    assert!(SUMMARY_BYTES >= size_of::<Summary>(), "summary width covers native storage");
    assert!(REVIEW_BYTES >= size_of::<api::Review>(), "review width covers native storage");
    assert!(STATUS_BYTES >= size_of::<api::Status>(), "status width covers native storage");
    assert!(COMMENT_BYTES >= size_of::<api::Comment>(), "comment width covers native storage");
    assert!(REMARK_BYTES >= size_of::<api::Remark>(), "remark width covers native storage");
    assert!(FILE_BYTES >= size_of::<File>(), "file width covers native storage");
    assert!(COMMIT_BYTES >= size_of::<api::Commit>(), "commit width covers native storage");
    assert!(COLLABORATOR_BYTES >= size_of::<api::Collaborator>(), "collaborator width covers native storage");
    assert!(STRING_BYTES >= size_of::<Box<[u8]>>(), "string width covers native storage");
    assert!(ID_BYTES >= size_of::<u64>(), "id width covers native storage");
};
fn bytes(value: &[u8]) -> Option<u64> {
    u64::try_from(value.len()).ok()
}
fn optional(value: Option<&[u8]>) -> Option<u64> {
    match value {
        Some(value) => bytes(value),
        None => Some(0),
    }
}
fn rows(count: usize, width: usize) -> Option<u64> {
    u64::try_from(count).ok()?.checked_mul(u64::try_from(width).ok()?)
}
/// Count strings and fixed list widths without validating an operation.
#[must_use]
pub fn op(value: &Op) -> Option<u64> {
    match value {
        Op::Read(value) => read(value),
        Op::Write(value) => write(value),
    }
}
fn read(value: &Read) -> Option<u64> {
    match value {
        Read::Runs { workflow, .. } => bytes(workflow),
        Read::Artifact { name, .. } => bytes(name),
        Read::Locate { owner, name } => bytes(owner)?.checked_add(bytes(name)?),
        Read::File { path, .. } => bytes(path),
        Read::Branches { prefix } => bytes(prefix),
        Read::PullFor { head, base } => bytes(head)?.checked_add(bytes(base)?),
        Read::Branch { branch } | Read::Protection { branch } => bytes(branch),
        Read::Run { .. }
        | Read::Tree { .. }
        | Read::Commits { .. }
        | Read::Items { .. }
        | Read::Reviews { .. }
        | Read::Statuses { .. }
        | Read::Remarks { .. }
        | Read::PullFiles { .. }
        | Read::Collaborators { .. }
        | Read::Job { .. }
        | Read::Item { .. }
        | Read::Pull { .. }
        | Read::Compare { .. }
        | Read::Checks { .. }
        | Read::Settings
        | Read::Permission { .. } => Some(0),
    }
}
/// Count a borrowed write without validating its fields.
#[must_use]
pub fn write(value: &Write) -> Option<u64> {
    match value {
        Write::CreateIssue { key, title, body } => bytes(key)?.checked_add(bytes(title)?)?.checked_add(bytes(body)?),
        Write::OpenPull { title, body, head, base } => {
            bytes(title)?.checked_add(bytes(body)?)?.checked_add(bytes(head)?)?.checked_add(bytes(base)?)
        }
        Write::Post { key, body, .. } | Write::Review { key, body, .. } => bytes(key)?.checked_add(bytes(body)?),
        Write::Edit { title, body, .. } => optional(title.as_deref())?.checked_add(optional(body.as_deref())?),
        Write::SetReviewers { reviewers, .. } => rows(reviewers.len(), ID_BYTES),
        Write::Dispatch { workflow, branch, inputs } => {
            let mut held =
                rows(inputs.len(), INPUT_BYTES)?.checked_add(bytes(workflow)?)?.checked_add(bytes(branch)?)?;
            for input in inputs {
                held = held.checked_add(bytes(&input.name)?)?.checked_add(bytes(&input.value)?)?;
            }
            Some(held)
        }
        Write::Status { context, description, link, .. } => {
            bytes(context)?.checked_add(bytes(description)?)?.checked_add(bytes(link)?)
        }
        Write::CreateBranch { branch, .. } | Write::DeleteBranch { branch } => bytes(branch),
        Write::Rerun { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Update { .. }
        | Write::Merge { .. } => Some(0),
    }
}
/// Count a summary including its nested labels, without a scalar struct width.
#[must_use]
pub fn summary(value: &Summary) -> Option<u64> {
    let mut held = rows(value.labels.len(), STRING_BYTES)?;
    for label in &value.labels {
        held = held.checked_add(bytes(label)?)?;
    }
    held.checked_add(optional(value.key.as_deref())?)?
        .checked_add(bytes(&value.title)?)?
        .checked_add(bytes(&value.body)?)
}
fn files(values: &[File]) -> Option<u64> {
    let mut held = rows(values.len(), FILE_BYTES)?;
    for value in values {
        held = held
            .checked_add(bytes(&value.path)?)?
            .checked_add(optional(value.before.as_deref())?)?
            .checked_add(optional(value.after.as_deref())?)?;
    }
    Some(held)
}
/// Count every string and fixed list element in an answer.
#[must_use]
pub fn answer(value: &Answer) -> Option<u64> {
    match value {
        Answer::Run(value) => run(value),
        Answer::Runs { runs, .. } => {
            let mut held = rows(runs.len(), RUN_BYTES)?;
            for value in runs {
                held = held.checked_add(run(value)?)?;
            }
            Some(held)
        }
        Answer::Commits { commits, .. } => commit_messages(commits),
        Answer::Tree { entries, .. } => {
            let mut held = rows(entries.len(), ENTRY_BYTES)?;
            for entry in entries {
                held = held.checked_add(bytes(&entry.name)?)?;
            }
            Some(held)
        }
        Answer::Items { items, .. } => {
            let mut held = rows(items.len(), SUMMARY_BYTES)?;
            for item in items {
                held = held.checked_add(summary(item)?)?;
            }
            Some(held)
        }
        Answer::Item { item, comments, .. } => {
            let mut held = rows(comments.len(), COMMENT_BYTES)?.checked_add(summary(item)?)?;
            for comment in comments {
                held = held.checked_add(optional(comment.key.as_deref())?)?.checked_add(bytes(&comment.body)?)?;
            }
            Some(held)
        }
        Answer::Pull(value) => pull(value),
        Answer::Reviews { reviews, .. } => {
            let mut held = rows(reviews.len(), REVIEW_BYTES)?;
            for review in reviews {
                held = held.checked_add(optional(review.key.as_deref())?)?.checked_add(bytes(&review.body)?)?;
            }
            Some(held)
        }
        Answer::Statuses { statuses, .. } | Answer::Checks(statuses) => {
            let mut held = rows(statuses.len(), STATUS_BYTES)?;
            for status in statuses {
                held = held
                    .checked_add(bytes(&status.context)?)?
                    .checked_add(bytes(&status.description)?)?
                    .checked_add(bytes(&status.url)?)?;
            }
            Some(held)
        }
        Answer::Job { step, log, .. } => bytes(log)?.checked_add(match step {
            Some(step) => bytes(&step.name)?,
            None => 0,
        }),
        Answer::File { path, bytes: value, .. } => bytes(path)?.checked_add(bytes(value)?),
        Answer::Remarks { remarks, .. } => {
            let mut held = rows(remarks.len(), REMARK_BYTES)?;
            for remark in remarks {
                held = held.checked_add(bytes(&remark.path)?)?.checked_add(bytes(&remark.body)?)?;
            }
            Some(held)
        }
        Answer::PullFiles { files: values, .. } => files(values),
        Answer::Compare { files: values, commits, .. } => {
            files(values)?.checked_add(rows(commits.len(), COMMIT_BYTES)?)
        }
        Answer::Protection(value) => match value {
            Some(value) => {
                let mut held = rows(value.contexts.len(), STRING_BYTES)?.checked_add(bytes(&value.branch)?)?;
                for context in &value.contexts {
                    held = held.checked_add(bytes(context)?)?;
                }
                Some(held)
            }
            None => Some(0),
        },
        Answer::Settings(value) => bytes(&value.default_branch),
        Answer::Collaborators { collaborators, .. } => rows(collaborators.len(), COLLABORATOR_BYTES),
        Answer::Branches(branches) => {
            let mut held = rows(branches.len(), STRING_BYTES)?;
            for branch in branches {
                held = held.checked_add(bytes(branch)?)?;
            }
            Some(held)
        }
        Answer::Located { .. }
        | Answer::Permission(_)
        | Answer::Commit(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => Some(0),
    }
}
/// Count a pull's strings and reviewers, without a scalar struct width.
#[must_use]
pub fn pull(value: &Pull) -> Option<u64> {
    rows(value.reviewers.len(), ID_BYTES)?
        .checked_add(bytes(&value.head)?)?
        .checked_add(bytes(&value.base)?)?
        .checked_add(match &value.dequeued {
            Some(reason) => bytes(reason)?,
            None => 0,
        })
}

/// Count a run's strings and nested jobs, without a scalar struct width.
#[must_use]
pub fn run(value: &api::Run) -> Option<u64> {
    let mut held =
        bytes(&value.workflow)?.checked_add(bytes(&value.url)?)?.checked_add(rows(value.jobs.len(), RUN_JOB_BYTES)?)?;
    for job in &value.jobs {
        held = held.checked_add(bytes(&job.name)?)?;
        if let Some(step) = &job.step {
            held = held.checked_add(bytes(&step.name)?)?;
        }
    }
    Some(held)
}

fn commit_messages(commits: &[api::CommitMessage]) -> Option<u64> {
    let mut held = rows(commits.len(), size_of::<api::CommitMessage>())?;
    for commit in commits {
        held = held.checked_add(rows(commit.parents.len(), COMMIT_BYTES)?)?.checked_add(bytes(&commit.message)?)?;
    }
    Some(held)
}
