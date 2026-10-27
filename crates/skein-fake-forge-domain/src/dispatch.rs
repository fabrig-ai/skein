//! Dispatch-only workflows, runner faults and explicit reruns (fake-forge.md, 2.3).
use crate::{
    Config, Domain, Limits,
    actions::{self, Conclusion, Job, Kept, Run, RunEvent, RunState},
    api::{Answer, Check, Cue, Error, Failing, Permission, What},
    ci,
    domain::{self, Alarm},
    faults,
    store::Repository,
};
use alloc::boxed::Box;
use skein_lib::{
    Env, Id, List,
    bytes::{copy_of, find},
};

pub(crate) fn fits(
    limits: &Limits,
    workflow: &[u8],
    branch: &[u8],
    inputs: &[crate::api::WorkflowInput],
) -> Result<(), Error> {
    crate::store::fits(workflow, limits.name_bytes)?;
    crate::store::fits(branch, limits.name_bytes)?;
    if inputs.len() > usize::try_from(limits.contexts).expect("context bound fits") {
        return Err(Error::TooLarge);
    }
    for (key, value) in inputs {
        crate::store::fits(key, limits.name_bytes)?;
        crate::store::fits(value, limits.body_bytes)?;
    }
    Ok(())
}

pub(crate) fn dispatch(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    file: Box<[u8]>,
    branch: Box<[u8]>,
    inputs: Box<[crate::api::WorkflowInput]>,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get(id).expect("dispatch repository");
    repository.require(user, Permission::Write)?;
    fits(&env.limits.limits, &file, &branch, &inputs)?;
    let workflow = workflow(repository, &file)?.clone();
    let commit = *repository.branches.get(&*branch).ok_or(Error::Missing(What::Branch))?;
    if repository.dispatched.capacity() == 0 {
        return Err(Error::Full);
    }
    let probe = Run {
        id: 0,
        number: 0,
        commit,
        workflow: Some(file.clone()),
        event: RunEvent::Dispatch,
        user,
        state: RunState::Running,
        conclusion: None,
        created: domain::clock(env),
        inputs: inputs.clone(),
    };
    crate::reports::failed(domain, &workflow, &probe, &env.limits.limits)?;
    let artifact = crate::reports::artifact(domain, &workflow, &probe, &env.limits.limits)?;
    let repository = domain.repositories.get_mut(id).expect("dispatch repository");
    if repository.dispatched.len() == repository.dispatched.capacity() {
        let (&oldest, _) = repository.dispatched.iter().next().expect("a full nonempty run store");
        let removed = repository.dispatched.remove(&oldest).expect("oldest retained run");
        for job in &removed.jobs {
            domain.timers.cancel(Alarm::Workflow { repository: id, run: oldest, job: job.id });
        }
    }
    domain.runs = domain.runs.checked_add(1).expect("run ids fit");
    repository.run_number = repository.run_number.checked_add(1).expect("run numbers fit");
    let run = Run {
        id: domain.runs,
        number: repository.run_number,
        commit,
        workflow: Some(file),
        event: RunEvent::Dispatch,
        user,
        state: RunState::Running,
        conclusion: None,
        created: domain::clock(env),
        inputs,
    };
    let mut jobs = List::with_capacity(u32::try_from(workflow.jobs.len()).expect("bounded jobs"));
    for (index, name) in workflow.jobs.iter().enumerate() {
        domain.jobs = domain.jobs.checked_add(1).expect("job ids fit");
        jobs.push(Job {
            id: domain.jobs,
            run: run.id,
            index: u32::try_from(index).expect("bounded jobs"),
            attempt: 1,
            name: copy_of(name),
            state: RunState::Running,
            conclusion: None,
            steps: Box::new([]),
        })
        .expect("one job per definition");
    }
    let mut kept = Kept { run, jobs, artifact: None };
    actions::complete(&mut kept);
    if kept.run.state == RunState::Completed {
        kept.artifact = artifact;
    }
    let run = kept.run.id;
    repository.dispatched.insert(run, kept).expect("run room admitted");
    for index in 0..u32::try_from(workflow.jobs.len()).expect("bounded jobs") {
        let job = domain
            .repositories
            .get(id)
            .expect("repository")
            .dispatched
            .get(&run)
            .expect("run")
            .jobs
            .get(index)
            .expect("job")
            .id;
        pending(domain, env, id, run, job);
    }
    Ok(Answer::Done)
}

fn workflow<'a>(repository: &'a Repository, file: &[u8]) -> Result<&'a crate::api::Workflow, Error> {
    for workflow in &repository.workflows {
        if workflow.file.as_ref() == file {
            return Ok(workflow);
        }
    }
    Err(Error::Missing(What::File))
}

fn pending(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, run: u64, job: u64) {
    let repository = domain.repositories.get(id).expect("repository");
    let kept = repository.dispatched.get(&run).expect("dispatched run");
    let file = kept.run.workflow.as_ref().expect("workflow run");
    let workflow = workflow(repository, file).expect("configured workflow for retained run");
    let (silent, min, max) = (workflow.silent, workflow.latency_min, workflow.latency_max);
    if domain.rng.chance(silent) {
        domain.tally.silent = domain.tally.silent.saturating_add(1);
    } else {
        let span = faults::draw(domain, min, max);
        domain
            .timers
            .arm(Alarm::Workflow { repository: id, run, job }, env.now.saturating_add(span))
            .expect("one timer per retained dispatched job");
    }
}

pub(crate) fn report(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, run: u64, job: u64) {
    let repository = domain.repositories.get(id).expect("repository");
    let kept = repository.dispatched.get(&run).expect("timer's retained run");
    let commit = kept.run.commit;
    let lost = domain.rng.chance(env.limits.lost);
    let file = kept.run.workflow.as_ref().expect("dispatched workflow");
    let workflow = workflow(repository, file).expect("configured workflow");
    let failed_ids =
        crate::reports::failed(domain, workflow, &kept.run, &env.limits.limits).expect("report inputs admitted");
    let state = if lost || !failed_ids.is_empty() {
        Check::Failed
    } else {
        match &workflow.cue {
            Some(Cue::Message { fail }) => {
                if find(&domain.object(commit).expect("run commit").message, fail).is_some() {
                    Check::Failed
                } else {
                    Check::Passed
                }
            }
            Some(Cue::Tests { .. }) => Check::Passed,
            Some(Cue::File { path, green }) => match domain.object(commit).expect("run commit").tree.get(&**path) {
                Some(content) if find(content, green).is_some() => Check::Passed,
                Some(_) | None => Check::Failed,
            },
            None => {
                if domain.rng.chance(workflow.passes) {
                    Check::Passed
                } else {
                    Check::Failed
                }
            }
        }
    };
    let setup = domain.rng.chance(env.limits.setup_faults);
    let state = if setup || lost { Check::Failed } else { state };
    let failing = if lost {
        Failing::NoStep
    } else if setup {
        Failing::Setup
    } else {
        Failing::Workflow
    };
    let artifact =
        crate::reports::artifact(domain, workflow, &kept.run, &env.limits.limits).expect("report inputs admitted");
    let repository = domain.repositories.get_mut(id).expect("repository");
    let kept = repository.dispatched.get_mut(&run).expect("run");
    let mut attempt = None;
    for index in 0..kept.jobs.len() {
        let entry = kept.jobs.get_mut(index).expect("retained job");
        if entry.id == job {
            entry.state = RunState::Completed;
            entry.conclusion = Some(if state == Check::Passed { Conclusion::Success } else { Conclusion::Failure });
            entry.steps = actions::outcome_steps(&entry.name, state, failing);
            attempt = Some(entry.attempt);
            break;
        }
    }
    actions::complete(kept);
    if crate::reports::reportable(kept) {
        kept.artifact = artifact;
    }
    let attempt = attempt.expect("timer's retained job");
    crate::hooks::notify(
        domain,
        env,
        id,
        crate::hooks::Hook::job(
            crate::api::Ended { job: crate::api::JobRef { run, job, attempt }, commit, state },
            env.limits.ci,
        ),
    );
    domain.tally.verdicts = domain.tally.verdicts.saturating_add(1);
}

pub(crate) fn rerun(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    user: u64,
    job: u64,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get(id).expect("repository");
    repository.require(user, Permission::Write)?;
    let (commit, entry) = actions::latest(repository, job)?;
    let kept = actions::run(repository, entry.run)?;
    if kept.run.state != RunState::Completed {
        return Err(Error::Refused);
    }
    if kept.run.workflow.is_none() {
        domain.timers.cancel(Alarm::Rerun { repository: id, commit, context: entry.index });
        ci::rerun(domain, env, id, commit, entry.index);
        return Ok(Answer::Done);
    }
    let repository = domain.repositories.get_mut(id).expect("repository");
    let kept = repository.dispatched.get_mut(&entry.run).expect("dispatched run");
    kept.artifact = None;
    for index in 0..kept.jobs.len() {
        let entry = kept.jobs.get_mut(index).expect("retained job");
        if entry.id == job {
            entry.attempt = entry.attempt.checked_add(1).expect("attempts fit");
            entry.state = RunState::Running;
            entry.conclusion = None;
            entry.steps = Box::new([]);
            break;
        }
    }
    actions::complete(kept);
    pending(domain, env, id, entry.run, job);
    Ok(Answer::Done)
}
