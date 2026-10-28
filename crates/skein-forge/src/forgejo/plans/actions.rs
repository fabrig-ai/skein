//! Actions reads: checks' job attempts and a verified attempt's bounded log.
use crate::api;
use crate::forgejo::plans::{
    Body, Heard,
    plan::{Bounds, Exchange, Step},
    reading,
    tables::{Repository, Tables},
};
use crate::forgejo::{document as d, header, id::ObjectId, request as r};
use alloc::boxed::Box;
use skein_lib::List;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Checks(api::Commit),
    Statuses { head: api::Commit, ci: api::Ci, more: bool },
    Job { attempt: api::JobAttempt, max: u32 },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Checks,
    RunLookup,
    JobsLookup,
    Run,
    Job,
    Log,
}
#[derive(Debug)]
pub struct Plan {
    mode: Mode,
    stage: Stage,
    row: Repository,
    statuses: List<api::Status>,
    count: u32,
    page: u32,
    limit: u32,
    position: u32,
    run_number: u64,
    job_index: u32,
    run: Option<u64>,
    step: Option<api::Step>,
    missing: bool,
    truncated: bool,
    resolve_jobs: bool,
    size: u64,
}
impl Plan {
    pub fn new(read: api::Read, row: Repository, bounds: Bounds) -> Result<Self, api::Error> {
        let (mode, stage, page) = match read {
            api::Read::Checks { commit } => (Mode::Checks(commit), Stage::Checks, 1),
            api::Read::Statuses { commit, page } => {
                (Mode::Statuses { head: commit, ci: api::Ci::None, more: false }, Stage::Checks, page)
            }
            api::Read::Job { attempt, max_bytes } => (Mode::Job { attempt, max: max_bytes }, Stage::Run, 1),
            api::Read::Locate { .. }
            | api::Read::Items { .. }
            | api::Read::Item { .. }
            | api::Read::Pull { .. }
            | api::Read::PullFor { .. }
            | api::Read::Reviews { .. }
            | api::Read::Remarks { .. }
            | api::Read::Branch { .. }
            | api::Read::Branches { .. }
            | api::Read::Protection { .. }
            | api::Read::Settings
            | api::Read::Collaborators { .. }
            | api::Read::Permission { .. }
            | api::Read::PullFiles { .. }
            | api::Read::Compare { .. }
            | api::Read::File { .. } => return Err(api::Error::InvalidAnswer),
        };
        let room = bounds
            .answer_bytes
            .checked_div(u32::try_from(crate::measure::STATUS_BYTES).expect("status width"))
            .expect("nonzero width");
        let capacity = match mode {
            Mode::Checks(_) => bounds.rows.min(room),
            Mode::Statuses { .. } => bounds.page_size.min(bounds.rows).min(room),
            Mode::Job { .. } => 0,
        };
        Ok(Self {
            mode,
            stage,
            row,
            statuses: List::with_capacity(capacity),
            count: 0,
            page,
            limit: bounds.page_size,
            position: 0,
            run_number: 0,
            job_index: 0,
            run: None,
            step: None,
            missing: false,
            truncated: false,
            resolve_jobs: true,
            size: 0,
        })
    }
    pub fn statuses(commit: api::Commit, row: Repository, bounds: Bounds) -> Result<Self, api::Error> {
        let mut plan = Self::new(api::Read::Checks { commit }, row, bounds)?;
        plan.resolve_jobs = false;
        Ok(plan)
    }
    fn at(&self) -> r::Repo {
        r::Repo { owner: self.row.owner.clone(), name: self.row.name.clone() }
    }
    fn id(&self, bytes: api::Commit) -> ObjectId {
        ObjectId { bytes, format: self.row.object_format }
    }
    pub fn begin(&mut self) -> Step {
        match self.mode {
            Mode::Checks(commit) | Mode::Statuses { head: commit, .. } => self.send(r::Request::CombinedStatus {
                at: self.at(),
                commit: self.id(commit),
                page: r::Page { page: Some(self.page), limit: Some(self.limit) },
            }),
            Mode::Job { attempt, .. } => self.send(r::Request::Run { at: self.at(), run: attempt.run }),
        }
    }
    fn send(&mut self, request: r::Request) -> Step {
        self.count = 0;
        self.missing = false;
        Step::send(request)
    }
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        repository: api::Repository,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, fields } => {
                self.missing = status == 404 && (self.stage == Stage::RunLookup || self.stage == Stage::JobsLookup);
                if self.missing {
                    return Ok(None);
                }
                if status == 404 || (300..400).contains(&status) {
                    tables.drop_repository(repository);
                }
                if status == 404 {
                    match self.mode {
                        Mode::Job { .. } => return Err(api::Error::MissingJob),
                        Mode::Checks(_) | Mode::Statuses { .. } => {}
                    }
                }
                if status != 200 && !(self.stage == Stage::Log && status == 206) {
                    return Err(crate::forgejo::plans::failure::read(status, &fields));
                }
                if self.stage == Stage::Log
                    && let Some(range) = fields.content_range
                {
                    self.truncated = range.first > 0;
                }
                Ok(None)
            }
            Heard::Row(row) => {
                self.row(row, bounds, repository.forge, tables)?;
                Ok(None)
            }
            Heard::Done(body) => {
                if self.missing {
                    return self.next_check();
                }
                match body {
                    Body::Document(document) => self.done(document),
                    Body::Raw { bytes, more } => self.log(bytes, more),
                    Body::Malformed(crate::forgejo::decode::Malformed::Bound { field }) => {
                        Err(if field == b"log" { api::Error::InvalidAnswer } else { api::Error::TooLarge })
                    }
                    Body::Empty | Body::Malformed(_) => Err(api::Error::InvalidAnswer),
                }
            }
            Heard::Failed(failure) => Err(crate::forgejo::plans::failure::transport(failure)),
        }
    }
    fn row(&mut self, row: d::Row, bounds: Bounds, forge: u16, tables: &mut Tables) -> Result<(), api::Error> {
        self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
        match row {
            d::Row::Status(status) => {
                if self.stage != Stage::Checks {
                    return Err(api::Error::InvalidAnswer);
                }
                if self.count > self.limit {
                    return Err(api::Error::TooLarge);
                }
                if let Some(user) = &status.creator {
                    tables.keep_user(forge, user.id, user.login.clone());
                }
                let value = reading::status(status, &self.row.html_url)?;
                let answer = api::Answer::Checks(Box::new([value.clone()]));
                self.size = self
                    .size
                    .checked_add(crate::forgejo::plans::reads::size::answer(&answer, bounds.rows)?)
                    .ok_or(api::Error::TooLarge)?;
                if self.size > u64::from(bounds.answer_bytes) {
                    return Err(api::Error::TooLarge);
                }
                match self.statuses.push(value) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(api::Error::TooLarge),
                }
            }
            d::Row::Run(run) => {
                if self.stage != Stage::RunLookup {
                    return Err(api::Error::InvalidAnswer);
                }
                if self.count > 1 {
                    return Err(api::Error::TooLarge);
                }
                let commit =
                    reading::commit(run.commit_sha, self.row.object_format).ok_or(api::Error::InvalidAnswer)?;
                match self.mode {
                    Mode::Checks(head) | Mode::Statuses { head, .. } => {
                        if commit == head && run.index_in_repo == self.run_number {
                            self.run = Some(run.id);
                        }
                    }
                    Mode::Job { .. } => return Err(api::Error::InvalidAnswer),
                }
                Ok(())
            }
            d::Row::Job(job) => {
                match self.mode {
                    Mode::Checks(head) | Mode::Statuses { head, .. } => {
                        if self.stage != Stage::JobsLookup {
                            return Err(api::Error::InvalidAnswer);
                        }
                        if self.count.saturating_sub(1) == self.job_index && Some(job.run_id) == self.run {
                            self.statuses.get_mut(self.position.saturating_sub(1)).expect("lookup status").job =
                                Some(api::JobAttempt { head, run: job.run_id, job: job.id, attempt: job.attempt });
                        }
                    }
                    Mode::Job { .. } => return Err(api::Error::InvalidAnswer),
                }
                Ok(())
            }
            d::Row::Step(step) => {
                if self.stage != Stage::Job {
                    return Err(api::Error::InvalidAnswer);
                }
                if self.step.is_none() && step.status == d::StepStatus::Failure {
                    self.step = Some(api::Step { number: step.number, name: step.name });
                }
                Ok(())
            }
            d::Row::Found(_)
            | d::Row::Reachable(_)
            | d::Row::Token(_)
            | d::Row::User(_)
            | d::Row::Issue(_)
            | d::Row::Comment(_)
            | d::Row::Review(_)
            | d::Row::ReviewComment(_)
            | d::Row::Pull(_)
            | d::Row::Branch(_)
            | d::Row::Protection(_)
            | d::Row::File(_)
            | d::Row::Hook(_)
            | d::Row::Team(_)
            | d::Row::Commit(_) => Err(api::Error::InvalidAnswer),
        }
    }
    #[expect(clippy::manual_map, reason = "Step functions use explicit matches without closures")]
    fn done(&mut self, document: d::Document) -> Result<Option<Step>, api::Error> {
        match document {
            d::Document::CombinedStatus(combined) => {
                if self.stage != Stage::Checks {
                    return Err(api::Error::InvalidAnswer);
                }
                if let Mode::Statuses { head, .. } = self.mode {
                    let read = u64::from(self.page.saturating_sub(1))
                        .saturating_mul(u64::from(self.limit))
                        .saturating_add(u64::from(self.count));
                    self.mode = Mode::Statuses {
                        head,
                        ci: reading::ci(combined.state, combined.total_count),
                        more: read < combined.total_count,
                    };
                } else if self.count == self.limit {
                    self.page = self.page.checked_add(1).ok_or(api::Error::TooLarge)?;
                    return Ok(Some(self.begin()));
                }
                self.next_check()
            }
            d::Document::Runs { .. } => {
                if self.stage != Stage::RunLookup {
                    return Err(api::Error::InvalidAnswer);
                }
                match self.run {
                    Some(run) => {
                        self.stage = Stage::JobsLookup;
                        Ok(Some(self.send(r::Request::Jobs { at: self.at(), run })))
                    }
                    None => self.next_check(),
                }
            }
            d::Document::Run(run) => {
                let attempt = match self.mode {
                    Mode::Job { attempt, .. } => attempt,
                    Mode::Checks(_) | Mode::Statuses { .. } => return Err(api::Error::InvalidAnswer),
                };
                if self.stage != Stage::Run {
                    return Err(api::Error::InvalidAnswer);
                }
                let head = reading::commit(run.commit_sha, self.row.object_format).ok_or(api::Error::InvalidAnswer)?;
                if head != attempt.head || run.id != attempt.run {
                    return Err(api::Error::MissingJob);
                }
                self.stage = Stage::Job;
                Ok(Some(self.send(r::Request::Job { at: self.at(), job: attempt.job })))
            }
            d::Document::Listed { .. } => {
                if self.stage != Stage::JobsLookup {
                    return Err(api::Error::InvalidAnswer);
                }
                self.next_check()
            }
            d::Document::Job(job) => {
                if self.stage != Stage::Job {
                    return Err(api::Error::InvalidAnswer);
                }
                let (attempt, max) = match self.mode {
                    Mode::Job { attempt, max } => (attempt, max),
                    Mode::Checks(_) | Mode::Statuses { .. } => return Err(api::Error::InvalidAnswer),
                };
                if job.id != attempt.job || job.run_id != attempt.run || job.attempt != attempt.attempt {
                    return Err(api::Error::MissingJob);
                }
                self.stage = Stage::Log;
                Ok(Some(Step::Send(Exchange {
                    request: r::Request::JobLog {
                        at: self.at(),
                        job: attempt.job,
                        attempt: attempt.attempt,
                        step: match &self.step {
                            Some(step) => Some(step.number),
                            None => None,
                        },
                    },
                    range: Some(crate::forgejo::plans::plan::Range::Tail(max)),
                })))
            }
            d::Document::Token(_)
            | d::Document::User(_)
            | d::Document::Org(_)
            | d::Document::CreatedFile { .. }
            | d::Document::CreatedRepository(_)
            | d::Document::Hook(_)
            | d::Document::Team(_)
            | d::Document::ApiSettings(_)
            | d::Document::Repository(_)
            | d::Document::RepoSearch { .. }
            | d::Document::Search { .. }
            | d::Document::Issue(_)
            | d::Document::Comment(_)
            | d::Document::Status(_)
            | d::Document::Review(_)
            | d::Document::Pull(_)
            | d::Document::Branch(_)
            | d::Document::Protection(_)
            | d::Document::Permission(_)
            | d::Document::Compare(_)
            | d::Document::Error(_) => Err(api::Error::InvalidAnswer),
        }
    }
    fn checks_answer(&self, statuses: Box<[api::Status]>) -> Result<api::Answer, api::Error> {
        match self.mode {
            Mode::Checks(_) => Ok(api::Answer::Checks(statuses)),
            Mode::Statuses { ci, more, .. } => Ok(api::Answer::Statuses { ci, statuses, more }),
            Mode::Job { .. } => Err(api::Error::InvalidAnswer),
        }
    }
    fn next_check(&mut self) -> Result<Option<Step>, api::Error> {
        if !self.resolve_jobs {
            let statuses = core::mem::replace(&mut self.statuses, List::with_capacity(0)).into_boxed();
            return Ok(Some(Step::Done(Ok(self.checks_answer(statuses)?))));
        }
        for _ in 0..self.statuses.len().saturating_add(1) {
            if self.position == self.statuses.len() {
                let statuses = core::mem::replace(&mut self.statuses, List::with_capacity(0)).into_boxed();
                return Ok(Some(Step::Done(Ok(self.checks_answer(statuses)?))));
            }
            let status = self.statuses.get(self.position).expect("check position");
            self.position = self.position.saturating_add(1);
            if let Some((run_number, job_index)) = header::job_link(&status.url, &self.row.html_url) {
                self.run_number = run_number;
                self.job_index = job_index;
                self.run = None;
                self.stage = Stage::RunLookup;
                let head = match self.mode {
                    Mode::Checks(head) | Mode::Statuses { head, .. } => head,
                    Mode::Job { .. } => return Err(api::Error::InvalidAnswer),
                };
                return Ok(Some(self.send(r::Request::Runs {
                    at: self.at(),
                    head_sha: self.id(head),
                    run_number,
                    page: r::Page { page: Some(1), limit: Some(1) },
                })));
            }
        }
        Err(api::Error::InvalidAnswer)
    }
    #[must_use]
    pub fn raw_room(&self, answer_bytes: u32) -> u32 {
        let name = match &self.step {
            Some(step) => step.name.len(),
            None => 0,
        };
        answer_bytes.saturating_sub(u32::try_from(name).expect("Bounded step name"))
    }
    fn log(&mut self, log: Box<[u8]>, more: bool) -> Result<Option<Step>, api::Error> {
        if self.stage != Stage::Log {
            return Err(api::Error::InvalidAnswer);
        }
        let attempt = match self.mode {
            Mode::Job { attempt, .. } => attempt,
            Mode::Checks(_) | Mode::Statuses { .. } => return Err(api::Error::InvalidAnswer),
        };
        let truncated = more || self.truncated;
        Ok(Some(Step::Done(Ok(api::Answer::Job { attempt, step: self.step.take(), log, truncated }))))
    }
}
