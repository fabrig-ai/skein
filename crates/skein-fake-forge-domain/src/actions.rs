//! Actions runs and jobs retained separately for commit CI and dispatches (fake-forge.md, 2.3).
use crate::api::{Answer, Check, Error, JobRef, What};
use crate::domain::{self, Config, Domain};
use crate::store::Repository;
use alloc::boxed::Box;
use skein_lib::bytes::copy_of;
use skein_lib::{Env, Id, List, Time};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RunEvent {
    Push,
    Dispatch,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RunState {
    Running,
    Completed,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Conclusion {
    Success,
    Failure,
}

/// A run's identity and current outcome (forge.md, 5.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    pub id: u64,
    pub number: u64,
    pub commit: u64,
    pub workflow: Option<Box<[u8]>>,
    pub event: RunEvent,
    pub user: u64,
    pub state: RunState,
    pub conclusion: Option<Conclusion>,
    pub created: Time,
    pub inputs: Box<[crate::api::WorkflowInput]>,
}
/// A job at its latest attempt, with its own outcome and steps.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Job {
    pub id: u64,
    pub run: u64,
    pub index: u32,
    pub attempt: u32,
    pub name: Box<[u8]>,
    pub state: RunState,
    pub conclusion: Option<Conclusion>,
    pub steps: Box<[Step]>,
}
#[derive(Debug)]
pub(crate) struct Kept {
    pub(crate) run: Run,
    pub(crate) jobs: List<Job>,
}

pub(crate) fn start(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, commit: u64, user: u64) {
    let repository = domain.repositories.get_mut(id).expect("a CI repository");
    domain.runs = domain.runs.checked_add(1).expect("run ids fit");
    repository.run_number = repository.run_number.checked_add(1).expect("run numbers fit");
    let run = Run {
        id: domain.runs,
        number: repository.run_number,
        commit,
        workflow: None,
        event: RunEvent::Push,
        user,
        state: RunState::Running,
        conclusion: None,
        created: domain::clock(env),
        inputs: Box::new([]),
    };
    let capacity = u32::try_from(repository.checks.contexts.len()).expect("bounded contexts");
    let mut jobs = List::with_capacity(capacity);
    for (index, name) in repository.checks.contexts.iter().enumerate() {
        domain.jobs = domain.jobs.checked_add(1).expect("job ids fit");
        jobs.push(Job {
            id: domain.jobs,
            run: run.id,
            index: u32::try_from(index).expect("bounded job index"),
            attempt: 1,
            name: copy_of(name),
            state: RunState::Running,
            conclusion: None,
            steps: Box::new([]),
        })
        .expect("one job per context");
    }
    repository.runs.insert(commit, Kept { run, jobs }).expect("room for each retained CI commit");
}

pub(crate) fn rerun(domain: &mut Domain, id: Id<Repository>, commit: u64, context: u32) {
    let repository = domain.repositories.get_mut(id).expect("a CI repository");
    let kept = repository.runs.get_mut(&commit).expect("a CI run");
    let job = kept.jobs.get_mut(context).expect("a CI job");
    job.attempt = job.attempt.checked_add(1).expect("attempts fit");
    job.state = RunState::Running;
    job.conclusion = None;
    job.steps = Box::new([]);
    complete(kept);
}

pub(crate) fn verdict(repository: &mut Repository, commit: u64, context: u32, state: Check) {
    let kept = repository.runs.get_mut(&commit).expect("CI run");
    let job = kept.jobs.get_mut(context).expect("CI job");
    job.state = RunState::Completed;
    job.conclusion = Some(match state {
        Check::Passed => Conclusion::Success,
        Check::Failed => Conclusion::Failure,
        Check::Pending => unreachable!("a verdict ends a job"),
    });
    // Commit CI steps are derived from its context and outcome when read.
    job.steps = Box::new([]);
    complete(kept);
}

pub(crate) fn complete(kept: &mut Kept) {
    let mut conclusion = Conclusion::Success;
    for job in &kept.jobs {
        if job.state == RunState::Running {
            kept.run.state = RunState::Running;
            kept.run.conclusion = None;
            return;
        }
        if job.conclusion != Some(Conclusion::Success) {
            conclusion = Conclusion::Failure;
        }
    }
    kept.run.state = RunState::Completed;
    kept.run.conclusion = Some(conclusion);
}

pub(crate) fn runs(
    repository: &Repository,
    workflow: Option<&[u8]>,
    commit: Option<u64>,
    number: Option<u64>,
    page: u32,
    size: u32,
) -> Answer {
    let mut ordered = skein_lib::Map::with_capacity(repository.runs.len().saturating_add(repository.dispatched.len()));
    for source in [&repository.runs, &repository.dispatched] {
        for (_, kept) in source {
            let run = &kept.run;
            if let Some(workflow) = workflow
                && Some(workflow) != run.workflow.as_deref()
            {
                continue;
            }
            if let Some(commit) = commit
                && commit != run.commit
            {
                continue;
            }
            if let Some(number) = number
                && number != run.number
            {
                continue;
            }
            ordered.insert(u64::MAX.saturating_sub(run.number), run.id).expect("one key per run");
        }
    }
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(size));
    let mut answer = List::with_capacity(size);
    for (index, (_, id)) in ordered.iter().enumerate() {
        if u64::try_from(index).expect("bounded runs") < skip {
            continue;
        }
        if answer.room() == 0 {
            break;
        }
        answer.push(run(repository, *id).expect("indexed run").run.clone()).expect("page room");
    }
    Answer::Runs(answer.into_boxed())
}

pub(crate) fn run(repository: &Repository, id: u64) -> Result<&Kept, Error> {
    for source in [&repository.runs, &repository.dispatched] {
        for (_, kept) in source {
            if kept.run.id == id {
                return Ok(kept);
            }
        }
    }
    Err(Error::Missing(What::Job))
}

pub(crate) fn jobs(repository: &Repository, id: u64) -> Result<Answer, Error> {
    let kept = run(repository, id)?;
    let mut answer = List::with_capacity(kept.jobs.len());
    for job in &kept.jobs {
        let mut view = job.clone();
        if kept.run.workflow.is_none() && job.state == RunState::Completed {
            let state = match job.conclusion {
                Some(Conclusion::Success) => Check::Passed,
                Some(Conclusion::Failure) => Check::Failed,
                None => unreachable!("a completed CI job has a conclusion"),
            };
            let failing =
                *repository.failing.get(usize::try_from(job.index).expect("bounded context")).expect("CI context");
            view.steps = outcome_steps(&job.name, state, failing);
        }
        answer.push(view).expect("one entry per job");
    }
    Ok(Answer::Jobs(answer.into_boxed()))
}

pub(crate) fn latest(repository: &Repository, id: u64) -> Result<(u64, Job), Error> {
    for source in [&repository.runs, &repository.dispatched] {
        for (_, kept) in source {
            for job in &kept.jobs {
                if job.id == id {
                    return Ok((kept.run.commit, job.clone()));
                }
            }
        }
    }
    Err(Error::Missing(What::Job))
}

pub(crate) fn reference(repository: &Repository, commit: u64, context: &[u8]) -> Option<JobRef> {
    let reference = repository.statuses.get(&commit)?.get(context)?.job?;
    let kept = run(repository, reference.run).ok()?;
    for job in &kept.jobs {
        if job.id == reference.job && job.attempt == reference.attempt {
            return Some(reference);
        }
    }
    None
}

pub(crate) fn ci_reference(repository: &Repository, commit: u64, context: &[u8]) -> Option<JobRef> {
    let kept = repository.runs.get(&commit)?;
    for job in &kept.jobs {
        if job.name.as_ref() == context {
            return Some(JobRef { run: kept.run.id, job: job.id, attempt: job.attempt });
        }
    }
    None
}

pub(crate) fn context(run: &Run, job: &Job) -> Box<[u8]> {
    let Some(file) = &run.workflow else { return copy_of(&job.name) };
    let capacity =
        file.len().checked_add(3).expect("bounded context").checked_add(job.name.len()).expect("bounded context");
    let mut out = List::with_capacity(u32::try_from(capacity).expect("bounded context"));
    append(&mut out, file);
    append(&mut out, b" / ");
    append(&mut out, &job.name);
    out.into_boxed()
}

pub(crate) fn link(repository: &Repository, commit: u64, context: &[u8]) -> Box<[u8]> {
    let Some(reference) = ci_reference(repository, commit, context) else { return Box::new([]) };
    let kept = run(repository, reference.run).expect("the reference's run");
    job_link(repository, &kept.run, reference.job)
}
pub(crate) fn job_link(repository: &Repository, run: &Run, id: u64) -> Box<[u8]> {
    let mut link = List::with_capacity(
        u32::try_from(repository.name.len()).expect("bounded name").checked_add(51).expect("bounded link"),
    );
    append(&mut link, b"/");
    append(&mut link, &repository.name);
    append(&mut link, b"/actions/runs/");
    decimal(&mut link, run.number);
    append(&mut link, b"/jobs/");
    let kept = self::run(repository, run.id).expect("stored run");
    for job in &kept.jobs {
        if job.id == id {
            decimal(&mut link, u64::from(job.index));
        }
    }
    link.into_boxed()
}
fn append(out: &mut List<u8>, bytes: &[u8]) {
    for &byte in bytes {
        out.push(byte).expect("bounded bytes");
    }
}
fn decimal(out: &mut List<u8>, number: u64) {
    append(out, skein_lib::Decimal::of(number).as_bytes());
}

pub(crate) fn outcome_steps(context: &[u8], state: Check, failing: Failing) -> Box<[Step]> {
    let mut steps = steps(context, failing);
    if state == Check::Passed {
        for step in &mut steps {
            step.status = StepStatus::Success;
            step.log = Box::new([]);
        }
    }
    steps
}

/// Where a context's failed job fails, set by the scenario.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failing {
    /// The workflow step and completion fail.
    Workflow,
    /// The runner fails before the workflow step.
    Setup,
    /// The workflow and completion are cancelled; no step fails.
    NoStep,
}

/// One step's Forgejo fields and its separately served log slice.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Step {
    pub number: u32,
    pub name: Box<[u8]>,
    pub status: StepStatus,
    pub log: Box<[u8]>,
}

/// A step's outcome, in the fake's vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StepStatus {
    Success,
    Failure,
    Cancelled,
}

/// The number of the job's one workflow step.
pub const WORKFLOW_STEP: u32 = 1;

pub(crate) fn steps(context: &[u8], failing: Failing) -> Box<[Step]> {
    let status = match failing {
        Failing::Workflow => StepStatus::Failure,
        Failing::Setup | Failing::NoStep => StepStatus::Cancelled,
    };
    Box::new([
        Step {
            number: 0,
            name: copy_of(b"Set up job"),
            status: if failing == Failing::Setup { StepStatus::Failure } else { StepStatus::Success },
            log: log(context, failing, Some(0)).expect("set-up log"),
        },
        Step {
            number: WORKFLOW_STEP,
            name: joined(context, &[b"Run ", context]),
            status,
            log: log(context, failing, Some(1)).expect("workflow log"),
        },
        Step {
            number: 2,
            name: copy_of(b"Complete job"),
            status,
            log: log(context, failing, Some(2)).expect("completion log"),
        },
    ])
}

pub(crate) fn log(context: &[u8], failing: Failing, step: Option<u32>) -> Result<Box<[u8]>, Error> {
    if let Some(number) = step
        && number > 2
    {
        return Err(Error::Missing(What::Job));
    }
    let capacity = u32::try_from(context.len())
        .expect("bounded context")
        .checked_mul(6)
        .expect("bounded log")
        .checked_add(91)
        .expect("bounded log");
    let mut out = List::with_capacity(capacity);
    if step.is_none() || step == Some(0) {
        append(&mut out, b"set up ");
        append(&mut out, context);
        append(&mut out, b"\n");
    }
    if step.is_none() || step == Some(WORKFLOW_STEP) {
        for line in 0_u8..4 {
            append(&mut out, b"failing check ");
            append(&mut out, context);
            append(&mut out, b" ");
            out.push(b'0'.checked_add(line).expect("four lines")).expect("log capacity");
            append(&mut out, b"\n");
        }
    }
    if step.is_none() || step == Some(2) {
        append(&mut out, b"job ");
        append(&mut out, context);
        append(
            &mut out,
            match failing {
                Failing::Workflow | Failing::Setup => b" failed\n",
                Failing::NoStep => b" cancelled\n",
            },
        );
    }
    Ok(out.into_boxed())
}

fn joined(context: &[u8], parts: &[&[u8]]) -> Box<[u8]> {
    let capacity = u32::try_from(context.len()).expect("bounded context").checked_add(4).expect("bounded name");
    let mut out = List::with_capacity(capacity);
    for part in parts {
        append(&mut out, part);
    }
    out.into_boxed()
}
