//! Client-side answer admission (forge-client.md, 2.3).
use skein_forge::api::{Answer, Commit, Kind, Op, Pull, Read, Write};
use skein_lib::Time;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Shape {
    Runs,
    Run,
    Tree,
    Located,
    Items,
    Item,
    Pull,
    Reviews,
    Statuses,
    Remarks,
    Commit,
    Branches,
    PullFiles,
    Compare,
    Checks,
    Job,
    File,
    Protection,
    Settings,
    Collaborators,
    Permission,
    Created,
    Commented,
    Reviewed,
    Merged,
    Queued,
    Branch,
    Done,
}
fn shape(answer: &Answer) -> Shape {
    match answer {
        Answer::Runs { .. } => Shape::Runs,
        Answer::Run(_) => Shape::Run,
        Answer::Tree { .. } => Shape::Tree,
        Answer::Located { .. } => Shape::Located,
        Answer::Items { .. } => Shape::Items,
        Answer::Item { .. } => Shape::Item,
        Answer::Pull(_) => Shape::Pull,
        Answer::Reviews { .. } => Shape::Reviews,
        Answer::Statuses { .. } => Shape::Statuses,
        Answer::Remarks { .. } => Shape::Remarks,
        Answer::Commit(_) => Shape::Commit,
        Answer::Branches(_) => Shape::Branches,
        Answer::PullFiles { .. } => Shape::PullFiles,
        Answer::Compare { .. } => Shape::Compare,
        Answer::Checks(_) => Shape::Checks,
        Answer::Job { .. } => Shape::Job,
        Answer::File { .. } => Shape::File,
        Answer::Protection(_) => Shape::Protection,
        Answer::Settings(_) => Shape::Settings,
        Answer::Collaborators { .. } => Shape::Collaborators,
        Answer::Permission(_) => Shape::Permission,
        Answer::Created(_) => Shape::Created,
        Answer::Commented(_) => Shape::Commented,
        Answer::Reviewed(_) => Shape::Reviewed,
        Answer::Merged(_) => Shape::Merged,
        Answer::Branch(_) => Shape::Branch,
        Answer::Queued => Shape::Queued,
        Answer::Done => Shape::Done,
    }
}
pub(crate) fn accepts(op: &Op, answer: &Answer) -> bool {
    let expected = match op {
        Op::Read(read) => match read {
            Read::Runs { .. } => Shape::Runs,
            Read::Run { .. } => Shape::Run,
            Read::Tree { .. } => Shape::Tree,
            Read::Locate { .. } => Shape::Located,
            Read::Items { .. } => Shape::Items,
            Read::Item { .. } => Shape::Item,
            Read::Pull { .. } | Read::PullFor { .. } => Shape::Pull,
            Read::Reviews { .. } => Shape::Reviews,
            Read::Statuses { .. } => Shape::Statuses,
            Read::Remarks { .. } => Shape::Remarks,
            Read::Branch { .. } => Shape::Commit,
            Read::Branches { .. } => Shape::Branches,
            Read::PullFiles { .. } => Shape::PullFiles,
            Read::Compare { .. } => Shape::Compare,
            Read::Checks { .. } => Shape::Checks,
            Read::Job { .. } => Shape::Job,
            Read::File { .. } => Shape::File,
            Read::Protection { .. } => Shape::Protection,
            Read::Settings => Shape::Settings,
            Read::Collaborators { .. } => Shape::Collaborators,
            Read::Permission { .. } => Shape::Permission,
        },
        Op::Write(write) => match write {
            Write::CreateIssue { .. } | Write::OpenPull { .. } => Shape::Created,
            Write::Post { .. } => Shape::Commented,
            Write::Review { .. } => Shape::Reviewed,
            Write::Merge { .. } => Shape::Merged,
            Write::CreateBranch { .. } => Shape::Branch,
            Write::Update { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. }
            | Write::Dispatch { .. }
            | Write::Rerun { .. }
            | Write::DeleteBranch { .. } => Shape::Done,
        },
    };
    if shape(answer) != expected && !(expected == Shape::Merged && shape(answer) == Shape::Queued) {
        return false;
    }
    match op {
        Op::Read(read) => read_accepts(read, answer),
        Op::Write(_) => true,
    }
}
fn read_accepts(read: &Read, answer: &Answer) -> bool {
    match read {
        Read::Runs { workflow, commit, .. } => {
            let Answer::Runs { runs, .. } = answer else {
                return false;
            };
            for (index, run) in runs.iter().enumerate() {
                if run.workflow != *workflow || run.commit != *commit {
                    return false;
                }
                for previous in runs.get(..index).expect("enumerated run") {
                    if previous.id == run.id {
                        return false;
                    }
                }
            }
            true
        }
        Read::Run { run } => {
            let Answer::Run(value) = answer else {
                return false;
            };
            value.id == *run
        }
        Read::Tree { .. } => {
            let Answer::Tree { entries, .. } = answer else {
                return false;
            };
            let mut previous: Option<&skein_forge::api::Entry> = None;
            for entry in entries {
                if let Some(last) = previous
                    && last
                        .name
                        .iter()
                        .chain(tree_suffix(last.kind))
                        .cmp(entry.name.iter().chain(tree_suffix(entry.kind)))
                        != core::cmp::Ordering::Less
                {
                    return false;
                }
                previous = Some(entry);
            }
            true
        }
        Read::Locate { .. } => skein_forge::locate::accepts(answer),
        Read::Items { kind, .. } => items_ordered(answer, *kind),
        Read::Job { attempt, max_bytes } => match answer {
            Answer::Job { attempt: received, log, .. } => {
                received == attempt && log.len() <= usize::try_from(*max_bytes).expect("u32 fits usize")
            }
            Answer::Items { .. }
            | Answer::Runs { .. }
            | Answer::Run(_)
            | Answer::Tree { .. }
            | Answer::Located { .. }
            | Answer::Item { .. }
            | Answer::Pull(_)
            | Answer::Reviews { .. }
            | Answer::Statuses { .. }
            | Answer::Remarks { .. }
            | Answer::Commit(_)
            | Answer::Branches(_)
            | Answer::PullFiles { .. }
            | Answer::Compare { .. }
            | Answer::File { .. }
            | Answer::Checks(_)
            | Answer::Protection(_)
            | Answer::Settings(_)
            | Answer::Collaborators { .. }
            | Answer::Permission(_)
            | Answer::Created(_)
            | Answer::Commented(_)
            | Answer::Reviewed(_)
            | Answer::Merged(_)
            | Answer::Queued
            | Answer::Branch(_)
            | Answer::Done => false,
        },
        Read::File { head, path, max_bytes } => file_matches(answer, *head, path, *max_bytes),
        Read::Checks { commit } => checks_at_head(answer, *commit),
        Read::PullFiles { head, .. } => observed_head(answer) == Some(*head),
        Read::Compare { before, after } => compared(answer, *before, *after),
        Read::Item { number, after } => item_matches(answer, *number, *after),
        Read::Reviews { .. } => reviews_ordered(answer),
        Read::Pull { number } => match pull_answer(answer) {
            Some(pull) => pull.number == *number,
            None => false,
        },
        Read::PullFor { head, base } => match pull_answer(answer) {
            Some(pull) => pull.head == *head && pull.base == *base,
            None => false,
        },
        Read::Branches { prefix } => branch_names_match(answer, prefix),
        Read::Statuses { .. }
        | Read::Remarks { .. }
        | Read::Branch { .. }
        | Read::Protection { .. }
        | Read::Settings
        | Read::Collaborators { .. }
        | Read::Permission { .. } => true,
    }
}

fn branch_names_match(answer: &Answer, prefix: &[u8]) -> bool {
    let Answer::Branches(names) = answer else {
        return false;
    };
    for name in names {
        if !name.starts_with(prefix) {
            return false;
        }
    }
    true
}
fn checks_at_head(answer: &Answer, head: Commit) -> bool {
    match answer {
        Answer::Checks(statuses) => {
            for status in statuses {
                match status.job {
                    Some(job) if job.head != head => return false,
                    Some(_) | None => {}
                }
            }
            true
        }
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn items_ordered(answer: &Answer, kind: Option<Kind>) -> bool {
    match answer {
        Answer::Items { items, .. } => {
            let mut last_time = Time::ZERO;
            let mut last_number = 0;
            for (index, item) in items.iter().enumerate() {
                if item.updated < last_time
                    || (item.updated == last_time && item.number <= last_number)
                    || item.number == 0
                    || (kind.is_some() && kind != Some(item.kind))
                {
                    return false;
                }
                for previous in items.get(..index).expect("enumerated item index") {
                    if previous.number == item.number {
                        return false;
                    }
                }
                last_time = item.updated;
                last_number = item.number;
            }
            true
        }
        Answer::Item { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn reviews_ordered(answer: &Answer) -> bool {
    match answer {
        Answer::Reviews { reviews, .. } => {
            let mut last = 0;
            for review in reviews {
                if review.id <= last {
                    return false;
                }
                last = review.id;
            }
            true
        }
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn item_matches(answer: &Answer, number: u64, after: u64) -> bool {
    match answer {
        Answer::Item { item, comments, .. } => {
            if item.number != number {
                return false;
            }
            let mut last = after;
            for comment in comments {
                if comment.id <= last {
                    return false;
                }
                last = comment.id;
            }
            true
        }
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}
fn pull_answer(answer: &Answer) -> Option<&Pull> {
    match answer {
        Answer::Pull(pull) => Some(pull),
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Item { .. }
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => None,
    }
}
fn observed_head(answer: &Answer) -> Option<Commit> {
    match answer {
        Answer::PullFiles { head, .. } => Some(*head),
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => None,
    }
}
fn compared(answer: &Answer, before: Commit, after: Commit) -> bool {
    match answer {
        Answer::Compare { before: observed_before, after: observed_after, .. } => {
            *observed_before == before && *observed_after == after
        }
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}

fn file_matches(answer: &Answer, head: Commit, path: &[u8], max_bytes: u32) -> bool {
    match answer {
        Answer::File { head: received, path: received_path, bytes, .. } => {
            *received == head
                && received_path.as_ref() == path
                && bytes.len() <= usize::try_from(max_bytes).expect("u32 fits usize")
        }
        Answer::Items { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Tree { .. }
        | Answer::Located { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::PullFiles { .. }
        | Answer::Compare { .. }
        | Answer::Checks(_)
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => false,
    }
}

fn tree_suffix(kind: skein_forge::api::EntryKind) -> &'static [u8] {
    match kind {
        skein_forge::api::EntryKind::Tree => b"/",
        skein_forge::api::EntryKind::Blob | skein_forge::api::EntryKind::Commit => b"",
    }
}
