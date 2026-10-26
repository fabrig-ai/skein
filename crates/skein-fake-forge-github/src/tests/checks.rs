use super::{fixtures as f, read_support as r};
use skein_fake_forge_domain::api as a;
use skein_forge::{
    api,
    github::{
        self,
        document::{Document, Kind},
        request::{Method, Request},
    },
};
use skein_lib::Duration;

#[test]
fn github_fake_ci_contexts_keep_actions_run_job_and_rerun_attempt_ids() {
    for reruns in [0, 1000] {
        let mut domain_config = r::domain_config();
        domain_config.limits.contexts = 4;
        let mut h = f::Harness::with_checks(
            f::limits(),
            f::config(),
            domain_config,
            None,
            a::Checks {
                contexts: Box::from([f::boxed(b"build"), f::boxed(b"lint")]),
                latency_min: Duration::from_secs(1),
                latency_max: Duration::from_secs(1),
                silent: 0,
                passes: 0,
                reruns,
                cue: None,
            },
        );
        let (_, commit) = r::pull(&mut h);
        let token = r::token(&mut h);
        let api::Answer::Checks(rows) =
            r::read(&mut h, &token, api::Read::Checks { commit: r::object(commit) }).unwrap()
        else {
            panic!("checks")
        };
        assert_eq!(rows.len(), 2);
        for (row, job) in rows.iter().zip([1, 2]) {
            let attempt = row.job.as_ref().expect("failed Actions job");
            assert_eq!((attempt.run, attempt.job, attempt.attempt), (1, job, if reruns == 0 { 1 } else { 2 }));
            assert_eq!(row.check, api::Check::Failed);
        }
        let request = Request::CheckRuns {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"widgets"),
            commit: r::object(commit),
            page: 1,
            per_page: 2,
        };
        let encoded = github::request::encode(&request, &h.limits.github).unwrap();
        let answer = h.ask(Method::Get, &encoded.target, Some(token.expose()), b"");
        let (runs, _) = h.decode(Kind::CheckRuns, &answer);
        for row in &runs {
            let github::document::Row::Run(row) = row else { panic!("run") };
            assert_eq!(row.app.as_deref(), Some(b"github-actions".as_slice()));
        }
    }
}

#[test]
fn github_fake_reported_statuses_stay_statuses_and_empty_combined_is_pending_zero() {
    let mut h = r::harness();
    let (_, commit) = r::pull(&mut h);
    let token = r::token(&mut h);
    for reported in [false, true] {
        if reported {
            r::call(
                &mut h,
                a::Op::Write(a::Write::Status { commit, context: f::boxed(b"human"), state: a::Check::Passed }),
            );
        }
        for runs in [false, true] {
            let request = if runs {
                Request::CheckRuns {
                    owner: f::boxed(b"acme"),
                    name: f::boxed(b"widgets"),
                    commit: r::object(commit),
                    page: 1,
                    per_page: 2,
                }
            } else {
                Request::CombinedStatus {
                    owner: f::boxed(b"acme"),
                    name: f::boxed(b"widgets"),
                    commit: r::object(commit),
                    page: 1,
                    per_page: 2,
                }
            };
            let encoded = github::request::encode(&request, &h.limits.github).unwrap();
            let answer = h.ask(Method::Get, &encoded.target, Some(token.expose()), b"");
            let (rows, document) = h.decode(if runs { Kind::CheckRuns } else { Kind::CombinedStatus }, &answer);
            assert_eq!(rows.len(), u32::from(reported && !runs));
            if !runs {
                let Document::Combined(combined) = document else { panic!("combined") };
                assert_eq!(combined.total, u64::from(reported));
                assert_eq!(combined.check, if reported { api::Check::Passed } else { api::Check::Pending });
            }
        }
    }
}
