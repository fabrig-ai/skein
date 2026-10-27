//! Admission validation and payload caps (forge-client.md, section 2.3).
use crate::Limits;
use skein_forge::api::{Answer, Op, Read, Write};
pub(crate) fn op(value: &Op, l: &Limits) -> bool {
    match value {
        Op::Read(Read::Job { max_bytes, .. } | Read::File { max_bytes, .. }) => {
            if *max_bytes == 0 || *max_bytes > l.answer_bytes {
                return false;
            }
        }
        Op::Read(_) | Op::Write(_) => {}
    }
    let valid = match value {
        Op::Read(value) => read(value).is_some(),
        Op::Write(value) => write(value, l).is_some(),
    };
    if !valid {
        return false;
    }
    let held = skein_forge::measure::op(value);
    match held {
        Some(held) => held <= u64::from(l.op_bytes),
        None => false,
    }
}
pub(crate) fn read(value: &Read) -> Option<u64> {
    let valid = match value {
        Read::File { path, .. } => !path.is_empty(),
        Read::Items { page, .. }
        | Read::Reviews { page, .. }
        | Read::Statuses { page, .. }
        | Read::Remarks { page, .. }
        | Read::PullFiles { page, .. }
        | Read::Collaborators { page }
        | Read::Runs { page, .. }
        | Read::Tree { page, .. } => *page != 0,
        Read::Locate { owner, name } => return skein_forge::locate::bytes(owner, name),
        Read::PullFor { .. }
        | Read::Branch { .. }
        | Read::Protection { .. }
        | Read::Run { .. }
        | Read::Job { .. }
        | Read::Item { .. }
        | Read::Pull { .. }
        | Read::Compare { .. }
        | Read::Checks { .. }
        | Read::Branches { .. }
        | Read::Settings
        | Read::Permission { .. } => true,
    };
    if valid { Some(0) } else { None }
}
pub(crate) fn write(value: &Write, l: &Limits) -> Option<u64> {
    match value {
        Write::Dispatch { workflow, branch, inputs } => {
            if workflow.is_empty() || branch.is_empty() || inputs.len() > skein_forge::api::INPUTS {
                return None;
            }
            for input in inputs {
                if input.name.is_empty() {
                    return None;
                }
            }
        }
        Write::Status { description, .. } => {
            if description.len() > skein_forge::api::DESCRIPTION_BYTES {
                return None;
            }
        }
        Write::SetReviewers { reviewers, .. } => {
            if u64::try_from(reviewers.len()).ok()? > u64::from(l.rows) {
                return None;
            }
        }
        Write::Rerun { .. }
        | Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Edit { .. }
        | Write::CreateBranch { .. }
        | Write::DeleteBranch { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Update { .. }
        | Write::Merge { .. } => {}
    }
    skein_forge::measure::write(value)
}
pub(crate) fn answer(value: &Answer, l: &Limits) -> bool {
    crate::measure::answer_rows(value, l.rows) && within(skein_forge::measure::answer(value), l.answer_bytes)
}
pub(crate) fn cached(value: &crate::Cached, l: &Limits) -> bool {
    let item = match &value.item {
        Some(item) => {
            item.labels.len() <= usize::try_from(l.rows).expect("row limit fits")
                && within(skein_forge::measure::summary(item), l.answer_bytes)
        }
        None => true,
    };
    let pull = match &value.pull {
        Some(pull) => {
            pull.reviewers.len() <= usize::try_from(l.rows).expect("row limit fits")
                && within(skein_forge::measure::pull(pull), l.answer_bytes)
        }
        None => true,
    };
    item && pull
}

fn within(bytes: Option<u64>, cap: u32) -> bool {
    match bytes {
        Some(bytes) => bytes <= u64::from(cap),
        None => false,
    }
}
