//! Forgejo Actions identities, bounded by the retained CI statuses.
use crate::api::{Answer, Cue, Error, JobRef, What};
use crate::domain::Domain;
use crate::store::Repository;
use alloc::boxed::Box;
use skein_lib::{Id, List};

/// One CI run per repository and commit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    pub id: u64,
    pub number: u64,
    pub commit: u64,
}
/// One job per configured context, in its context's position.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Job {
    pub id: u64,
    pub run: u64,
    pub index: u32,
    pub attempt: u32,
}
#[derive(Debug)]
pub(crate) struct Kept {
    pub(crate) run: Run,
    pub(crate) pull: Option<u64>,
    pub(crate) jobs: List<Job>,
}

pub(crate) fn start(domain: &mut Domain, id: Id<Repository>, commit: u64, pull: Option<u64>) {
    let repository = domain.repositories.get_mut(id).expect("a CI repository");
    domain.runs = domain.runs.checked_add(1).expect("run ids fit");
    repository.run_number = repository.run_number.checked_add(1).expect("run numbers fit");
    let run = Run { id: domain.runs, number: repository.run_number, commit };
    let capacity = u32::try_from(repository.checks.contexts.len()).expect("bounded contexts");
    let mut jobs = List::with_capacity(capacity);
    for index in 0..capacity {
        domain.jobs = domain.jobs.checked_add(1).expect("job ids fit");
        jobs.push(Job { id: domain.jobs, run: run.id, index, attempt: 1 }).expect("one job per context");
    }
    repository.runs.insert(commit, Kept { run, pull, jobs }).expect("room for each retained CI commit");
}

pub(crate) fn rerun(domain: &mut Domain, id: Id<Repository>, commit: u64, context: u32) {
    let repository = domain.repositories.get_mut(id).expect("a CI repository");
    let run = repository.runs.get_mut(&commit).expect("a CI run");
    let job = run.jobs.get_mut(context).expect("a CI job");
    job.attempt = 2;
}

pub(crate) fn runs(repository: &Repository, commit: u64, number: u64) -> Answer {
    let mut runs = List::with_capacity(1);
    if let Some(kept) = repository.runs.get(&commit)
        && kept.run.number == number
    {
        runs.push(kept.run).expect("one matching run");
    }
    Answer::Runs(runs.into_boxed())
}

pub(crate) fn run(repository: &Repository, id: u64) -> Result<&Kept, Error> {
    for (_, run) in &repository.runs {
        if run.run.id == id {
            return Ok(run);
        }
    }
    Err(Error::Missing(What::Job))
}

pub(crate) fn jobs(repository: &Repository, id: u64) -> Result<Answer, Error> {
    Ok(Answer::Jobs(copy_of_jobs(&run(repository, id)?.jobs)))
}
fn copy_of_jobs(jobs: &List<Job>) -> Box<[Job]> {
    let mut answer = List::with_capacity(jobs.len());
    for job in jobs {
        answer.push(*job).expect("one entry per job");
    }
    answer.into_boxed()
}

pub(crate) fn latest(repository: &Repository, id: u64) -> Result<(u64, Job), Error> {
    for (&commit, kept) in &repository.runs {
        for job in &kept.jobs {
            if job.id == id {
                return Ok((commit, *job));
            }
        }
    }
    Err(Error::Missing(What::Job))
}

pub(crate) fn reference(repository: &Repository, commit: u64, context: &[u8]) -> Option<JobRef> {
    let run = repository.runs.get(&commit)?;
    for job in &run.jobs {
        let index = usize::try_from(job.index).ok()?;
        if *repository.checks.contexts.get(index)?.as_ref() == *context {
            return Some(JobRef { run: run.run.id, job: job.id, attempt: job.attempt });
        }
    }
    None
}

pub(crate) fn link(repository: &Repository, commit: u64, context: &[u8]) -> Box<[u8]> {
    let Some(reference) = reference(repository, commit, context) else { return Box::new([]) };
    let kept = repository.runs.get(&commit).expect("the reference's run");
    let mut link = List::with_capacity(
        u32::try_from(repository.name.len()).expect("bounded name").checked_add(51).expect("bounded link"),
    );
    append(&mut link, b"/");
    append(&mut link, &repository.name);
    append(&mut link, b"/actions/runs/");
    decimal(&mut link, kept.run.number);
    append(&mut link, b"/jobs/");
    for job in &kept.jobs {
        if job.id == reference.job {
            decimal(&mut link, u64::from(job.index));
        }
    }
    link.into_boxed()
}
fn append(out: &mut List<u8>, bytes: &[u8]) {
    for &byte in bytes {
        out.push(byte).expect("link capacity");
    }
}
fn decimal(out: &mut List<u8>, number: u64) {
    append(out, skein_lib::Decimal::of(number).as_bytes());
}

/// Where a context's failed job fails, set by the scenario.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failing {
    /// The workflow step and completion fail.
    Workflow,
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

pub(crate) fn steps(context: &[u8], failing: Failing, cue: Option<&Cue>, pull: Option<u64>) -> Box<[Step]> {
    let status = match failing {
        Failing::Workflow => StepStatus::Failure,
        Failing::NoStep => StepStatus::Cancelled,
    };
    Box::new([
        Step {
            number: 0,
            name: skein_lib::bytes::copy_of(b"Set up job"),
            status: StepStatus::Success,
            log: log(context, failing, cue, pull, Some(0)).expect("set-up log"),
        },
        Step {
            number: WORKFLOW_STEP,
            name: match cue {
                Some(Cue::PullFile { step, .. }) => skein_lib::bytes::copy_of(step),
                Some(Cue::File { .. } | Cue::Message { .. }) | None => joined(context, &[b"Run ", context]),
            },
            status,
            log: log(context, failing, cue, pull, Some(1)).expect("workflow log"),
        },
        Step {
            number: 2,
            name: skein_lib::bytes::copy_of(b"Complete job"),
            status,
            log: log(context, failing, cue, pull, Some(2)).expect("completion log"),
        },
    ])
}

pub(crate) fn log(
    context: &[u8],
    failing: Failing,
    cue: Option<&Cue>,
    pull: Option<u64>,
    step: Option<u32>,
) -> Result<Box<[u8]>, Error> {
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
    let custom = match cue {
        Some(Cue::PullFile { log, .. }) => Some(crate::ci::template(log, pull)),
        Some(Cue::File { .. } | Cue::Message { .. }) | None => None,
    };
    let extra = match &custom {
        Some(line) => u32::try_from(line.len()).expect("bounded log").checked_add(1).expect("bounded log"),
        None => 0,
    };
    let mut out = List::with_capacity(capacity.checked_add(extra).expect("bounded log"));
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
    if (step.is_none() || step == Some(WORKFLOW_STEP))
        && let Some(line) = &custom
    {
        append(&mut out, line);
        append(&mut out, b"\n");
    }
    if step.is_none() || step == Some(2) {
        append(&mut out, b"job ");
        append(&mut out, context);
        append(
            &mut out,
            match failing {
                Failing::Workflow => b" failed\n",
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
