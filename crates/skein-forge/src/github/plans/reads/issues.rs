//! Bounded issue listings and comments paged by Link (forge-github.md, 4.1).
use super::{Bounds, Plan, Read, Response, Stage, Step, api, document, size};
use crate::{
    github::{plans::reading, request::Request},
    measure,
};
use skein_lib::{List, Time};

#[derive(Debug)]
pub(super) struct State {
    items: List<api::Summary>,
    comments: List<api::Comment>,
    item: Option<api::Summary>,
    more: bool,
    now: Time,
}
impl State {
    pub(super) fn set_now(&mut self, now: Time) {
        self.now = now;
    }
    pub(super) fn new() -> Self {
        Self {
            items: List::with_capacity(0),
            comments: List::with_capacity(0),
            item: None,
            more: false,
            now: Time::ZERO,
        }
    }
}
impl Plan {
    pub(super) fn issue_admission(&mut self, bounds: Bounds) -> Result<(), api::Error> {
        match self.read {
            Read::Items { .. } => {
                let capacity = size::capacity(bounds, measure::SUMMARY_BYTES)?;
                self.limit = size::page_size(bounds, capacity)?;
                self.issues.items = List::with_capacity(capacity);
            }
            Read::Item { .. } => {
                let capacity = size::capacity(bounds, measure::COMMENT_BYTES)?;
                self.limit = size::page_size(bounds, capacity)?;
                self.issues.comments = List::with_capacity(capacity);
            }
            Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Pull { .. }
            | Read::PullFor { .. }
            | Read::Checks { .. }
            | Read::Statuses { .. }
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => {
                return Err(api::Error::InvalidAnswer);
            }
        }
        Ok(())
    }
    #[expect(clippy::too_many_lines, reason = "the row dispatcher exhaustively handles each document kind")]
    pub(super) fn issue_row(&mut self, row: document::Row, bounds: Bounds, word: &[u8]) -> Result<(), api::Error> {
        if self.stage != Stage::Read && self.stage != Stage::Comments {
            return Err(api::Error::InvalidAnswer);
        }
        self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
        if self.count > self.limit {
            return Err(api::Error::TooLarge);
        }
        match (&self.read, row) {
            (Read::Items { kind, .. }, document::Row::Issue(issue)) if self.stage == Stage::Read => {
                let wanted = match kind {
                    None => true,
                    Some(api::Kind::Issue) => !issue.pull,
                    Some(api::Kind::Pull) => issue.pull,
                };
                if !wanted {
                    return Ok(());
                }
                let item = reading::summary(word, issue);
                let width = u64::try_from(measure::SUMMARY_BYTES).ok().ok_or(api::Error::TooLarge)?;
                let measured = measure::summary(&item).ok_or(api::Error::TooLarge)?;
                let total = self
                    .size
                    .checked_add(width)
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(measured)
                    .ok_or(api::Error::TooLarge)?;
                if self.issues.items.len() >= bounds.rows || total > u64::from(bounds.answer_bytes) {
                    return Err(api::Error::TooLarge);
                }
                if self.issues.items.push(item).is_err() {
                    return Err(api::Error::TooLarge);
                }
                self.size = total;
            }
            (Read::Item { after, .. }, document::Row::Comment(comment)) if self.stage == Stage::Comments => {
                if self.issues.more || comment.id <= *after {
                    return Ok(());
                }
                let comment = reading::comment(word, comment)?;
                let width = u64::try_from(measure::COMMENT_BYTES).ok().ok_or(api::Error::TooLarge)?;
                let key = match &comment.key {
                    Some(key) => key.len(),
                    None => 0,
                };
                let key = u64::try_from(key).ok().ok_or(api::Error::TooLarge)?;
                let body = u64::try_from(comment.body.len()).ok().ok_or(api::Error::TooLarge)?;
                let total = self
                    .size
                    .checked_add(width)
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(key)
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(body)
                    .ok_or(api::Error::TooLarge)?;
                if self.issues.comments.len() >= bounds.rows || total > u64::from(bounds.answer_bytes) {
                    self.issues.more = true;
                } else {
                    if self.issues.comments.push(comment).is_err() {
                        return Err(api::Error::TooLarge);
                    }
                    self.size = total;
                }
            }
            (
                Read::Items { .. }
                | Read::Item { .. }
                | Read::Locate { .. }
                | Read::CiSetup
                | Read::Settings
                | Read::Branch { .. }
                | Read::Branches { .. }
                | Read::Pull { .. }
                | Read::PullFor { .. }
                | Read::Checks { .. }
                | Read::Statuses { .. }
                | Read::Permission { .. }
                | Read::Collaborators { .. }
                | Read::Remarks { .. }
                | Read::Reviews { .. }
                | Read::File { .. }
                | Read::Compare { .. }
                | Read::PullFiles { .. }
                | Read::Protection { .. }
                | Read::Job { .. },
                document::Row::Issue(_)
                | document::Row::Comment(_)
                | document::Row::Log(_)
                | document::Row::Installation(_)
                | document::Row::Commit(_)
                | document::Row::File(_)
                | document::Row::Review(_)
                | document::Row::Remark(_)
                | document::Row::Collaborator(_)
                | document::Row::Rule(_)
                | document::Row::Reachable(_)
                | document::Row::Pull(_)
                | document::Row::Context(_)
                | document::Row::Run(_)
                | document::Row::Branch(_)
                | document::Row::SkippedBranch,
            ) => return Err(api::Error::InvalidAnswer),
        }
        Ok(())
    }
    pub(super) fn issue_done(
        &mut self,
        issue: document::Issue,
        bounds: Bounds,
        word: &[u8],
    ) -> Result<Option<Step>, api::Error> {
        let number = match self.read {
            Read::Item { number, .. } if self.stage == Stage::Read => number,
            Read::Item { .. }
            | Read::Items { .. }
            | Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Pull { .. }
            | Read::PullFor { .. }
            | Read::Checks { .. }
            | Read::Statuses { .. }
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => return Err(api::Error::InvalidAnswer),
        };
        if issue.number != number {
            return Err(api::Error::InvalidAnswer);
        }
        let item = reading::summary(word, issue);
        self.size = measure::summary(&item).ok_or(api::Error::TooLarge)?;
        if self.size > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        self.issues.item = Some(item);
        self.stage = Stage::Comments;
        self.comments_request()
    }
    fn comments_request(&mut self) -> Result<Option<Step>, api::Error> {
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let number = match self.read {
            Read::Item { number, .. } => number,
            Read::Items { .. }
            | Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Pull { .. }
            | Read::PullFor { .. }
            | Read::Checks { .. }
            | Read::Statuses { .. }
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => return Err(api::Error::InvalidAnswer),
        };
        Ok(Some(self.send(Request::Comments {
            owner: row.owner.clone(),
            name: row.name.clone(),
            number,
            page: self.page,
            per_page: self.limit,
        })))
    }
    pub(super) fn issue_rows_done(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let next = match self.response {
            Response::Body { next } => next,
            Response::None | Response::Head | Response::Refusal { .. } => return Err(api::Error::InvalidAnswer),
        };
        match self.read {
            Read::Items { .. } if self.stage == Stage::Read => {
                let mut items = core::mem::replace(&mut self.issues.items, List::with_capacity(0)).into_boxed();
                // Only contiguous equal-time runs are sorted: times remain the forge's order.
                order(&mut items)?;
                self.terminal(api::Answer::Items { items, more: next, now: self.issues.now }, bounds)
            }
            Read::Item { .. } if self.stage == Stage::Comments => {
                if !self.issues.more && next {
                    self.page = self.page.checked_add(1).ok_or(api::Error::TooLarge)?;
                    return self.comments_request();
                }
                let item = self.issues.item.take().ok_or(api::Error::InvalidAnswer)?;
                let comments = core::mem::replace(&mut self.issues.comments, List::with_capacity(0)).into_boxed();
                self.terminal(api::Answer::Item { item, comments, more: self.issues.more }, bounds)
            }
            Read::Items { .. }
            | Read::Item { .. }
            | Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Pull { .. }
            | Read::PullFor { .. }
            | Read::Checks { .. }
            | Read::Statuses { .. }
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => Err(api::Error::InvalidAnswer),
        }
    }
}
// Insertion moves a row only across larger numbers of the same contiguous time run.
fn order(items: &mut [api::Summary]) -> Result<(), api::Error> {
    for end in 1..items.len() {
        for right in (1..=end).rev() {
            let left = right.checked_sub(1).ok_or(api::Error::InvalidAnswer)?;
            let a = items.get(left).ok_or(api::Error::InvalidAnswer)?;
            let b = items.get(right).ok_or(api::Error::InvalidAnswer)?;
            if a.updated != b.updated || a.number <= b.number {
                break;
            }
            let [a, b] = items.get_disjoint_mut([left, right]).ok().ok_or(api::Error::InvalidAnswer)?;
            core::mem::swap(a, b);
        }
    }
    Ok(())
}
