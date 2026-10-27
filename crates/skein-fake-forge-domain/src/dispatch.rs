//! Dispatch-only workflows, runner faults and explicit reruns (fake-forge.md, 2.3).
use crate::{
    Config, Domain, Limits,
    actions::{self, Conclusion, Job, Kept, Run, RunEvent, RunState},
    api::{Answer, Check, Cue, Error, Failing, Permission, What},
    ci,
    domain::{self, Alarm},
    faults,
    store::{Repository, Status},
};
use alloc::boxed::Box;
use skein_lib::{
    Env, Id, List, Map,
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
    // Check the entire context admission before any run, timer or status changes.
    let mut contexts = skein_lib::Set::with_capacity(env.limits.limits.contexts);
    if let Some(statuses) = repository.statuses.get(&commit) {
        for (name, _) in statuses {
            contexts.insert(copy_of(name)).expect("retained contexts fit");
        }
    }
    for name in &workflow.jobs {
        let context = joined(&file, name);
        if contexts.insert(context).is_err() {
            return Err(Error::Full);
        }
    }
    ensure_statuses(domain, env, id, commit)?;
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
    let mut kept = Kept { run, jobs };
    actions::complete(&mut kept);
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
        pending(domain, env, id, run, job)?;
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

fn joined(file: &[u8], name: &[u8]) -> Box<[u8]> {
    let capacity =
        file.len().checked_add(3).expect("bounded context").checked_add(name.len()).expect("bounded context");
    let mut out = List::with_capacity(u32::try_from(capacity).expect("bounded context"));
    for part in [file, b" / ", name] {
        for &byte in part {
            out.push(byte).expect("context capacity");
        }
    }
    out.into_boxed()
}

fn ensure_statuses(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, commit: u64) -> Result<(), Error> {
    if domain.repositories.get(id).expect("repository").statuses.contains_key(&commit) {
        return Ok(());
    }
    if !ci::room(domain, id) {
        return Err(Error::Full);
    }
    domain
        .repositories
        .get_mut(id)
        .expect("repository")
        .statuses
        .insert(commit, Map::with_capacity(env.limits.limits.contexts))
        .expect("status room admitted");
    Ok(())
}

fn pending(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, run: u64, job: u64) -> Result<(), Error> {
    let repository = domain.repositories.get(id).expect("repository");
    let kept = repository.dispatched.get(&run).expect("dispatched run");
    let (_, job_view) = actions::latest(repository, job)?;
    let file = kept.run.workflow.as_ref().expect("workflow run");
    let workflow = workflow(repository, file)?;
    let (silent, min, max) = (workflow.silent, workflow.latency_min, workflow.latency_max);
    let commit = kept.run.commit;
    let context = actions::context(&kept.run, &job_view);
    let link = actions::job_link(repository, &kept.run, job);
    let status = Status {
        state: Check::Pending,
        author: env.limits.ci,
        at: domain::clock(env),
        rerun: job_view.attempt > 1,
        description: ci::description(Check::Pending),
        link,
        job: Some(crate::api::JobRef { run, job, attempt: job_view.attempt }),
    };
    assert!(ci::set(domain, id, commit, &context, status), "pending contexts admitted together");
    ci::reported(domain, env, id, commit, context, Check::Pending, env.limits.ci);
    if domain.rng.chance(silent) {
        domain.tally.silent = domain.tally.silent.saturating_add(1);
    } else {
        let span = faults::draw(domain, min, max);
        domain
            .timers
            .arm(Alarm::Workflow { repository: id, run, job }, env.now.saturating_add(span))
            .expect("one timer per retained dispatched job");
    }
    Ok(())
}

pub(crate) fn report(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, run: u64, job: u64) {
    let repository = domain.repositories.get(id).expect("repository");
    let kept = repository.dispatched.get(&run).expect("timer's retained run");
    let commit = kept.run.commit;
    let lost = domain.rng.chance(env.limits.lost);
    let file = kept.run.workflow.as_ref().expect("dispatched workflow");
    let workflow = workflow(repository, file).expect("configured workflow");
    let state = if lost {
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
    let repository = domain.repositories.get_mut(id).expect("repository");
    let kept = repository.dispatched.get_mut(&run).expect("run");
    let mut context = None;
    for index in 0..kept.jobs.len() {
        let entry = kept.jobs.get_mut(index).expect("retained job");
        if entry.id == job {
            entry.state = RunState::Completed;
            entry.conclusion = Some(if state == Check::Passed { Conclusion::Success } else { Conclusion::Failure });
            entry.steps = actions::outcome_steps(&entry.name, state, failing);
            context = Some(actions::context(&kept.run, entry));
            break;
        }
    }
    actions::complete(kept);
    let context = context.expect("timer's retained job");
    let link = actions::job_link(repository, &actions::run(repository, run).expect("run").run, job);
    let status = Status {
        state,
        author: env.limits.ci,
        at: domain::clock(env),
        rerun: true,
        description: ci::description(state),
        link,
        job: Some(crate::api::JobRef { run, job, attempt: actions::latest(repository, job).expect("job").1.attempt }),
    };
    // A run outlives status eviction; its own verdict remains readable in that case.
    if ensure_statuses(domain, env, id, commit).is_ok() && ci::set(domain, id, commit, &context, status) {
        ci::reported(domain, env, id, commit, context, state, env.limits.ci);
    } else {
        domain.tally.unreported = domain.tally.unreported.saturating_add(1);
    }
    domain.tally.verdicts = domain.tally.verdicts.saturating_add(1);
    crate::queue::verdict(domain, env, id, commit);
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
    let context = actions::context(&kept.run, &entry);
    if let Some(statuses) = repository.statuses.get(&commit)
        && !statuses.contains_key(&*context)
        && statuses.len() == statuses.capacity()
    {
        return Err(Error::Full);
    }
    ensure_statuses(domain, env, id, commit)?;
    let repository = domain.repositories.get_mut(id).expect("repository");
    let kept = repository.dispatched.get_mut(&entry.run).expect("dispatched run");
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
    pending(domain, env, id, entry.run, job)?;
    Ok(Answer::Done)
}
