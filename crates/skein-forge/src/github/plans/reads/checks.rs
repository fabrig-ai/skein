//! Status pages, run pages, then sequential failed Actions jobs (forge-github.md, 4.1).
use super::{Bounds, Plan, Read, Response, Stage, Step, api, document, pulls::RUN_PAGES, size};
use crate::{
    github::{plans::reading, request::Request},
    measure,
};
use skein_lib::{List, Time};

// The answer width also covers the pending-job slot allocated beside each status.
const _: () = assert!(
    measure::STATUS_BYTES >= size_of::<api::Status>().saturating_add(size_of::<(u32, u64)>()),
    "status width covers its pending job"
);

#[derive(Debug)]
pub(super) struct State {
    statuses: List<api::Status>,
    jobs: List<(u32, u64)>,
    job: u32,
}
impl State {
    pub(super) fn new() -> Self {
        Self { statuses: List::with_capacity(0), jobs: List::with_capacity(0), job: 0 }
    }
}
impl Plan {
    fn checks_commit(&self) -> Result<api::Commit, api::Error> {
        match self.read {
            Read::Checks { commit } => Ok(commit),
            Read::Locate { .. }
            | Read::CiSetup
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Items { .. }
            | Read::Item { .. }
            | Read::Pull { .. }
            | Read::PullFor { .. }
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
    pub(super) fn checks_admission(&mut self, bounds: Bounds) -> Result<(), api::Error> {
        let capacity = size::capacity(bounds, measure::STATUS_BYTES)?;
        self.limit = size::page_size(bounds, capacity)?;
        // Both retained rows and pending jobs have a fixed capacity from rows and answer bytes.
        self.checks.statuses = List::with_capacity(capacity);
        self.checks.jobs = List::with_capacity(capacity);
        Ok(())
    }
    pub(super) fn checks_initial(&mut self) -> Result<Step, api::Error> {
        self.stage = Stage::Combined;
        self.checks_request()
    }
    fn checks_request(&mut self) -> Result<Step, api::Error> {
        let commit = self.checks_commit()?;
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let owner = row.owner.clone();
        let name = row.name.clone();
        let request = if self.stage == Stage::Combined {
            Request::CombinedStatus { owner, name, commit, page: self.page, per_page: self.limit }
        } else if self.stage == Stage::Runs {
            Request::CheckRuns { owner, name, commit, page: self.page, per_page: self.limit }
        } else if self.stage == Stage::Jobs {
            let (_, id) = self.checks.jobs.get(self.checks.job).ok_or(api::Error::InvalidAnswer)?;
            Request::Job { owner, name, id: *id }
        } else {
            return Err(api::Error::InvalidAnswer);
        };
        Ok(self.send(request))
    }
    pub(super) fn checks_row(&mut self, row: document::Row, bounds: Bounds) -> Result<(), api::Error> {
        let (status, job) = match row {
            document::Row::Context(context) if self.stage == Stage::Combined => (reading::status(context), None),
            document::Row::Run(run) if self.stage == Stage::Runs => {
                let commit = self.checks_commit()?;
                let id = run.id.ok_or(api::Error::InvalidAnswer)?;
                if run.head != Some(commit) {
                    return Err(api::Error::InvalidAnswer);
                }
                let job = if run.check == api::Check::Failed && run.app.as_deref() == Some(b"github-actions") {
                    Some(id)
                } else {
                    None
                };
                let at = match run.completed.or(run.started) {
                    Some(wall) => Time::from_nanos(wall.as_nanos()),
                    None => Time::ZERO,
                };
                (
                    api::Status {
                        at,
                        context: run.name,
                        check: run.check,
                        description: run.title,
                        url: run.url,
                        job: None,
                    },
                    job,
                )
            }
            document::Row::Context(_)
            | document::Row::Run(_)
            | document::Row::Pull(_)
            | document::Row::Log(_)
            | document::Row::Installation(_)
            | document::Row::Rule(_)
            | document::Row::Reachable(_)
            | document::Row::Branch(_)
            | document::Row::SkippedBranch
            | document::Row::Issue(_)
            | document::Row::Comment(_)
            | document::Row::Collaborator(_)
            | document::Row::Remark(_)
            | document::Row::Review(_)
            | document::Row::Commit(_)
            | document::Row::File(_) => return Err(api::Error::InvalidAnswer),
        };
        self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
        if self.count > self.limit || self.checks.statuses.len() >= bounds.rows {
            return Err(api::Error::TooLarge);
        }
        let mut total = self
            .size
            .checked_add(u64::try_from(measure::STATUS_BYTES).ok().ok_or(api::Error::TooLarge)?)
            .ok_or(api::Error::TooLarge)?;
        for bytes in [&status.context, &status.description, &status.url] {
            total = total
                .checked_add(u64::try_from(bytes.len()).ok().ok_or(api::Error::TooLarge)?)
                .ok_or(api::Error::TooLarge)?;
        }
        if total > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        if let Some(id) = job {
            self.checks.jobs.push((self.checks.statuses.len(), id)).ok().ok_or(api::Error::TooLarge)?;
        }
        self.checks.statuses.push(status).ok().ok_or(api::Error::TooLarge)?;
        self.size = total;
        Ok(())
    }
    fn checks_next(&mut self) -> Result<bool, api::Error> {
        let next = match self.response {
            Response::Body { next } => next,
            Response::None | Response::Head | Response::Refusal { .. } => return Err(api::Error::InvalidAnswer),
        };
        if next {
            // Each complete listing, including empty linked pages, is capped at Pull's 50 pages.
            if self.page >= RUN_PAGES {
                return Err(api::Error::TooLarge);
            }
            self.page = self.page.checked_add(1).ok_or(api::Error::TooLarge)?;
        }
        Ok(next)
    }
    pub(super) fn checks_combined_done(&mut self) -> Result<Option<Step>, api::Error> {
        if self.stage != Stage::Combined {
            return Err(api::Error::InvalidAnswer);
        }
        if !self.checks_next()? {
            self.stage = Stage::Runs;
            self.page = 1;
        }
        Ok(Some(self.checks_request()?))
    }
    pub(super) fn checks_runs_done(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if self.stage != Stage::Runs {
            return Err(api::Error::InvalidAnswer);
        }
        if self.checks_next()? {
            return Ok(Some(self.checks_request()?));
        }
        self.stage = Stage::Jobs;
        self.checks_jobs_or_answer(bounds)
    }
    fn checks_jobs_or_answer(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if self.checks.job < self.checks.jobs.len() {
            return Ok(Some(self.checks_request()?));
        }
        let statuses = core::mem::replace(&mut self.checks.statuses, List::with_capacity(0)).into_boxed();
        self.terminal(api::Answer::Checks(statuses), bounds)
    }
    pub(super) fn checks_job_done(&mut self, job: document::Job, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let commit = self.checks_commit()?;
        if self.stage != Stage::Jobs {
            return Err(api::Error::InvalidAnswer);
        }
        let (index, id) = self.checks.jobs.get(self.checks.job).ok_or(api::Error::InvalidAnswer)?;
        if job.id != *id || job.head != commit {
            return Err(api::Error::InvalidAnswer);
        }
        let status = self.checks.statuses.get_mut(*index).ok_or(api::Error::InvalidAnswer)?;
        status.job = Some(api::JobAttempt { head: commit, run: job.run, job: job.id, attempt: job.attempt });
        self.checks.job = self.checks.job.checked_add(1).ok_or(api::Error::TooLarge)?;
        self.checks_jobs_or_answer(bounds)
    }
}
