//! Durable payload accounting and independent answer row admission.
use crate::{Condition, Delivery, Echo, Effect, Stored, What};
use alloc::boxed::Box;
use skein_forge::{api::Answer, measure};

/// Fixed bytes of a saved delivery or echo stamp.
pub const STAMP_BYTES: usize = 16;
/// Fixed bytes of a cached label slot.
pub const LABEL_BYTES: usize = 16;
/// Fixed bytes of a cached reviewer slot.
pub const REVIEWER_BYTES: usize = 8;
const _: () = {
    assert!(STAMP_BYTES >= size_of::<Delivery>(), "stamp width covers delivery storage");
    assert!(STAMP_BYTES >= size_of::<Echo>(), "stamp width covers echo storage");
    assert!(LABEL_BYTES >= size_of::<Box<[u8]>>(), "label width covers string storage");
    assert!(REVIEWER_BYTES >= size_of::<u64>(), "reviewer width covers identifier storage");
    assert!(REVIEWER_BYTES >= 8, "reviewer width covers protocol identifier");
};

/// Count a durable row's strings and fixed list widths.
#[must_use]
pub fn stored(row: &Stored) -> Option<u64> {
    match row {
        Stored::Repository(_) | Stored::Writer(_) => Some(0),
        Stored::Live(row) => {
            let mut held = match &row.watch.resource.what {
                What::Branch(branch) => u64::try_from(branch.len()).ok()?,
                What::Repository | What::Pull(_) | What::Issue(_) => 0,
            };
            for count in [row.comments.len(), row.reviews.len(), row.echoes.len()] {
                held = held.checked_add(u64::try_from(count).ok()?.checked_mul(u64::try_from(STAMP_BYTES).ok()?)?)?;
            }
            if let Some(item) = &row.cached.item {
                held = held.checked_add(measure::summary(item)?)?;
            }
            if let Some(pull) = &row.cached.pull {
                held = held.checked_add(measure::pull(pull)?)?;
            }
            Some(held)
        }
    }
}

/// Count a write together with its client-side condition.
#[must_use]
pub fn effect(effect: &Effect) -> Option<u64> {
    let held = measure::write(&effect.write)?;
    match &effect.condition {
        Condition::Merge { base } => held.checked_add(u64::try_from(base.len()).ok()?),
        Condition::None | Condition::Update { .. } | Condition::Review { .. } => Some(held),
    }
}

/// Check every answer list, including nested lists, independently of bytes.
#[must_use]
pub fn answer_rows(value: &Answer, rows: u32) -> bool {
    let rows = usize::try_from(rows).expect("row limit fits");
    match value {
        Answer::Run(run) => run.jobs.len() <= rows,
        Answer::Runs { runs, .. } => {
            if runs.len() > rows { return false; }
            for run in runs { if run.jobs.len() > rows { return false; } }
            true
        }
        Answer::Tree { entries, .. } => entries.len() <= rows,
        Answer::Items { items, .. } => {
            if items.len() > rows {
                return false;
            }
            for item in items {
                if item.labels.len() > rows {
                    return false;
                }
            }
            true
        }
        Answer::Item { item, comments, .. } => item.labels.len() <= rows && comments.len() <= rows,
        Answer::Pull(pull) => pull.reviewers.len() <= rows,
        Answer::Reviews { reviews, .. } => reviews.len() <= rows,
        Answer::Statuses { statuses, .. } | Answer::Checks(statuses) => statuses.len() <= rows,
        Answer::Remarks { remarks, .. } => remarks.len() <= rows,
        Answer::Branches(branches) => branches.len() <= rows,
        Answer::PullFiles { files, .. } => files.len() <= rows,
        Answer::Compare { files, commits, .. } => files.len() <= rows && commits.len() <= rows,
        Answer::Protection(protection) => match protection {
            Some(value) => value.contexts.len() <= rows,
            None => true,
        },
        Answer::Collaborators { collaborators, .. } => collaborators.len() <= rows,
        Answer::Located { .. }
        | Answer::Commit(_)
        | Answer::Job { .. }
        | Answer::File { .. }
        | Answer::Settings(_)
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        // Only a merge is answered Queued; it never reaches a read.
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => true,
    }
}
