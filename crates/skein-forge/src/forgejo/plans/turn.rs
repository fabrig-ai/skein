//! Actions runs and nonrecursive trees (forge.md, 5.2).
use super::{
    Body, Heard,
    plan::{Bounds, Step},
    reading,
    tables::{Repository, Tables},
};
use crate::api;
use crate::forgejo::{document as d, id::ObjectId, request as r};
use alloc::boxed::Box;
use skein_lib::{List, Time};

#[derive(Debug)]
pub(super) enum Mode {
    Runs { workflow: Box<[u8]>, commit: api::Commit, page: u32 },
    Run(u64),
    Tree { object: api::Commit, page: u32 },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Initial,
    Jobs,
    Steps,
}
#[derive(Debug)]
pub(super) struct Plan {
    mode: Mode,
    row: Repository,
    stage: Stage,
    runs: List<api::Run>,
    entries: List<api::Entry>,
    jobs: List<api::RunJob>,
    run: Option<api::Run>,
    position: u32,
    size: u64,
    count: u32,
}
impl Plan {
    pub(super) fn new(mode: Mode, row: Repository, bounds: Bounds) -> Self {
        let (runs, entries, jobs) = match mode {
            Mode::Runs { .. } => (capacity(bounds, crate::measure::RUN_BYTES).min(bounds.page_size), 0, 0),
            Mode::Tree { .. } => (0, capacity(bounds, crate::measure::ENTRY_BYTES).min(bounds.page_size), 0),
            Mode::Run(_) => (0, 0, capacity(bounds, crate::measure::RUN_JOB_BYTES)),
        };
        Self {
            mode,
            row,
            stage: Stage::Initial,
            runs: List::with_capacity(runs),
            entries: List::with_capacity(entries),
            jobs: List::with_capacity(jobs),
            run: None,
            position: 0,
            size: 0,
            count: 0,
        }
    }
    fn at(&self) -> r::Repo {
        r::Repo { owner: self.row.owner.clone(), name: self.row.name.clone() }
    }
    fn id(&self, bytes: api::Commit) -> ObjectId {
        ObjectId { bytes, format: self.row.object_format }
    }
    pub(super) fn begin(&self, bounds: Bounds) -> Step {
        let at = self.at();
        Step::send(match &self.mode {
            Mode::Runs { workflow, commit, page } => r::Request::WorkflowRuns {
                at,
                workflow: workflow.clone(),
                head_sha: self.id(*commit),
                page: r::Page { page: Some(*page), limit: Some(bounds.page_size) },
            },
            Mode::Run(run) => r::Request::Run { at, run: *run },
            Mode::Tree { object, page } => r::Request::Tree {
                at,
                object: self.id(*object),
                page: r::Page { page: Some(*page), limit: Some(bounds.page_size) },
            },
        })
    }
    fn add(&mut self, size: u64, bounds: Bounds) -> Result<(), api::Error> {
        self.size = self.size.checked_add(size).ok_or(api::Error::TooLarge)?;
        if self.size > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        Ok(())
    }
    pub(super) fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        repository: api::Repository,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, fields } => {
                if status != 200 {
                    let failure =
                        super::failure::classify(super::failure::Asked::Leaf, status, None, fields.retry_after);
                    match failure.drop {
                        super::failure::Drop::Nothing => {}
                        super::failure::Drop::Path | super::failure::Drop::Row => tables.drop_repository(repository),
                    }
                    return Err(failure.error);
                }
                Ok(None)
            }
            Heard::Failed(fault) => Err(super::failure::transport(fault)),
            Heard::Row(row) => {
                self.row(row, bounds)?;
                Ok(None)
            }
            Heard::Done(body) => match body {
                Body::Document(document) => self.done(document, bounds),
                Body::Malformed(
                    crate::forgejo::decode::Malformed::Bound { .. } | crate::forgejo::decode::Malformed::TooLong { .. },
                ) => Err(api::Error::TooLarge),
                Body::Empty | Body::Raw { .. } | Body::Malformed(_) => Err(api::Error::InvalidAnswer),
            },
        }
    }
    fn row(&mut self, row: d::Row, bounds: Bounds) -> Result<(), api::Error> {
        match row {
            d::Row::Run(run) => {
                if self.stage != Stage::Initial {
                    return Err(api::Error::InvalidAnswer);
                }
                let Mode::Runs { workflow, commit, .. } = &self.mode else {
                    return Err(api::Error::InvalidAnswer);
                };
                let value = run_value(run, self.row.object_format)?;
                // Forgejo filters head_sha but ignores workflow (forge.md, 9).
                if value.workflow.as_ref() != workflow.as_ref() || value.commit != *commit {
                    self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
                    return Ok(());
                }
                let size = crate::measure::run(&value)
                    .ok_or(api::Error::TooLarge)?
                    .checked_add(u64::try_from(crate::measure::RUN_BYTES).expect("run width"))
                    .ok_or(api::Error::TooLarge)?;
                self.add(size, bounds)?;
                self.runs.push(value).or(Err(api::Error::TooLarge))?;
            }
            d::Row::Entry(entry) => {
                if self.stage != Stage::Initial {
                    return Err(api::Error::InvalidAnswer);
                }
                let Mode::Tree { .. } = self.mode else {
                    return Err(api::Error::InvalidAnswer);
                };
                let value = api::Entry {
                    name: entry.path,
                    mode: entry.mode,
                    kind: entry.kind,
                    object: reading::commit(entry.sha, self.row.object_format).ok_or(api::Error::InvalidAnswer)?,
                };
                self.add(
                    u64::try_from(crate::measure::ENTRY_BYTES.saturating_add(value.name.len())).expect("entry size"),
                    bounds,
                )?;
                self.entries.push(value).or(Err(api::Error::TooLarge))?;
            }
            d::Row::Job(job) => {
                if self.stage != Stage::Jobs {
                    return Err(api::Error::InvalidAnswer);
                }
                if job.run_id != self.run.as_ref().expect("run before jobs").id {
                    return Err(api::Error::InvalidAnswer);
                }
                let (state, conclusion) = state(job.status);
                self.add(
                    u64::try_from(crate::measure::RUN_JOB_BYTES.saturating_add(job.name.len())).expect("job size"),
                    bounds,
                )?;
                self.jobs
                    .push(api::RunJob {
                        steps: 0,
                        id: job.id,
                        name: job.name,
                        attempt: job.attempt,
                        state,
                        conclusion,
                        step: None,
                    })
                    .or(Err(api::Error::TooLarge))?;
            }
            d::Row::Step(step) => {
                if self.stage != Stage::Steps {
                    return Err(api::Error::InvalidAnswer);
                }
                let job = self.jobs.get_mut(self.position).expect("failed job position");
                job.steps = job.steps.checked_add(1).ok_or(api::Error::TooLarge)?;
                if step.status == d::StepStatus::Failure && job.step.is_none() {
                    let size = u64::try_from(step.name.len()).expect("step size");
                    job.step = Some(api::Step { number: step.number, name: step.name });
                    self.add(size, bounds)?;
                }
            }
            d::Row::Found(_)
            | d::Row::Reachable(_)
            | d::Row::Token(_)
            | d::Row::User(_)
            | d::Row::Issue(_)
            | d::Row::Comment(_)
            | d::Row::Status(_)
            | d::Row::Review(_)
            | d::Row::ReviewComment(_)
            | d::Row::Branch(_)
            | d::Row::Protection(_)
            | d::Row::Pull(_)
            | d::Row::File(_)
            | d::Row::Commit(_)
            | d::Row::Artifact(_)
            | d::Row::CommitMessage(_)
            | d::Row::Hook(_)
            | d::Row::Team(_) => return Err(api::Error::InvalidAnswer),
        }
        self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
        Ok(())
    }
    fn done(&mut self, document: d::Document, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        match document {
            d::Document::Runs { .. } => {
                let Mode::Runs { .. } = self.mode else {
                    return Err(api::Error::InvalidAnswer);
                };
                if self.stage != Stage::Initial {
                    return Err(api::Error::InvalidAnswer);
                }
                let runs = core::mem::replace(&mut self.runs, List::with_capacity(0)).into_boxed();
                Ok(Some(Step::Done(Ok(api::Answer::Runs { runs, more: self.count == bounds.page_size }))))
            }
            d::Document::Tree { truncated } => {
                let Mode::Tree { .. } = self.mode else {
                    return Err(api::Error::InvalidAnswer);
                };
                let entries = core::mem::replace(&mut self.entries, List::with_capacity(0)).into_boxed();
                Ok(Some(Step::Done(Ok(api::Answer::Tree {
                    entries,
                    more: truncated || self.count == bounds.page_size,
                }))))
            }
            d::Document::Run(run) => {
                let Mode::Run(expected) = self.mode else {
                    return Err(api::Error::InvalidAnswer);
                };
                if self.stage != Stage::Initial || run.id != expected {
                    return Err(api::Error::InvalidAnswer);
                }
                let run = run_value(run, self.row.object_format)?;
                self.add(crate::measure::run(&run).ok_or(api::Error::TooLarge)?, bounds)?;
                self.run = Some(run);
                self.stage = Stage::Jobs;
                Ok(Some(Step::send(r::Request::Jobs { at: self.at(), run: expected })))
            }
            d::Document::Listed { .. } => {
                if self.stage != Stage::Jobs {
                    return Err(api::Error::InvalidAnswer);
                }
                self.next_job()
            }
            d::Document::Job(job) => {
                if self.stage != Stage::Steps {
                    return Err(api::Error::InvalidAnswer);
                }
                let expected = self.jobs.get(self.position).expect("failed job position");
                if job.id != expected.id
                    || job.attempt != expected.attempt
                    || job.run_id != self.run.as_ref().expect("read run").id
                {
                    return Err(api::Error::InvalidAnswer);
                }
                self.position = self.position.saturating_add(1);
                self.next_job()
            }
            d::Document::Hook(_)
            | d::Document::Token(_)
            | d::Document::Org(_)
            | d::Document::Team(_)
            | d::Document::User(_)
            | d::Document::ApiSettings(_)
            | d::Document::Repository(_)
            | d::Document::CreatedRepository(_)
            | d::Document::CreatedFile { .. }
            | d::Document::Search { .. }
            | d::Document::RepoSearch { .. }
            | d::Document::Issue(_)
            | d::Document::Comment(_)
            | d::Document::Status(_)
            | d::Document::Review(_)
            | d::Document::Pull(_)
            | d::Document::CombinedStatus(_)
            | d::Document::Branch(_)
            | d::Document::Protection(_)
            | d::Document::Permission(_)
            | d::Document::Compare(_)
            | d::Document::Error(_) => Err(api::Error::InvalidAnswer),
        }
    }
    fn next_job(&mut self) -> Result<Option<Step>, api::Error> {
        for _ in self.position..self.jobs.len() {
            let job = self.jobs.get(self.position).expect("listed job");
            if job.conclusion == Some(api::Conclusion::Failure) {
                self.stage = Stage::Steps;
                return Ok(Some(Step::send(r::Request::Job { at: self.at(), job: job.id })));
            }
            self.position = self.position.saturating_add(1);
        }
        let mut run = self.run.take().ok_or(api::Error::InvalidAnswer)?;
        run.jobs = core::mem::replace(&mut self.jobs, List::with_capacity(0)).into_boxed();
        Ok(Some(Step::Done(Ok(api::Answer::Run(run)))))
    }
}
fn state(status: d::StepStatus) -> (api::RunState, Option<api::Conclusion>) {
    match status {
        d::StepStatus::Unknown | d::StepStatus::Waiting | d::StepStatus::Blocked => (api::RunState::Waiting, None),
        d::StepStatus::Running => (api::RunState::Running, None),
        d::StepStatus::Success => (api::RunState::Completed, Some(api::Conclusion::Success)),
        d::StepStatus::Failure => (api::RunState::Completed, Some(api::Conclusion::Failure)),
        d::StepStatus::Cancelled => (api::RunState::Completed, Some(api::Conclusion::Cancelled)),
        d::StepStatus::Skipped => (api::RunState::Completed, Some(api::Conclusion::Skipped)),
    }
}
fn run_value(value: d::Run, format: crate::forgejo::id::ObjectFormat) -> Result<api::Run, api::Error> {
    if value.workflow.is_empty() || value.trigger_actor == 0 || value.html_url.is_empty() {
        return Err(api::Error::InvalidAnswer);
    }
    let (state, conclusion) = state(value.status);
    Ok(api::Run {
        id: value.id,
        number: value.index_in_repo,
        workflow: value.workflow,
        commit: reading::commit(value.commit_sha, format).ok_or(api::Error::InvalidAnswer)?,
        event: if value.event.as_ref() == b"workflow_dispatch" { api::Event::Dispatch } else { api::Event::Other },
        actor: value.trigger_actor,
        state,
        conclusion,
        url: value.html_url,
        created: Time::from_nanos(value.created_at.as_nanos()),
        jobs: Box::new([]),
    })
}

fn capacity(bounds: Bounds, width: usize) -> u32 {
    bounds
        .rows
        .min(bounds.answer_bytes.checked_div(u32::try_from(width).expect("list width fits")).expect("nonzero width"))
}
