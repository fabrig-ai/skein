//! The client's allocation measure, accumulated as rows arrive.
use crate::api;
fn length(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("bounded allocation")
}
fn optional(bytes: Option<&[u8]>) -> u64 {
    match bytes {
        Some(bytes) => length(bytes),
        None => 0,
    }
}
fn rows(count: usize, width: usize, limit: u32) -> Result<u64, api::Error> {
    let count = u64::try_from(count).expect("bounded row count");
    if count > u64::from(limit) {
        return Err(api::Error::TooLarge);
    }
    count.checked_mul(u64::try_from(width).expect("row width")).ok_or(api::Error::TooLarge)
}
pub(super) fn summary(item: &api::Summary, limit: u32) -> Result<u64, api::Error> {
    let mut total = rows(item.labels.len(), crate::measure::STRING_BYTES, limit)?;
    for label in &item.labels {
        total = total.checked_add(length(label)).ok_or(api::Error::TooLarge)?;
    }
    total = total.checked_add(optional(item.key.as_deref())).ok_or(api::Error::TooLarge)?;
    total = total.checked_add(length(&item.title)).ok_or(api::Error::TooLarge)?;
    total.checked_add(length(&item.body)).ok_or(api::Error::TooLarge)
}
#[expect(clippy::too_many_lines, reason = "the exhaustive answer measure keeps every answer variant visible")]
pub fn answer(answer: &api::Answer, limit: u32) -> Result<u64, api::Error> {
    let mut total = 0_u64;
    match answer {
        api::Answer::Items { items, .. } => {
            total = rows(items.len(), crate::measure::SUMMARY_BYTES, limit)?;
            for item in items {
                total = total.checked_add(summary(item, limit)?).ok_or(api::Error::TooLarge)?;
            }
        }
        api::Answer::Item { item, comments, .. } => {
            total = rows(comments.len(), crate::measure::COMMENT_BYTES, limit)?
                .checked_add(summary(item, limit)?)
                .ok_or(api::Error::TooLarge)?;
            for comment in comments {
                total = total
                    .checked_add(optional(comment.key.as_deref()))
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(length(&comment.body))
                    .ok_or(api::Error::TooLarge)?;
            }
        }
        api::Answer::Pull(pull) => {
            total = rows(pull.reviewers.len(), crate::measure::ID_BYTES, limit)?
                .checked_add(length(&pull.head))
                .ok_or(api::Error::TooLarge)?
                .checked_add(length(&pull.base))
                .ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Reviews { reviews, .. } => {
            total = rows(reviews.len(), crate::measure::REVIEW_BYTES, limit)?;
            for review in reviews {
                total = total
                    .checked_add(optional(review.key.as_deref()))
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(length(&review.body))
                    .ok_or(api::Error::TooLarge)?;
            }
        }
        api::Answer::Statuses { statuses, .. } | api::Answer::Checks(statuses) => {
            total = rows(statuses.len(), crate::measure::STATUS_BYTES, limit)?;
            for status in statuses {
                total = total
                    .checked_add(length(&status.context))
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(length(&status.description))
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(length(&status.url))
                    .ok_or(api::Error::TooLarge)?;
            }
        }
        api::Answer::Remarks { remarks, .. } => {
            total = rows(remarks.len(), crate::measure::REMARK_BYTES, limit)?;
            for remark in remarks {
                total = total
                    .checked_add(length(&remark.path))
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(length(&remark.body))
                    .ok_or(api::Error::TooLarge)?;
            }
        }
        api::Answer::Branches(branches) => {
            total = rows(branches.len(), crate::measure::STRING_BYTES, limit)?;
            for branch in branches {
                total = total.checked_add(length(branch)).ok_or(api::Error::TooLarge)?;
            }
        }
        api::Answer::Protection(protection) => {
            if let Some(protection) = protection {
                total = rows(protection.contexts.len(), crate::measure::STRING_BYTES, limit)?
                    .checked_add(length(&protection.branch))
                    .ok_or(api::Error::TooLarge)?;
                for context in &protection.contexts {
                    total = total.checked_add(length(context)).ok_or(api::Error::TooLarge)?;
                }
            }
        }
        api::Answer::Settings(settings) => total = length(&settings.default_branch),
        api::Answer::Collaborators { collaborators, .. } => {
            total = rows(collaborators.len(), crate::measure::COLLABORATOR_BYTES, limit)?;
        }
        api::Answer::Runs { runs, .. } => {
            rows(runs.len(), crate::measure::RUN_BYTES, limit)?;
            for run in runs {
                rows(run.jobs.len(), crate::measure::RUN_JOB_BYTES, limit)?;
            }
            total = crate::measure::answer(answer).ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Commits { commits, .. } => {
            rows(commits.len(), size_of::<api::CommitMessage>(), limit)?;
            for commit in commits {
                rows(commit.parents.len(), crate::measure::COMMIT_BYTES, limit)?;
            }
            total = crate::measure::answer(answer).ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Run(run) => {
            rows(run.jobs.len(), crate::measure::RUN_JOB_BYTES, limit)?;
            total = crate::measure::run(run).ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Tree { entries, .. } => {
            rows(entries.len(), crate::measure::ENTRY_BYTES, limit)?;
            total = crate::measure::answer(answer).ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Located { .. } | api::Answer::Commit(_) | api::Answer::Permission(_) => {}
        api::Answer::File { path, bytes, .. } => {
            total = length(path).checked_add(length(bytes)).ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Job { step, log, .. } => {
            total = length(log).checked_add(step_name(step.as_ref())).ok_or(api::Error::TooLarge)?;
        }
        api::Answer::PullFiles { files, .. } => total = files_size(files, limit)?,
        api::Answer::Compare { files, commits, .. } => {
            total = files_size(files, limit)?
                .checked_add(rows(commits.len(), crate::measure::COMMIT_BYTES, limit)?)
                .ok_or(api::Error::TooLarge)?;
        }
        api::Answer::Created(_)
        | api::Answer::Commented(_)
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Queued
        | api::Answer::Branch(_)
        | api::Answer::Done => return Err(api::Error::InvalidAnswer),
    }
    Ok(total)
}
fn files_size(files: &[api::File], limit: u32) -> Result<u64, api::Error> {
    let mut total = rows(files.len(), crate::measure::FILE_BYTES, limit)?;
    for file in files {
        total = total
            .checked_add(length(&file.path))
            .ok_or(api::Error::TooLarge)?
            .checked_add(optional(file.before.as_deref()))
            .ok_or(api::Error::TooLarge)?
            .checked_add(optional(file.after.as_deref()))
            .ok_or(api::Error::TooLarge)?;
    }
    Ok(total)
}

fn step_name(step: Option<&api::Step>) -> u64 {
    match step {
        Some(step) => length(&step.name),
        None => 0,
    }
}
