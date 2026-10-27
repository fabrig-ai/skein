#![expect(clippy::disallowed_macros, reason = "the document harness formats fixture values and literal expected rows")]
#![expect(
    clippy::disallowed_methods,
    reason = "the document harness transforms finite fixture rows with ordinary Rust iterators"
)]
#![expect(
    clippy::disallowed_types,
    reason = "the document test harness retains finite fixture rows and mutation buffers in ordinary Rust collections"
)]
#![expect(
    clippy::must_use_candidate,
    reason = "fixture-returning helpers are only consumed by assertions or other fixture construction"
)]
use super::super::forgejo;
use skein_forge::forgejo::{
    document::{
        ChangedFile, CombinedStatus, CommitStatus, Compare, Document, FileStatus, Job, Row, Run, StatusState, Step,
        StepStatus,
    },
    request::Request,
    server::Answer,
};
use skein_lib::Wall;
pub fn file(n: u32) -> ChangedFile {
    ChangedFile {
        filename: format!("file-{n}.txt").into_bytes().into_boxed_slice(),
        previous_filename: None,
        status: FileStatus::Added,
    }
}
pub fn run() -> Run {
    Run {
        id: 1,
        index_in_repo: 1,
        commit_sha: forgejo::sha(16),
        workflow: Box::from(*b"conformance.yaml"),
        event: Box::from(*b"push"),
        trigger_actor: 1,
        status: StepStatus::Failure,
        html_url: Box::from(*b"http://127.0.0.1:3000/fixture/specimen/actions/runs/1"),
        created_at: Wall::from_nanos(1_577_817_011_000_000_000),
    }
}
pub fn jobs() -> Box<[Job]> {
    Box::from([
        Job { id: 1, run_id: 1, attempt: 1, name: Box::from(*b"pass"), status: StepStatus::Success },
        Job { id: 2, run_id: 1, attempt: 1, name: Box::from(*b"fail"), status: StepStatus::Failure },
    ])
}
pub fn status(context: &[u8], status: StatusState, description: &[u8], target_url: &[u8], at: u64) -> CommitStatus {
    CommitStatus {
        creator: None,
        updated_at: Wall::from_nanos(at),
        context: Box::from(context),
        status,
        description: Box::from(description),
        target_url: Box::from(target_url),
    }
}
pub fn raw() -> Request {
    Request::Raw { at: forgejo::specimen(), path: Box::from(*b"file-0.txt"), reference: forgejo::sha(4) }
}
pub fn failing_job() -> Answer {
    Answer::Job {
        job: Job { id: 2, run_id: 1, attempt: 1, name: Box::from(*b"fail"), status: StepStatus::Failure },
        steps: Box::from([
            Step { number: 0, name: Box::from(*b"Set up job"), status: StepStatus::Success },
            Step {
                number: 1,
                name: Box::from(*b"printf 'KIT_CONFORMANCE_FAILED_JOB\\n'\nexit 1\n"),
                status: StepStatus::Failure,
            },
            Step { number: 2, name: Box::from(*b"Complete job"), status: StepStatus::Failure },
        ]),
    }
}
pub fn step_log(job: u64, step: u32) -> Request {
    Request::JobLog { at: forgejo::specimen(), job, attempt: 1, step: Some(step) }
}
pub fn log() -> Request {
    Request::JobLog { at: forgejo::specimen(), job: 2, attempt: 1, step: None }
}
pub fn cases() -> Vec<(&'static str, Request, Option<Answer>, u16)> {
    vec![
        (
            "pull-files-page-1",
            Request::PullFiles { at: forgejo::specimen(), number: 1, page: forgejo::page(1, 2) },
            Some(Answer::Files(Box::from([file(0), file(1)]))),
            200,
        ),
        (
            "pull-files-page-2",
            Request::PullFiles { at: forgejo::specimen(), number: 1, page: forgejo::page(2, 2) },
            Some(Answer::Files(Box::from([file(2)]))),
            200,
        ),
        ("raw-file", raw(), None, 200),
        ("raw-file-range", raw(), None, 206),
        (
            "actions-combined-status",
            Request::CombinedStatus { at: forgejo::specimen(), commit: forgejo::sha(16), page: forgejo::page(1, 2) },
            Some(Answer::CombinedStatus {
                state: CombinedStatus { state: Some(StatusState::Failure), total_count: 2 },
                sha: Some(forgejo::sha(16)),
                statuses: Box::from([
                    status(
                        b"conformance / pass (push)",
                        StatusState::Success,
                        b"Successful in 0s",
                        b"/fixture/specimen/actions/runs/1/jobs/0",
                        1_577_817_011_000_000_000,
                    ),
                    status(
                        b"conformance / fail (push)",
                        StatusState::Failure,
                        b"Failing after 0s",
                        b"/fixture/specimen/actions/runs/1/jobs/1",
                        1_577_817_012_000_000_000,
                    ),
                ]),
            }),
            200,
        ),
        (
            "actions-runs",
            Request::Runs {
                at: forgejo::specimen(),
                head_sha: forgejo::sha(16),
                run_number: 1,
                page: forgejo::page(1, 1),
            },
            Some(Answer::Runs { total_count: 1, runs: Box::from([run()]) }),
            200,
        ),
        ("actions-run", Request::Run { at: forgejo::specimen(), run: 1 }, Some(Answer::Run(run())), 200),
        ("actions-jobs", Request::Jobs { at: forgejo::specimen(), run: 1 }, Some(Answer::Jobs(jobs())), 200),
        ("actions-job-log", log(), None, 200),
        ("actions-job-log-range", log(), None, 206),
        ("actions-job", Request::Job { at: forgejo::specimen(), job: 2 }, Some(failing_job()), 200),
        ("actions-job-log-step", step_log(2, 1), None, 200),
        ("actions-job-log-step-suffix", step_log(2, 1), None, 206),
        ("actions-job-log-past-last-step", step_log(2, 3), None, 404),
        ("actions-job-log-empty-suffix", step_log(1, 1), None, 206),
        (
            "compare-verification-false",
            Request::Compare { at: forgejo::specimen(), before: forgejo::sha(6), after: forgejo::sha(4), files: true },
            Some(Answer::Compare {
                compare: Compare { total_commits: 3 },
                commits: Box::from([forgejo::sha(4), forgejo::sha(11), forgejo::sha(8)]),
                files: Box::from([file(2), file(1), file(0)]),
            }),
            200,
        ),
        (
            "compare-reverse-files-false",
            Request::Compare { at: forgejo::specimen(), before: forgejo::sha(4), after: forgejo::sha(6), files: false },
            Some(Answer::Compare {
                compare: Compare { total_commits: 0 },
                commits: Box::from([]),
                files: Box::from([]),
            }),
            200,
        ),
        (
            "compare-files-false",
            Request::Compare { at: forgejo::specimen(), before: forgejo::sha(6), after: forgejo::sha(4), files: false },
            Some(Answer::Compare {
                compare: Compare { total_commits: 3 },
                commits: Box::from([forgejo::sha(4), forgejo::sha(11), forgejo::sha(8)]),
                files: Box::from([]),
            }),
            200,
        ),
    ]
}
pub fn expected(answer: Answer) -> (Document, Vec<Row>) {
    match answer {
        Answer::Files(v) => (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::File).collect()),
        Answer::Compare { compare, commits, files } => {
            let mut rows = commits.into_vec().into_iter().map(Row::Commit).collect::<Vec<_>>();
            rows.extend(files.into_vec().into_iter().map(Row::File));
            (Document::Compare(compare), rows)
        }
        Answer::Runs { total_count, runs } => {
            (Document::Runs { total_count }, runs.into_vec().into_iter().map(Row::Run).collect())
        }
        Answer::Job { job, steps } => (Document::Job(job), steps.into_vec().into_iter().map(Row::Step).collect()),
        Answer::Run(v) => (Document::Run(v), vec![]),
        Answer::Jobs(v) => (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::Job).collect()),
        Answer::CombinedStatus { state, statuses, .. } => {
            (Document::CombinedStatus(state), statuses.into_vec().into_iter().map(Row::Status).collect())
        }
        Answer::Org(_)
        | Answer::CreatedRepository(_)
        | Answer::CreatedFile { .. }
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::Pulls(_)
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Tree { .. }
        | Answer::Commits { .. }
        | Answer::Permission(_) => panic!("file family answer"),
    }
}
