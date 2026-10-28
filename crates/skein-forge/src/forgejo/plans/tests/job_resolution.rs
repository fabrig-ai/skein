//! Actions names are verified API ids for each check state and each statuses page.
use crate::api::{self, Answer, Error, Op, Read};
use crate::forgejo::plans::plan::Step;
use crate::forgejo::tests::fixtures::{self, HEAD, REPO, Run};
use alloc::{boxed::Box, format};
use skein_lib::Time;

// The link's run number is 5, its job index 0; the API ids are run 7 and job 1.
const LINK: &str = "/fixture/specimen/actions/runs/5/jobs/0";
const RUNS: &[u8] = br#"{"total_count":1,"workflow_runs":[{"id":7,"index_in_repo":5,"commit_sha":"0100000000000000000000000000000000000000"}]}"#;
const JOBS: &[u8] = br#"[{"id":1,"run_id":7,"attempt":0}]"#;
const ATTEMPT: api::JobAttempt = api::JobAttempt { head: HEAD, run: 7, job: 1, attempt: 0 };

#[expect(clippy::disallowed_macros, reason = "test wire fixtures are built from the state and link table")]
fn status_body(state: &str, link: &str, total: u64) -> Box<[u8]> {
    format!(r#"{{"state":"{state}","total_count":{total},"statuses":[{{"creator":null,"updated_at":"1970-01-01T00:00:00Z","context":"build","status":"{state}","description":"CI","target_url":"{link}"}}]}}"#).into_bytes().into_boxed_slice()
}
#[expect(clippy::disallowed_macros, reason = "test expectation builds the absolute URL for its table row")]
fn status(check: api::Check, link: &str, job: Option<api::JobAttempt>) -> api::Status {
    api::Status {
        at: Time::ZERO,
        context: Box::from(&b"build"[..]),
        check,
        description: Box::from(&b"CI"[..]),
        url: format!("http://127.0.0.1:3000{link}").into_bytes().into_boxed_slice(),
        job,
    }
}

#[test]
fn checks_resolve_pending_passed_and_failed_jobs_by_api_id() {
    for (state, check) in
        [("pending", api::Check::Pending), ("success", api::Check::Passed), ("failure", api::Check::Failed)]
    {
        let mut run = Run::known(Op::Read(Read::Checks { commit: HEAD }));
        run.begin();
        run.answer(200, &status_body(state, LINK, 1));
        run.answer(200, RUNS);
        assert_eq!(
            run.answer(200, JOBS),
            Step::Done(Ok(Answer::Checks(Box::new([status(check, LINK, Some(ATTEMPT))]))))
        );
        run.assert_sent(&[
            "GET /api/v1/repos/fixture/specimen/commits/0100000000000000000000000000000000000000/status?page=1&limit=2",
            "GET /api/v1/repos/fixture/specimen/actions/runs?head_sha=0100000000000000000000000000000000000000&run_number=5&page=1&limit=1",
            "GET /api/v1/repos/fixture/specimen/actions/runs/7/jobs",
        ]);
    }
}

#[test]
fn statuses_resolve_jobs_on_the_requested_page_and_keep_more() {
    for (state, check, ci) in [
        ("pending", api::Check::Pending, api::Ci::Pending),
        ("success", api::Check::Passed, api::Ci::Passed),
        ("failure", api::Check::Failed, api::Ci::Failed),
    ] {
        for (total, more) in [(3, false), (4, true)] {
            let mut run = Run::known(Op::Read(Read::Statuses { commit: HEAD, page: 2 }));
            run.begin();
            run.answer(200, &status_body(state, LINK, total));
            run.answer(200, RUNS);
            assert_eq!(
                run.answer(200, JOBS),
                Step::Done(Ok(Answer::Statuses { ci, statuses: Box::new([status(check, LINK, Some(ATTEMPT))]), more }))
            );
            run.assert_sent(&[
                "GET /api/v1/repos/fixture/specimen/commits/0100000000000000000000000000000000000000/status?page=2&limit=2",
                "GET /api/v1/repos/fixture/specimen/actions/runs?head_sha=0100000000000000000000000000000000000000&run_number=5&page=1&limit=1",
                "GET /api/v1/repos/fixture/specimen/actions/runs/7/jobs",
            ]);
        }
    }
}

#[test]
fn unresolvable_links_name_no_job_in_checks_or_statuses() {
    for read in [Read::Checks { commit: HEAD }, Read::Statuses { commit: HEAD, page: 1 }] {
        for (link, lookup) in [
            ("/external-ci", &[][..]),
            (LINK, &[(200, br#"{"workflow_runs":[],"total_count":0}"#.as_slice())][..]),
            (LINK, &[(200, RUNS), (200, b"[]".as_slice())][..]),
            (LINK, &[(404, b"{}".as_slice())][..]),
            (LINK, &[(200, RUNS), (404, b"{}".as_slice())][..]),
            (LINK, &[(200, RUNS), (200, br#"[{"id":1,"run_id":8,"attempt":0}]"#.as_slice())][..]),
            (LINK, &[(200, br#"{"total_count":1,"workflow_runs":[{"id":7,"index_in_repo":5,"commit_sha":"0300000000000000000000000000000000000000"}]}"#.as_slice())][..]),
        ] {
            let mut run = Run::known(Op::Read(read.clone()));
            run.begin();
            let mut step = run.answer(200, &status_body("success", link, 1));
            for (code, body) in lookup {
                step = run.answer(*code, body);
            }
            let statuses = Box::new([status(api::Check::Passed, link, None)]);
            let expected = if let Read::Checks { .. } = read { Answer::Checks(statuses) } else { Answer::Statuses { ci: api::Ci::Passed, statuses, more: false } };
            assert_eq!(step, Step::Done(Ok(expected)), "{read:?} {link} {lookup:?}");
            assert_eq!(run.tables().repository(REPO), Some(&fixtures::specimen()));
        }
    }
}

#[test]
fn status_resolution_keeps_the_answer_byte_bound() {
    let expected = Answer::Statuses {
        ci: api::Ci::Passed,
        statuses: Box::new([status(api::Check::Passed, LINK, Some(ATTEMPT))]),
        more: false,
    };
    let size =
        u32::try_from(crate::forgejo::plans::reads::size::answer(&expected, fixtures::BOUNDS.rows).unwrap()).unwrap();
    for (bytes, fits) in [(size, true), (size.saturating_sub(1), false)] {
        let mut run = Run::with(
            crate::forgejo::plans::plan::Bounds { answer_bytes: bytes, ..fixtures::BOUNDS },
            Op::Read(Read::Statuses { commit: HEAD, page: 1 }),
        );
        run.begin();
        let mut step = run.answer(200, &status_body("success", LINK, 1));
        if fits {
            run.answer(200, RUNS);
            step = run.answer(200, JOBS);
        }
        assert_eq!(step, if fits { Step::Done(Ok(expected.clone())) } else { Step::Done(Err(Error::TooLarge)) });
    }
}
