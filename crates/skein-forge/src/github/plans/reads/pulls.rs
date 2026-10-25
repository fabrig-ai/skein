//! Pull lookup and bounded CI page folding (forge-github.md, 4.1 and 4.4).
use super::{Bounds, Plan, Read, Response, Stage, Step, api, document, size};
use crate::{
    github::{plans::reading, request::Request},
    measure,
};
use skein_lib::List;

// Check runs are folded without retaining rows, but at most 50 pages are read.
pub(super) const RUN_PAGES: u32 = 50;
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "listing preference, GraphQL selection, merged fallback and independent CI fold flags"
)]
pub(super) struct State {
    pull: Option<api::Pull>,
    number: Option<u64>,
    open: bool,
    graphql: bool,
    merged_missing: bool,
    failed: bool,
    pending: bool,
    present: bool,
    statuses: List<api::Status>,
}
impl State {
    pub(super) fn new() -> Self {
        Self {
            pull: None,
            number: None,
            open: true,
            graphql: false,
            merged_missing: false,
            failed: false,
            pending: false,
            present: false,
            statuses: List::with_capacity(0),
        }
    }
    fn fold(&mut self, check: api::Check) {
        self.present = true;
        match check {
            api::Check::Failed => self.failed = true,
            api::Check::Pending => self.pending = true,
            api::Check::Passed => {}
        }
    }
    fn ci(&self) -> api::Ci {
        reading::ci(self.failed, self.pending, self.present)
    }
}
impl Plan {
    pub(super) fn pull_admission(&mut self, bounds: Bounds) -> Result<(), api::Error> {
        let capacity = size::capacity(bounds, measure::STATUS_BYTES)?;
        self.limit = size::page_size(bounds, capacity)?;
        if is_statuses(&self.read) {
            self.pulls.statuses = List::with_capacity(capacity);
        }
        Ok(())
    }
    pub(super) fn pull_initial(&mut self) -> Result<Step, api::Error> {
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let owner = row.owner.clone();
        let name = row.name.clone();
        let request = match &self.read {
            Read::Pull { number } => {
                self.stage = Stage::Read;
                self.pulls.number = Some(*number);
                Request::Pull { owner, name, number: *number }
            }
            Read::PullFor { head, base } => {
                self.stage = Stage::Listing;
                Request::PullsFor { owner, name, head: head.clone(), base: base.clone(), open: self.pulls.open }
            }
            Read::Statuses { commit, page } => {
                self.stage = Stage::Read;
                Request::CombinedStatus { owner, name, commit: *commit, page: *page, per_page: self.limit }
            }
            Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Items { .. }
            | Read::Item { .. }
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Checks { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => return Err(api::Error::InvalidAnswer),
        };
        Ok(self.send(request))
    }
    pub(super) fn pull_row(&mut self, row: document::Row, bounds: Bounds) -> Result<(), api::Error> {
        match row {
            document::Row::Pull(pull) if self.stage == Stage::Listing => {
                if self.pulls.number.replace(pull.number).is_some() {
                    return Err(api::Error::InvalidAnswer);
                }
            }
            // Pull's combined request uses only its envelope; contexts are streamed and dropped here.
            document::Row::Context(_) if self.stage == Stage::Combined => {}
            document::Row::Context(context) if self.stage == Stage::Read && is_statuses(&self.read) => {
                self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
                if self.count > self.limit || self.pulls.statuses.len() >= bounds.rows {
                    return Err(api::Error::TooLarge);
                }
                let status = reading::status(context);
                let width = u64::try_from(measure::STATUS_BYTES).ok().ok_or(api::Error::TooLarge)?;
                let mut total = self.size.checked_add(width).ok_or(api::Error::TooLarge)?;
                for bytes in [&status.context, &status.description, &status.url] {
                    total = total
                        .checked_add(u64::try_from(bytes.len()).ok().ok_or(api::Error::TooLarge)?)
                        .ok_or(api::Error::TooLarge)?;
                }
                if total > u64::from(bounds.answer_bytes) {
                    return Err(api::Error::TooLarge);
                }
                if self.pulls.statuses.push(status).is_err() {
                    return Err(api::Error::TooLarge);
                }
                self.size = total;
            }
            document::Row::Run(run) if self.stage == Stage::Runs => {
                self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
                if self.count > self.limit {
                    return Err(api::Error::TooLarge);
                }
                self.pulls.fold(run.check);
            }
            document::Row::Pull(_)
            | document::Row::Context(_)
            | document::Row::Run(_)
            | document::Row::Issue(_)
            | document::Row::Comment(_)
            | document::Row::Branch(_)
            | document::Row::SkippedBranch
            | document::Row::Log(_)
            | document::Row::Installation(_)
            | document::Row::Commit(_)
            | document::Row::File(_)
            | document::Row::Review(_)
            | document::Row::Remark(_)
            | document::Row::Collaborator(_)
            | document::Row::Rule(_)
            | document::Row::Reachable(_) => return Err(api::Error::InvalidAnswer),
        }
        Ok(())
    }
    pub(super) fn pull_done(&mut self, mut pull: document::Pull, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if self.stage != Stage::Read || Some(pull.number) != self.pulls.number {
            return Err(api::Error::InvalidAnswer);
        }
        match &self.read {
            Read::Pull { .. } => {}
            Read::PullFor { head, base } => {
                if pull.head != *head || pull.base != *base {
                    return Err(api::Error::InvalidAnswer);
                }
            }
            Read::Statuses { .. }
            | Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Items { .. }
            | Read::Item { .. }
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Checks { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => return Err(api::Error::InvalidAnswer),
        }
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        self.pulls.merged_missing = pull.merged && pull.merge_commit.is_none();
        self.pulls.graphql = (!pull.closed && !pull.merged && row.organization) || self.pulls.merged_missing;
        // The GraphQL fallback supplies the landed commit before the answer is exposed.
        if self.pulls.merged_missing {
            pull.merged = false;
            pull.closed = true;
        }
        let pull = reading::pull(pull, api::Ci::None)?;
        if measure::pull(&pull).ok_or(api::Error::TooLarge)? > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        let commit = pull.commit;
        self.pulls.pull = Some(pull);
        self.stage = Stage::Combined;
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        Ok(Some(self.send(Request::CombinedStatus {
            owner: row.owner.clone(),
            name: row.name.clone(),
            commit,
            page: 1,
            per_page: 1,
        })))
    }
    pub(super) fn combined_done(
        &mut self,
        combined: document::Combined,
        bounds: Bounds,
    ) -> Result<Option<Step>, api::Error> {
        if self.stage != Stage::Combined && !(self.stage == Stage::Read && is_statuses(&self.read)) {
            return Err(api::Error::InvalidAnswer);
        }
        if combined.total > 0 {
            self.pulls.fold(combined.check);
        }
        if self.stage == Stage::Combined {
            self.stage = Stage::Runs;
            self.page = 1;
            return self.runs_request();
        }
        let more = self.pull_next()?;
        let statuses = core::mem::replace(&mut self.pulls.statuses, List::with_capacity(0)).into_boxed();
        self.terminal(api::Answer::Statuses { ci: self.pulls.ci(), statuses, more }, bounds)
    }
    fn runs_request(&mut self) -> Result<Option<Step>, api::Error> {
        let commit = self.pulls.pull.as_ref().ok_or(api::Error::InvalidAnswer)?.commit;
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        Ok(Some(self.send(Request::CheckRuns {
            owner: row.owner.clone(),
            name: row.name.clone(),
            commit,
            page: self.page,
            per_page: self.limit,
        })))
    }
    fn pull_next(&self) -> Result<bool, api::Error> {
        match self.response {
            Response::Body { next } => Ok(next),
            Response::None | Response::Head | Response::Refusal { .. } => Err(api::Error::InvalidAnswer),
        }
    }
    pub(super) fn pull_rows_done(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if self.stage == Stage::Listing {
            if let Some(number) = self.pulls.number {
                self.stage = Stage::Read;
                let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
                return Ok(Some(self.send(Request::Pull { owner: row.owner.clone(), name: row.name.clone(), number })));
            }
            if !self.pulls.open {
                return Err(api::Error::Missing);
            }
            self.pulls.open = false;
            return Ok(Some(self.pull_initial()?));
        }
        if self.stage != Stage::Runs {
            return Err(api::Error::InvalidAnswer);
        }
        if self.pull_next()? {
            if self.page >= RUN_PAGES {
                return Err(api::Error::TooLarge);
            }
            self.page = self.page.checked_add(1).ok_or(api::Error::TooLarge)?;
            return self.runs_request();
        }
        if self.pulls.graphql {
            self.stage = Stage::PullGraphql;
            let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
            return Ok(Some(self.send(Request::PullGraphql {
                owner: row.owner.clone(),
                name: row.name.clone(),
                number: self.pulls.number.ok_or(api::Error::InvalidAnswer)?,
            })));
        }
        self.pull_answer(bounds)
    }
    pub(super) fn pull_graphql_done(
        &mut self,
        value: document::PullGraphql,
        bounds: Bounds,
    ) -> Result<Option<Step>, api::Error> {
        if self.stage != Stage::PullGraphql {
            return Err(api::Error::InvalidAnswer);
        }
        let pull = self.pulls.pull.as_mut().ok_or(api::Error::InvalidAnswer)?;
        if self.pulls.merged_missing {
            pull.merged = Some(value.merge_commit.ok_or(api::Error::InvalidAnswer)?);
        }
        pull.queued = value.queued;
        pull.dequeued = value.dequeued;
        self.pull_answer(bounds)
    }
    fn pull_answer(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let mut pull = self.pulls.pull.take().ok_or(api::Error::InvalidAnswer)?;
        pull.ci = self.pulls.ci();
        self.terminal(api::Answer::Pull(pull), bounds)
    }
}
fn is_statuses(read: &Read) -> bool {
    match read {
        Read::Statuses { .. } => true,
        Read::Pull { .. }
        | Read::PullFor { .. }
        | Read::Locate { .. }
        | Read::CiSetup
        | Read::Settings
        | Read::Branch { .. }
        | Read::Branches { .. }
        | Read::Items { .. }
        | Read::Item { .. }
        | Read::Permission { .. }
        | Read::Collaborators { .. }
        | Read::Remarks { .. }
        | Read::Reviews { .. }
        | Read::File { .. }
        | Read::Compare { .. }
        | Read::PullFiles { .. }
        | Read::Checks { .. }
        | Read::Protection { .. }
        | Read::Job { .. } => false,
    }
}
