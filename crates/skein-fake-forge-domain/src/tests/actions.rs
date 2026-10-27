use super::*;
use crate::api::{Failing, Job, JobRef, Order, Run, Step, StepStatus};

fn failing(reruns: u32) -> (Harness, u64) {
    let mut configured = setup();
    configured.checks.passes = 0;
    configured.checks.reruns = reruns;
    let mut h = Harness::with(CALM, configured);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).unwrap();
    h.settle();
    (h, work)
}

#[test]
fn a_failing_ci_status_links_its_run_number_and_job_index() {
    let mut configured = setup();
    configured.checks.passes = 0;
    configured.checks.contexts = names(&[b"ci", b"fmt"]);
    let mut h = Harness::with(CALM, configured);
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).unwrap();
    h.settle();
    let Answer::Checks(checks) = h.ok(PERSON, read(Read::Checks { commit: work })) else { unreachable!("checks") };
    let check = checks.first().unwrap();
    assert_eq!(check.status.state, Check::Failed);
    assert_eq!(&*check.link, b"/ai/kit/actions/runs/1/jobs/0");
    assert_eq!(check.job, Some(JobRef { run: 1, job: 1, attempt: 1 }));
    let second = checks.get(1).unwrap();
    assert_eq!(&*second.link, b"/ai/kit/actions/runs/1/jobs/1");
    assert_eq!(second.job, Some(JobRef { run: 1, job: 2, attempt: 1 }));
    assert_eq!(
        h.ok(PERSON, read(Read::Runs { workflow: None, commit: Some(work), number: Some(1), page: 1, limit: 1 })),
        Answer::Runs(Box::new([ci_run(1, 1, work)]))
    );
}

#[test]
fn a_commits_run_is_found_by_its_number_and_its_jobs_by_run() {
    let (mut h, work) = failing(0);
    let expected = ci_run(1, 1, work);
    let rows = [
        (
            Read::Runs { workflow: None, commit: Some(work), number: Some(1), page: 1, limit: 1 },
            Ok(Answer::Runs(Box::new([expected.clone()]))),
        ),
        (
            Read::Runs { workflow: None, commit: Some(work), number: Some(2), page: 1, limit: 1 },
            Ok(Answer::Runs(Box::new([]))),
        ),
        (Read::Run { run: 1 }, Ok(Answer::Run(expected.clone()))),
        (Read::Run { run: 2 }, Err(Error::Missing(What::Job))),
        (Read::Jobs { run: 1 }, Ok(Answer::Jobs(Box::new([ci_job(1, 1, 0, 1)])))),
        (Read::Jobs { run: 2 }, Err(Error::Missing(What::Job))),
    ];
    for (input, expected) in rows {
        assert_eq!(h.call(PERSON, read(input.clone())), expected, "{input:?}");
        assert_eq!(h.call(STRANGER, read(input.clone())), Err(Error::Forbidden), "{input:?} permission");
    }
    let mut configured = setup();
    configured.id = 3;
    configured.name = copy_of(b"ai/other");
    let first = crate::repository(&mut h.domain, &CALM, configured);
    crate::grant(&mut h.domain, b"ai/other", ENGINE, Permission::Write);
    let next = h.commit(first, &[(b"src", b"two")]);
    let created = h.env.now;
    h.call_on(b"ai/other", ENGINE, Op::Git(Git::Push { branch: copy_of(b"work"), commit: next, expected: None }))
        .unwrap();
    assert_eq!(
        h.call_on(b"ai/other", ENGINE, read(Read::Run { run: 2 })),
        Ok(Answer::Run(Run { created, state: crate::api::RunState::Running, conclusion: None, ..ci_run(2, 1, next) }))
    );
    assert_eq!(
        h.call_on(b"ai/other", ENGINE, read(Read::Jobs { run: 2 })),
        Ok(Answer::Jobs(Box::new([Job {
            state: crate::api::RunState::Running,
            conclusion: None,
            steps: Box::new([]),
            ..ci_job(2, 2, 0, 1)
        }])))
    );
}

#[test]
fn a_job_log_is_read_by_job_id_and_attempt() {
    let (mut h, _) = failing(0);
    for (job, attempt, expected) in [
        (1, 1, Ok(Answer::File(copy_of(WHOLE_LOG)))),
        (1, 2, Err(Error::Missing(What::Job))),
        (2, 1, Err(Error::Missing(What::Job))),
    ] {
        assert_eq!(
            h.call(PERSON, read(Read::Log { job, attempt, step: None })),
            expected,
            "job {job} attempt {attempt}"
        );
    }
}

#[test]
fn a_rerun_context_answers_its_second_attempt() {
    let (mut h, work) = failing(1000);
    assert_eq!(h.ok(PERSON, read(Read::Jobs { run: 1 })), Answer::Jobs(Box::new([ci_job(1, 1, 0, 2)])));
    let Answer::Checks(checks) = h.ok(PERSON, read(Read::Checks { commit: work })) else { unreachable!("checks") };
    assert_eq!(checks.first().unwrap().job, Some(JobRef { run: 1, job: 1, attempt: 2 }));
    assert_eq!(h.call(PERSON, read(Read::Log { job: 1, attempt: 1, step: None })), Err(Error::Missing(What::Job)));
    assert_eq!(h.ok(PERSON, read(Read::Log { job: 1, attempt: 2, step: None })), Answer::File(copy_of(WHOLE_LOG)));
}

#[test]
fn a_thread_is_one_read_oldest_first() {
    let mut h = Harness::new(CALM);
    assert_eq!(h.ok(PERSON, create(b"thread", b"body", &[])), Answer::Created(1));
    let rows = [(b"one".as_slice(), 1), (b"two".as_slice(), 2), (b"three".as_slice(), 3)];
    for (text, id) in rows {
        assert_eq!(
            h.ok(PERSON, write(Write::Comment { number: 1, body: copy_of(text) })),
            Answer::Commented(id),
            "{text:?}"
        );
    }
    let Answer::Comments(comments) = h.ok(ENGINE, read(Read::Comments { number: 1 })) else { unreachable!("thread") };
    assert_eq!(comments.len(), 3);
    for (comment, (text, id)) in comments.iter().zip(rows) {
        assert_eq!((comment.id, &*comment.body), (id, text), "comment {id}");
    }
    assert_eq!(h.call(PERSON, read(Read::Comments { number: 2 })), Err(Error::Missing(What::Item)));
    assert_eq!(h.call(STRANGER, read(Read::Comments { number: 1 })), Err(Error::Forbidden));
}

#[test]
fn reviewers_are_requested_and_removed_by_their_own_writes() {
    let (mut h, _) = failing(0);
    assert_eq!(h.open(b"work"), Ok(Answer::Created(1)));
    for (input, expected, reviewers) in [
        (Write::RequestReviewers { number: 1, reviewers: Box::new([PERSON]) }, Ok(Answer::Done), [PERSON].as_slice()),
        (
            Write::RequestReviewers { number: 1, reviewers: Box::new([PERSON, ADMIN]) },
            Ok(Answer::Done),
            [PERSON, ADMIN].as_slice(),
        ),
        (
            Write::RequestReviewers { number: 1, reviewers: Box::new([ENGINE]) },
            Err(Error::Forbidden),
            [PERSON, ADMIN].as_slice(),
        ),
        (
            Write::RemoveReviewers { number: 1, reviewers: Box::new([STRANGER]) },
            Err(Error::Forbidden),
            [PERSON, ADMIN].as_slice(),
        ),
        (Write::RemoveReviewers { number: 1, reviewers: Box::new([PERSON]) }, Ok(Answer::Done), [ADMIN].as_slice()),
    ] {
        assert_eq!(h.call(PERSON, write(input.clone())), Err(Error::Forbidden), "{input:?} permission");
        assert_eq!(h.call(ENGINE, write(input.clone())), expected, "{input:?}");
        assert_eq!(&*h.pull(1).reviewers, reviewers, "{input:?} reviewers");
    }
}

#[test]
fn items_come_newest_created_first_in_forgejos_default_order() {
    let mut h = Harness::new(CALM);
    for title in [b"one".as_slice(), b"two", b"three"] {
        h.ok(PERSON, create(title, b"body", &[]));
    }
    h.ok(PERSON, write(Write::Comment { number: 1, body: copy_of(b"updated") }));
    for (order, expected) in [(Order::NewestCreated, [3, 2, 1]), (Order::LeastUpdated, [2, 3, 1])] {
        let mut found = List::with_capacity(LIMITS.items);
        for page in 1..=2 {
            let input = Read::Items {
                order,
                state: None,
                kind: None,
                labels: Box::new([]),
                author: None,
                since: Time::ZERO,
                page,
                limit: 2,
            };
            let Answer::Items { items, .. } = h.ok(PERSON, read(input)) else { unreachable!("items") };
            for item in &items {
                found.push(item.number).unwrap();
            }
        }
        assert_eq!(found.as_slice(), expected, "{order:?}");
    }
}

#[test]
fn pull_for_answers_the_open_pull_before_an_older_closed_one() {
    let (mut h, _) = failing(0);
    assert_eq!(h.open(b"work"), Ok(Answer::Created(1)));
    h.ok(ENGINE, write(Write::Close { number: 1 }));
    assert_eq!(h.open(b"work"), Ok(Answer::Created(2)));
    for (input, expected) in [
        (Read::PullFor { head: copy_of(b"work"), base: copy_of(MAIN) }, 2),
        (Read::FirstPull { head: copy_of(b"work"), base: copy_of(MAIN) }, 1),
    ] {
        let Answer::Pull(pull) = h.ok(PERSON, read(input.clone())) else { unreachable!("pull") };
        assert_eq!(pull.number, expected, "{input:?}");
        assert_eq!(pull.merge_base, Some(FIRST), "{input:?} merge base");
    }
    let Answer::Pulls(pulls) = h.ok(PERSON, read(Read::OpenPulls { page: 1, limit: 2 })) else {
        unreachable!("open pulls")
    };
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls.first().unwrap().number, 2);
    h.ok(ENGINE, write(Write::Close { number: 2 }));
    let Answer::Pull(pull) = h.ok(PERSON, read(Read::PullFor { head: copy_of(b"work"), base: copy_of(MAIN) })) else {
        unreachable!("oldest closed pull")
    };
    assert_eq!(pull.number, 1);
}

#[test]
fn an_empty_pull_opens_only_when_opens_empty() {
    let mut h = Harness::new(CALM);
    h.ok(ENGINE, create_branch(b"work", FIRST));
    for (head, opens_empty, expected) in [
        (b"work".as_slice(), false, Err(Error::NothingToMerge)),
        (b"main".as_slice(), true, Err(Error::NothingToMerge)),
        (b"work".as_slice(), true, Ok(Answer::Created(1))),
    ] {
        let input = Write::OpenPull {
            title: copy_of(b"empty"),
            body: Box::new([]),
            head: copy_of(head),
            base: copy_of(MAIN),
            opens_empty,
        };
        assert_eq!(h.call(ENGINE, write(input)), expected, "head {head:?} opens_empty {opens_empty}");
    }
}

const WORKFLOW_LOG: &[u8] = b"failing check ci 0\nfailing check ci 1\nfailing check ci 2\nfailing check ci 3\n";
const WHOLE_LOG: &[u8] =
    b"set up ci\nfailing check ci 0\nfailing check ci 1\nfailing check ci 2\nfailing check ci 3\njob ci failed\n";

#[test]
fn a_failed_job_read_by_id_has_three_steps_failing_at_the_workflow() {
    let (mut h, _) = failing(0);
    assert_eq!(
        h.ok(PERSON, read(Read::Job { job: 1 })),
        Answer::Job {
            job: ci_job(1, 1, 0, 1),
            steps: Box::new([
                Step {
                    number: 0,
                    name: copy_of(b"Set up job"),
                    status: StepStatus::Success,
                    log: copy_of(b"set up ci\n")
                },
                Step { number: 1, name: copy_of(b"Run ci"), status: StepStatus::Failure, log: copy_of(WORKFLOW_LOG) },
                Step {
                    number: 2,
                    name: copy_of(b"Complete job"),
                    status: StepStatus::Failure,
                    log: copy_of(b"job ci failed\n")
                },
            ]),
        }
    );
}

#[test]
fn a_cancelled_job_has_its_workflow_and_completion_steps_cancelled() {
    let (mut h, _) = failing(0);
    crate::set_failing(&mut h.domain, REPOSITORY, b"ci", Failing::NoStep);
    let Answer::Job { steps, .. } = h.ok(PERSON, read(Read::Job { job: 1 })) else { unreachable!("job") };
    assert_eq!(steps[0].status, StepStatus::Success);
    assert_eq!(steps[1].status, StepStatus::Cancelled);
    assert_eq!(steps[2].status, StepStatus::Cancelled);
    assert_eq!(&*steps[2].log, b"job ci cancelled\n");
}

#[test]
fn each_step_of_a_failed_jobs_log_is_a_slice_of_its_own() {
    let (mut h, _) = failing(0);
    for (step, expected) in [
        (Some(0), b"set up ci\n".as_slice()),
        (Some(1), WORKFLOW_LOG),
        (Some(2), b"job ci failed\n"),
        (None, WHOLE_LOG),
    ] {
        assert_eq!(
            h.ok(PERSON, read(Read::Log { job: 1, attempt: 1, step })),
            Answer::File(copy_of(expected)),
            "{step:?}"
        );
    }
}

#[test]
fn a_log_step_past_the_last_is_missing() {
    let (mut h, _) = failing(0);
    assert_eq!(h.call(PERSON, read(Read::Log { job: 1, attempt: 1, step: Some(3) })), Err(Error::Missing(What::Job)));
}

#[test]
fn a_job_read_by_id_answers_its_latest_attempt() {
    let (mut h, _) = failing(1000);
    let Answer::Job { job, .. } = h.ok(PERSON, read(Read::Job { job: 1 })) else { unreachable!("job") };
    assert_eq!(job, ci_job(1, 1, 0, 2));
}

#[test]
fn a_passing_ci_jobs_read_by_id_is_missing() {
    let mut h = Harness::with(CALM, setup());
    let work = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", work).unwrap();
    h.settle();
    assert_eq!(h.call(PERSON, read(Read::Job { job: 1 })), Err(Error::Missing(What::Job)));
}

#[test]
fn forgotten_statuses_remove_the_commits_run() {
    let mut h = Harness::new(Config { limits: Limits { statuses: 1, ..LIMITS }, ..CALM });
    let one = h.commit(FIRST, &[(b"src", b"one")]);
    let two = h.commit(one, &[(b"src", b"two")]);
    h.push(ENGINE, b"work", one).expect("first head");
    h.push(ENGINE, b"work", two).expect("replacement head");
    assert_eq!(
        h.ok(PERSON, read(Read::Runs { workflow: None, commit: Some(one), number: Some(1), page: 1, limit: 1 })),
        Answer::Runs(Box::new([])),
        "the displaced commit's run was forgotten"
    );
}

fn ci_run(id: u64, number: u64, commit: u64) -> Run {
    Run {
        id,
        number,
        commit,
        workflow: None,
        event: crate::api::RunEvent::Push,
        user: ENGINE,
        state: crate::api::RunState::Completed,
        conclusion: Some(crate::api::Conclusion::Failure),
        created: Time::ZERO,
        inputs: Box::new([]),
    }
}
fn ci_job(id: u64, run: u64, index: u32, attempt: u32) -> Job {
    Job {
        id,
        run,
        index,
        attempt,
        name: copy_of(b"ci"),
        state: crate::api::RunState::Completed,
        conclusion: Some(crate::api::Conclusion::Failure),
        steps: crate::actions::steps(b"ci", Failing::Workflow),
    }
}

fn dispatch_harness(config: Config, jobs: &[&[u8]]) -> Harness {
    let mut configured = setup();
    configured.checks.contexts = Box::new([]);
    configured.workflows = Box::new([crate::api::Workflow {
        file: copy_of(b"gate.yml"),
        jobs: names(jobs),
        latency_min: Duration::from_secs(10),
        latency_max: Duration::from_secs(10),
        passes: 1000,
        silent: 0,
        cue: None,
    }]);
    Harness::with(config, configured)
}
fn dispatch() -> Op {
    write(Write::Dispatch {
        workflow: copy_of(b"gate.yml"),
        branch: copy_of(MAIN),
        inputs: Box::new([(copy_of(b"key"), copy_of(b"value"))]),
    })
}
fn workflow_runs(h: &Harness) -> Box<[Run]> {
    let Answer::Runs(runs) = h
        .inspect(&Read::Runs { workflow: Some(copy_of(b"gate.yml")), commit: None, number: None, page: 1, limit: 2 })
        .unwrap()
    else {
        unreachable!("runs")
    };
    runs
}
fn run_jobs(h: &Harness, run: u64) -> Box<[Job]> {
    let Answer::Jobs(jobs) = h.inspect(&Read::Jobs { run }).unwrap() else { unreachable!("jobs") };
    jobs
}

#[test]
fn a_dispatch_runs_its_workflow_at_the_branch_head() {
    let mut h = dispatch_harness(CALM, &[b"ci", b"fmt"]);
    let created = h.env.now;
    assert_eq!(h.call(ENGINE, dispatch()), Ok(Answer::Done));
    let runs = workflow_runs(&h);
    let run = &runs[0];
    assert_eq!(
        (run.commit, run.event, run.user, run.created),
        (FIRST, crate::api::RunEvent::Dispatch, ENGINE, created)
    );
    assert_eq!(&*run.inputs, &[(copy_of(b"key"), copy_of(b"value"))]);
    assert_eq!((run.state, run.conclusion), (crate::api::RunState::Running, None));
    let Answer::Statuses(statuses) = h.inspect(&Read::Statuses { commit: FIRST }).unwrap() else {
        unreachable!("statuses")
    };
    assert_eq!(statuses.len(), 2);
    for (status, (name, link)) in statuses.iter().zip([
        (b"gate.yml / ci".as_slice(), b"/ai/kit/actions/runs/1/jobs/0".as_slice()),
        (b"gate.yml / fmt".as_slice(), b"/ai/kit/actions/runs/1/jobs/1".as_slice()),
    ]) {
        assert_eq!(&*status.context, name);
        assert_eq!(status.state, Check::Pending);
        assert_eq!(&*status.link, link);
    }
    h.settle();
    let run = &workflow_runs(&h)[0];
    assert_eq!((run.state, run.conclusion), (crate::api::RunState::Completed, Some(crate::api::Conclusion::Success)));
    for job in run_jobs(&h, run.id) {
        assert_eq!(job.conclusion, Some(crate::api::Conclusion::Success));
        assert_eq!(job.steps.len(), 3);
    }
    let Answer::Checks(checks) = h.inspect(&Read::Checks { commit: FIRST }).unwrap() else { unreachable!("checks") };
    for check in checks {
        assert_eq!(check.status.state, Check::Passed);
        assert!(!check.link.is_empty());
    }
}

#[test]
fn two_dispatches_at_one_commit_make_two_runs() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    h.ok(ENGINE, dispatch());
    let runs = workflow_runs(&h);
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].number, runs[1].number), (2, 1));
    assert_eq!((runs[0].commit, runs[1].commit), (FIRST, FIRST));
    assert_ne!(run_jobs(&h, runs[0].id)[0].id, run_jobs(&h, runs[1].id)[0].id);
}

#[test]
fn a_push_starts_no_workflow() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    let head = h.commit(FIRST, &[(b"src", b"changed")]);
    h.push(ENGINE, b"work", head).unwrap();
    h.settle();
    assert!(workflow_runs(&h).is_empty());
}

#[test]
fn a_dispatch_of_a_missing_workflow_or_branch_is_missing() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    for (workflow, branch, missing) in
        [(b"missing".as_slice(), MAIN, What::File), (b"gate.yml", b"missing", What::Branch)]
    {
        assert_eq!(
            h.call(
                ENGINE,
                write(Write::Dispatch { workflow: copy_of(workflow), branch: copy_of(branch), inputs: Box::new([]) })
            ),
            Err(Error::Missing(missing))
        );
        assert!(workflow_runs(&h).is_empty());
    }
    assert_eq!(h.call(PERSON, dispatch()), Err(Error::Forbidden));
}

#[test]
fn a_cued_message_fails_the_workflow_step() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    let id = h.domain.id(REPOSITORY);
    h.domain.repositories.get_mut(id).unwrap().workflows[0].cue = Some(Cue::Message { fail: copy_of(b"FAIL") });
    let head =
        crate::commit(&mut h.domain, &CALM, FIRST, files(&[(b"src", b"change")]), b"FAIL this test").unwrap().unwrap();
    h.push(ENGINE, MAIN, head).unwrap();
    h.ok(ENGINE, dispatch());
    h.settle();
    let run = &workflow_runs(&h)[0];
    assert_eq!(run.commit, head);
    assert_eq!(run.conclusion, Some(crate::api::Conclusion::Failure));
    let jobs = run_jobs(&h, run.id);
    assert_eq!(jobs[0].steps[0].status, StepStatus::Success);
    assert_eq!(jobs[0].steps[1].status, StepStatus::Failure);
}

#[test]
fn a_setup_fault_fails_the_set_up_step() {
    let mut h = dispatch_harness(Config { setup_faults: 1000, ..CALM }, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    h.settle();
    let jobs = run_jobs(&h, workflow_runs(&h)[0].id);
    assert_eq!(jobs[0].steps[0].status, StepStatus::Failure);
    assert_eq!(jobs[0].steps[1].status, StepStatus::Cancelled);
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Failure));
}

#[test]
fn a_lost_run_fails_with_no_step_failed() {
    let mut h = dispatch_harness(Config { lost: 1000, ..CALM }, &[b"ci", b"fmt"]);
    h.ok(ENGINE, dispatch());
    h.settle();
    let run = &workflow_runs(&h)[0];
    assert_eq!(run.conclusion, Some(crate::api::Conclusion::Failure));
    for job in run_jobs(&h, run.id) {
        assert_eq!(job.conclusion, Some(crate::api::Conclusion::Failure));
        for step in &job.steps {
            assert_ne!(step.status, StepStatus::Failure);
        }
    }
}

#[test]
fn a_rerun_of_a_failed_job_runs_its_next_attempt() {
    let mut h = dispatch_harness(Config { setup_faults: 1000, ..CALM }, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    h.settle();
    let run = workflow_runs(&h)[0].id;
    let job = run_jobs(&h, run)[0].id;
    h.env.limits.setup_faults = 0;
    assert_eq!(h.call(ENGINE, write(Write::Rerun { job })), Ok(Answer::Done));
    assert_eq!(run_jobs(&h, run)[0].attempt, 2);
    assert_eq!(run_jobs(&h, run)[0].conclusion, None);
    assert_eq!(workflow_runs(&h)[0].state, crate::api::RunState::Running);
    h.settle();
    assert_eq!(run_jobs(&h, run)[0].conclusion, Some(crate::api::Conclusion::Success));
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Success));
    assert_eq!(h.call(ENGINE, write(Write::Rerun { job: 999 })), Err(Error::Missing(What::Job)));
}

#[test]
fn a_rerun_before_its_run_completes_is_refused() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    let job = run_jobs(&h, workflow_runs(&h)[0].id)[0].id;
    assert_eq!(h.call(ENGINE, write(Write::Rerun { job })), Err(Error::Refused));
    assert_eq!(run_jobs(&h, workflow_runs(&h)[0].id)[0].attempt, 1);
}

#[test]
fn runs_are_listed_by_workflow_and_commit_newest_first() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    h.ok(ENGINE, dispatch());
    let head = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"src", b"new", ENGINE).unwrap();
    h.ok(ENGINE, dispatch());
    for (page, expected) in [(1, [2].as_slice()), (2, [1].as_slice()), (3, [].as_slice())] {
        let Answer::Runs(runs) = h
            .inspect(&Read::Runs {
                workflow: Some(copy_of(b"gate.yml")),
                commit: Some(FIRST),
                number: None,
                page,
                limit: 1,
            })
            .unwrap()
        else {
            unreachable!("runs")
        };
        let mut numbers = List::with_capacity(2);
        for run in runs {
            numbers.push(run.number).unwrap();
        }
        assert_eq!(numbers.as_slice(), expected);
    }
    assert_eq!(workflow_runs(&h)[0].commit, head);
    let Answer::Runs(runs) = h
        .inspect(&Read::Runs { workflow: Some(copy_of(b"absent")), commit: None, number: None, page: 1, limit: 2 })
        .unwrap()
    else {
        unreachable!("runs")
    };
    assert!(runs.is_empty());
}

#[test]
fn workflow_runs_past_their_limit_drop_the_oldest() {
    let mut h = dispatch_harness(Config { limits: Limits { workflow_runs: 1, ..LIMITS }, ..CALM }, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    let old = workflow_runs(&h)[0].id;
    let job = run_jobs(&h, old)[0].id;
    h.ok(ENGINE, dispatch());
    assert_eq!(workflow_runs(&h).len(), 1);
    assert_eq!(h.inspect(&Read::Run { run: old }), Err(Error::Missing(What::Job)));
    assert_eq!(h.inspect(&Read::Job { job }), Err(Error::Missing(What::Job)));
    h.settle();
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Success));
}

#[test]
fn a_status_keeps_its_description_and_link() {
    let mut h = Harness::new(CALM);
    h.ok(
        ENGINE,
        write(Write::Status {
            commit: FIRST,
            context: copy_of(b"gate"),
            state: Check::Passed,
            description: copy_of(b"proof passed"),
            link: copy_of(b"https://proof/42"),
        }),
    );
    let Answer::Statuses(statuses) = h.inspect(&Read::Statuses { commit: FIRST }).unwrap() else {
        unreachable!("statuses")
    };
    assert_eq!(&*statuses[0].description, b"proof passed");
    assert_eq!(&*statuses[0].link, b"https://proof/42");
    let Answer::Checks(checks) = h.inspect(&Read::Checks { commit: FIRST }).unwrap() else { unreachable!("checks") };
    assert_eq!(checks[0].description, statuses[0].description);
    assert_eq!(checks[0].link, statuses[0].link);
    assert_eq!(checks[0].status, statuses[0]);
}

#[test]
fn overlapping_dispatches_keep_the_job_identity_of_the_status_that_reported() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    h.ok(ENGINE, dispatch());
    let first = workflow_runs(&h)[0].id;
    let first_job = run_jobs(&h, first)[0].id;
    let id = h.domain.id(REPOSITORY);
    let definition = &mut h.domain.repositories.get_mut(id).unwrap().workflows[0];
    definition.latency_min = Duration::from_secs(2);
    definition.latency_max = Duration::from_secs(2);
    definition.passes = 0;
    h.ok(ENGINE, dispatch());
    h.settle();
    let Answer::Checks(checks) = h.inspect(&Read::Checks { commit: FIRST }).unwrap() else { unreachable!("checks") };
    assert_eq!(checks[0].status.state, Check::Failed);
    assert_eq!(&*checks[0].link, b"/ai/kit/actions/runs/1/jobs/0");
    assert_eq!(checks[0].job, Some(JobRef { run: first, job: first_job, attempt: 1 }));
}

#[test]
fn explicit_reruns_also_advance_a_completed_commits_ci_job() {
    let (mut h, _) = failing(0);
    for attempt in [2, 3] {
        assert_eq!(h.call(ENGINE, write(Write::Rerun { job: 1 })), Ok(Answer::Done));
        assert_eq!(run_jobs(&h, 1)[0].attempt, attempt);
        assert_eq!(run_jobs(&h, 1)[0].state, crate::api::RunState::Running);
        h.settle();
        assert_eq!(run_jobs(&h, 1)[0].conclusion, Some(crate::api::Conclusion::Failure));
    }
}

#[test]
fn workflow_and_input_limits_refuse_before_making_a_run() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    let too_many = write(Write::Dispatch {
        workflow: copy_of(b"gate.yml"),
        branch: copy_of(MAIN),
        inputs: Box::new([
            (copy_of(b"a"), copy_of(b"v")),
            (copy_of(b"b"), copy_of(b"v")),
            (copy_of(b"c"), copy_of(b"v")),
        ]),
    });
    assert_eq!(h.call(ENGINE, too_many), Err(Error::TooLarge));
    assert!(workflow_runs(&h).is_empty());
    let mut empty = dispatch_harness(Config { limits: Limits { workflow_runs: 0, ..LIMITS }, ..CALM }, &[b"ci"]);
    assert_eq!(empty.call(ENGINE, dispatch()), Err(Error::Full));
    assert!(workflow_runs(&empty).is_empty());
}

#[test]
fn a_silent_workflow_job_keeps_its_run_running() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    let id = h.domain.id(REPOSITORY);
    h.domain.repositories.get_mut(id).unwrap().workflows[0].silent = 1000;
    h.ok(ENGINE, dispatch());
    h.settle();
    assert_eq!(workflow_runs(&h)[0].state, crate::api::RunState::Running);
    assert_eq!(run_jobs(&h, workflow_runs(&h)[0].id)[0].conclusion, None);
}

#[test]
fn dispatch_runs_and_commit_ci_keep_separate_identities_and_one_numbering() {
    let mut h = dispatch_harness(CALM, &[b"ci"]);
    let id = h.domain.id(REPOSITORY);
    h.domain.repositories.get_mut(id).unwrap().checks.contexts = names(&[b"ci"]);
    h.domain.repositories.get_mut(id).unwrap().failing = Box::new([Failing::Workflow]);
    let head = h.commit(FIRST, &[(b"src", b"new")]);
    h.push(ENGINE, MAIN, head).unwrap();
    assert!(workflow_runs(&h).is_empty());
    h.ok(ENGINE, dispatch());
    h.settle();
    let Answer::Runs(runs) =
        h.inspect(&Read::Runs { workflow: None, commit: Some(head), number: None, page: 1, limit: 2 }).unwrap()
    else {
        unreachable!("runs")
    };
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].number, runs[0].event), (2, crate::api::RunEvent::Dispatch));
    assert_eq!((runs[1].number, runs[1].event), (1, crate::api::RunEvent::Push));
    h.push(ENGINE, b"work", head).unwrap();
    assert_eq!(workflow_runs(&h).len(), 1);
    let Answer::Statuses(statuses) = h.inspect(&Read::Statuses { commit: head }).unwrap() else {
        unreachable!("statuses")
    };
    assert_eq!(statuses.len(), 2);
}

#[test]
fn a_seed_replays_workflow_latency_runner_faults_and_reruns() {
    for seed in 0..16 {
        let config = Config { setup_faults: 350, lost: 250, ..CALM };
        let mut first = dispatch_harness(config, &[b"ci", b"fmt"]);
        let mut second = dispatch_harness(config, &[b"ci", b"fmt"]);
        for h in [&mut first, &mut second] {
            h.domain.rng = skein_lib::Rng::new(seed);
            let id = h.domain.id(REPOSITORY);
            let definition = &mut h.domain.repositories.get_mut(id).unwrap().workflows[0];
            definition.passes = 450;
            definition.latency_min = Duration::from_secs(2);
            definition.latency_max = Duration::from_secs(20);
            h.ok(ENGINE, dispatch());
            h.settle();
            let job = run_jobs(h, workflow_runs(h)[0].id)[0].id;
            h.ok(ENGINE, write(Write::Rerun { job }));
            h.settle();
        }
        assert_eq!(workflow_runs(&first), workflow_runs(&second), "seed {seed}");
        assert_eq!(run_jobs(&first, 1), run_jobs(&second, 1), "seed {seed}");
        assert_eq!(
            first.inspect(&Read::Statuses { commit: FIRST }),
            second.inspect(&Read::Statuses { commit: FIRST }),
            "seed {seed}"
        );
    }
}
