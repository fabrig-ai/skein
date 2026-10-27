use super::fixtures::{self, Harness, boxed};
use crate::Route;
use skein_fake_forge_domain::api::{self as a, Op, Read, Write};
use skein_forge::forgejo::{
    document::{Document, Row, User},
    request::Request,
};
use skein_lib::{Time, bytes};

#[test]
fn collaborators_are_listed_without_permissions() {
    let mut h = Harness::new(fixtures::config());
    skein_fake_forge_domain::grant(&mut h.domain, b"org/repo", fixtures::ADMIN, a::Permission::Admin);
    skein_fake_forge_domain::grant(&mut h.domain, b"org/repo", fixtures::PERSON, a::Permission::Write);
    let request = Request::Collaborators { at: fixtures::at(), page: fixtures::PAGE };
    h.send(&request);
    assert_eq!(h.status(), 200);
    assert_eq!(
        &*h.decoded(&request).rows,
        &[
            Row::User(User { id: fixtures::ADMIN, login: boxed(b"engine"), is_admin: false, full_name: boxed(b"") }),
            Row::User(User { id: fixtures::PERSON, login: boxed(b"person"), is_admin: false, full_name: boxed(b"") }),
        ]
    );
    assert_eq!(bytes::find(&h.body(), b"permission"), None);
}
#[test]
fn pull_by_branches_answers_the_oldest_pull() {
    let mut h = Harness::new(fixtures::config());
    h.push(b"work");
    let open = || {
        Op::Write(Write::OpenPull {
            title: boxed(b"pull"),
            body: boxed(b"body"),
            head: boxed(b"work"),
            base: boxed(b"main"),
            opens_empty: false,
        })
    };
    h.call(fixtures::PERSON, open()).expect("first pull");
    h.call(fixtures::PERSON, Op::Write(Write::Close { number: FIRST_PULL })).expect("close pull");
    h.call(fixtures::PERSON, open()).expect("replacement pull");
    h.ask(b"GET", b"/api/v1/repos/org/repo/pulls/main/work", b"");
    assert_eq!(h.status(), 200);
    let request = Request::PullFor { at: fixtures::at(), base: boxed(b"main"), head: boxed(b"work") };
    let Document::Pull(pull) = h.decoded(&request).document else { panic!("pull response") };
    assert_eq!(pull.number, FIRST_PULL);
}
// The first item allocated in the fixture repository is pull number one.
const FIRST_PULL: u64 = 1;
#[test]
fn unrouted_paths_increment_only_the_unrouted_count() {
    let mut h = Harness::new(fixtures::config());
    h.ask(b"GET", b"/api/v1/not-a-route", b"");
    assert_eq!(h.status(), 404);
    assert_eq!(h.service().tally().total, fixtures::ONE_REQUEST);
    assert_eq!(h.service().tally().unrouted, fixtures::ONE_REQUEST);
    assert_eq!(h.domain_calls(), fixtures::NO_CALLS);
}
#[test]
fn unavailable_calls_answer_503() {
    let mut h = Harness::new(fixtures::config());
    h.fake_env.limits.unavailable = 1000;
    h.ask(b"POST", b"/api/v1/repos/org/repo/issues", br#"{"title":"created","body":"body"}"#);
    assert_eq!(h.status(), 503);
    h.fake_env.limits.unavailable = 0;
    assert_eq!(
        h.call(fixtures::PERSON, Op::Read(Read::Item { number: FIRST_PULL, after: 0 })),
        Err(a::Error::Missing(a::What::Item))
    );
}
#[test]
fn retry_after_rounds_up_to_whole_seconds() {
    use skein_lib::Duration;
    let mut h = Harness::new(fixtures::config());
    h.fake_env.limits.rate_limit = 1;
    h.fake_env.limits.rate_window = Duration::from_millis(1501);
    h.fake_env.limits.resolution = Duration::from_millis(1);
    h.fake_env.limits.skew = skein_fake_forge_domain::Skew::Ahead(Duration::ZERO);
    h.fake_env.limits.latency_min = Duration::ZERO;
    h.fake_env.limits.latency_max = Duration::ZERO;
    for _ in 0..RATE_REQUESTS {
        h.request(b"GET /api/v1/repos/org/repo/issues HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nAuthorization: token 1111111111111111111111111111111111111111\r\n\r\n");
        h.run_at(Time::ZERO);
    }
    assert_eq!(h.status(), 429);
    assert!(bytes::find(&h.head(), b"retry-after: 2\r\n").is_some());
}
// One admitted call consumes the only slot; the next receives its reset.
const RATE_REQUESTS: u32 = 2;
#[test]
fn an_unknown_token_is_answered_401_and_counted() {
    let mut h = Harness::new(fixtures::config());
    h.request(b"GET /api/v1/user HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nAuthorization: token 0000000000000000000000000000000000000000\r\n\r\n");
    h.run();
    assert_eq!(h.status(), 401);
    assert_eq!(h.service().tally().unauthorized, 1);
    assert_eq!(h.service().tally().total, 1);
    assert_eq!(h.domain_calls(), 0);
}
#[test]
fn the_issue_listing_pages_leastupdate_capped_at_max_response_items() {
    let mut h = Harness::new(fixtures::config());
    for title in [b"one".as_slice(), b"two", b"three"] {
        h.issue(title);
    }
    h.ask(b"GET", b"/api/v1/repos/org/repo/issues?sort=leastupdate&page=1&limit=99", b"");
    assert_eq!(h.status(), 200);
    let body = h.body();
    assert!(bytes::find(&body, b"\"title\":\"one\"") < bytes::find(&body, b"\"title\":\"two\""));
    assert_eq!(bytes::find(&body, b"\"title\":\"three\""), None);
    assert_eq!(h.domain_calls(), 1);
}
#[test]
fn a_limit_without_a_page_takes_the_default_paging_num() {
    let mut h = Harness::new(fixtures::config());
    for title in [b"one".as_slice(), b"two", b"three"] {
        h.issue(title);
    }
    h.ask(b"GET", b"/api/v1/repos/org/repo/issues?limit=1", b"");
    assert_eq!(h.status(), 200);
    let body = h.body();
    assert!(bytes::find(&body, b"\"title\":\"three\"").is_some());
    assert!(bytes::find(&body, b"\"title\":\"two\"").is_some());
    assert_eq!(bytes::find(&body, b"\"title\":\"one\""), None);
}
#[test]
fn a_merge_answers_200_with_no_body() {
    let mut h = Harness::new(fixtures::config());
    let commit = h.push(b"work");
    let a::Answer::Created(number) = h
        .call(
            2,
            Op::Write(Write::OpenPull {
                title: boxed(b"pull"),
                body: boxed(b"body"),
                head: boxed(b"work"),
                base: boxed(b"main"),
                opens_empty: false,
            }),
        )
        .expect("pull")
    else {
        panic!("created")
    };
    assert_eq!(number, 1);
    assert_eq!(commit, 2);
    h.ask(
        b"POST",
        b"/api/v1/repos/org/repo/pulls/1/merge",
        b"{\"Do\":\"squash\",\"head_commit_id\":\"975835de1c9756ce000000000000000200000000\"}",
    );
    assert_eq!(h.status(), 200);
    assert!(h.body().is_empty());
    assert!(bytes::find(&h.head(), b"Content-Length: 0").is_some());
    assert_eq!(h.domain_calls(), 1);
}
#[test]
fn requests_are_counted_by_route_user_and_total() {
    let mut h = Harness::new(fixtures::config());
    h.ask(b"GET", b"/api/v1/user", b"");
    h.ask(b"GET", b"/api/v1/repos/org/repo/issues", b"");
    h.ask(b"POST", b"/api/v1/repos/org/repo/issues", b"{\"title\":\"created\",\"body\":\"body\"}");
    assert_eq!(h.status(), 201);
    let tally = h.service().tally();
    assert_eq!(tally.total, 3);
    assert_eq!(tally.users.get(&1), Some(&3));
    for (route, count) in [(Route::User, 1), (Route::Issues, 1), (Route::CreateIssue, 1)] {
        assert_eq!(tally.routes.get(&route), Some(&count), "{route:?}");
    }
    assert_eq!(h.domain_calls(), 3, "listing, write and readback");
    assert_eq!(tally.unrouted, 0);
    assert!(bytes::find(&h.body(), b"\"title\":\"created\"").is_some());
}
#[test]
fn a_reset_closes_before_reading_and_a_timeout_drops_or_answers_500() {
    let mut config = fixtures::config();
    config.faults.reset = 1000;
    let mut h = Harness::new(config);
    h.request(b"GET /api/v1/user HTTP/1.1\r\nHost: localhost\r\n\r\n");
    h.run();
    assert!(h.closed);
    assert_eq!(h.service().tally().total, 0);
    assert_eq!(h.domain_calls(), 0);
    for (drop, server_error, closed, status) in [(1000, 0, true, None), (0, 1000, false, Some(500))] {
        let mut config = fixtures::config();
        config.faults.drop = drop;
        config.faults.server_error = server_error;
        let mut h = Harness::new(config);
        fault_write(&mut h, closed, status, "drawn post-write fault");
    }
    for (seed, closed, status) in [(0, true, None), (2, false, Some(500))] {
        let mut h = Harness::new(fixtures::config());
        h.service.rng = skein_lib::Rng::new(seed);
        h.fake_env.limits.timeouts = 1000;
        fault_write(&mut h, closed, status, "domain timeout");
    }
}
fn fault_write(h: &mut Harness, closed: bool, status: Option<u16>, row: &str) {
    h.ask(b"POST", b"/api/v1/repos/org/repo/issues", b"{\"title\":\"created\",\"body\":\"body\"}");
    assert_eq!(h.closed, closed, "{row}");
    assert_eq!(h.domain_calls(), 1, "{row}: no read after fault");
    assert_eq!(h.service().tally().total, 1, "{row}: HTTP entrance once");
    if let Some(status) = status {
        assert_eq!(h.status(), status, "{row}");
    }
    h.fake_env.limits.timeouts = 0;
    let Ok(a::Answer::Item { item, .. }) = h.call(2, Op::Read(Read::Item { number: 1, after: 0 })) else {
        panic!("{row}: write happened")
    };
    assert_eq!(item.number, 1, "{row}");
}
#[test]
fn deleting_a_missing_branch_answers_500_with_an_empty_body() {
    let mut h = Harness::new(fixtures::config());
    h.ask(b"DELETE", b"/api/v1/repos/org/repo/branches/missing", b"");
    assert_eq!(h.status(), 500);
    assert!(h.body().is_empty());
}
#[test]
fn a_pull_with_nothing_to_merge_is_opened_201() {
    let mut h = Harness::new(fixtures::config());
    h.branch(b"same", 1);
    h.ask(
        b"POST",
        b"/api/v1/repos/org/repo/pulls",
        b"{\"title\":\"empty\",\"body\":\"body\",\"head\":\"same\",\"base\":\"main\"}",
    );
    assert_eq!(h.status(), 201);
    assert!(bytes::find(&h.body(), b"\"number\":1").is_some());
    assert_eq!(h.domain_calls(), 2, "write then readback");
    let Ok(a::Answer::Pull(pull)) = h.call(2, Op::Read(Read::Pull { number: 1 })) else { panic!("pull readback") };
    assert_eq!(pull.number, 1);
}
#[test]
fn a_ci_status_has_no_creator_and_a_link_relative_to_the_origin() {
    let mut h = Harness::new(fixtures::config());
    assert_eq!(h.push(b"work"), 2);
    h.ask(b"GET", b"/api/v1/repos/org/repo/commits/975835de1c9756ce000000000000000200000000/status", b"");
    assert_eq!(h.status(), 200);
    let body = h.body();
    assert!(bytes::find(&body, b"\"creator\":null").is_some());
    assert!(bytes::find(&body, b"\"target_url\":\"/org/repo/actions/runs/1/jobs/0\"").is_some());
    assert!(bytes::find(&body, b"\"status\":\"failure\"").is_some());
    assert_eq!(h.domain_calls(), 1);
    assert!(h.fake_env.now > Time::ZERO);
}
#[test]
fn a_malformed_write_is_422_without_a_domain_call() {
    let mut h = Harness::new(fixtures::config());
    h.ask(b"POST", b"/api/v1/repos/org/repo/issues", b"{\"title\":3}");
    assert_eq!(h.status(), 422);
    assert_eq!(h.domain_calls(), 0);
    assert_eq!(h.service().tally().unrouted, 1);
}
#[test]
fn a_rate_refusal_uses_the_forges_clock_for_retry_after() {
    let mut h = Harness::new(fixtures::config());
    h.fake_env.limits.rate_limit = 1;
    h.ask(b"GET", b"/api/v1/repos/org/repo/issues", b"");
    h.ask(b"GET", b"/api/v1/repos/org/repo/issues", b"");
    assert_eq!(h.status(), 429);
    assert!(bytes::find(&h.head(), b"retry-after: 10\r\n").is_some());
}
#[test]
fn ci_statuses_keep_a_null_creator_before_and_after_a_success() {
    let request = Request::CombinedStatus {
        at: fixtures::at(),
        commit: crate::routes::object(2),
        page: skein_forge::forgejo::request::Page { page: None, limit: None },
    };
    for (state, expected) in
        [(a::Check::Pending, b"\"status\":\"pending\"".as_slice()), (a::Check::Passed, b"\"status\":\"success\"")]
    {
        let answer = a::Answer::Checks(Box::new([a::CheckSummary {
            status: a::Status {
                description: Box::new([]),
                link: Box::new([]),
                context: boxed(b"build"),
                state,
                author: 3,
                at: Time::ZERO,
            },
            description: boxed(b"CI"),
            link: boxed(b"/org/repo/actions/runs/1/jobs/0"),
            job: None,
        }]));
        let reply = crate::response::answer(
            &fixtures::config(),
            &crate::setup::Store::new(&fixtures::limits()),
            &fixtures::limits(),
            &request,
            &crate::response::Select::Document,
            &answer,
        )
        .expect("status");
        assert!(bytes::find(&reply.body, b"\"creator\":null").is_some(), "{state:?}");
        assert!(bytes::find(&reply.body, expected).is_some(), "{state:?}");
    }
}
#[test]
fn an_unpaged_thread_past_the_projection_budget_is_refused() {
    let answer = a::Answer::Comments(Box::new([a::Comment {
        id: 1,
        author: 2,
        body: Box::new([b'x'; 2048]),
        created: Time::ZERO,
        edited: None,
    }]));
    let request = Request::Comments { at: fixtures::at(), number: 1 };
    let mut limits = fixtures::limits();
    limits.documents.writer.length = 1024;
    assert_eq!(
        crate::response::answer(
            &fixtures::config(),
            &crate::setup::Store::new(&limits),
            &limits,
            &request,
            &crate::response::Select::Document,
            &answer
        ),
        Err(a::Error::TooLarge)
    );
}

#[test]
fn a_failed_jobs_steps_and_failed_steps_log_are_served_over_http() {
    let mut h = Harness::new(fixtures::config());
    assert_eq!(h.push(b"work"), 2);
    h.ask(b"GET", b"/api/v1/repos/org/repo/actions/jobs/1", b"");
    assert_eq!(h.status(), 200);
    let job = h.body();
    for step in [
        b"{\"number\":0,\"name\":\"Set up job\",\"status\":\"success\"}".as_slice(),
        b"{\"number\":1,\"name\":\"Run build\",\"status\":\"failure\"}",
        b"{\"number\":2,\"name\":\"Complete job\",\"status\":\"failure\"}",
    ] {
        assert!(bytes::find(&job, step).is_some(), "{step:?}");
    }
    h.ask(b"GET", b"/api/v1/repos/org/repo/actions/jobs/1/logs?attempt=1&step=1", b"");
    assert_eq!(h.status(), 200);
    assert_eq!(
        &*h.body(),
        b"failing check build 0\nfailing check build 1\nfailing check build 2\nfailing check build 3\n"
    );
    assert_eq!(h.domain_calls(), 2);
}

#[test]
fn a_steps_log_end_is_served_by_a_suffix_range() {
    let mut h = Harness::new(fixtures::config());
    assert_eq!(h.push(b"work"), 2);
    h.request(b"GET /api/v1/repos/org/repo/actions/jobs/1/logs?attempt=1&step=1 HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nAuthorization: token 1111111111111111111111111111111111111111\r\nRange: bytes=-10\r\n\r\n");
    h.run();
    assert_eq!(h.status(), 206);
    assert!(bytes::find(&h.head(), b"content-range: bytes 78-87/88\r\n").is_some());
    assert_eq!(&*h.body(), b"k build 3\n");
}

#[test]
fn unauthorized_heads_are_drained_and_counted_in_the_bound() {
    // The fixture's tables and protocol state, plus each connection's captured head.
    const CONNECTIONS: u64 = 4;
    // Includes the git compact buffers, Basic credential, bounded parent frontier
    // and target_bytes reserved for the host's bound origin.
    // The tally reserves 80 route entries; repository identities and full paths
    // reserve documents.inner slots for repositories created during a run.
    const BASE_BYTES: u64 = 10_254_596;
    const HEAD_BYTES: u64 = 4_664;
    let config = fixtures::config();
    let mut limits = fixtures::limits();
    let deliveries = crate::delivery::worst_case(&limits, &config).unwrap();
    assert_eq!(crate::worst_case(&limits, &config), Some(BASE_BYTES + CONNECTIONS * HEAD_BYTES + deliveries));
    assert_eq!(crate::worst_case(&limits, &config), Some(10_273_252 + deliveries));
    limits.http.head += 1;
    assert_eq!(crate::worst_case(&limits, &config), Some(10_273_396 + deliveries));
    let mut h = Harness::new(config);
    h.ask_as(b"GET", b"/api/v1/user", b"", b"wrong-token");
    assert_eq!(h.status(), 401);
    let call = h.take_head().expect("unauthorized entrance captured");
    assert_eq!(&*call.target, b"/api/v1/user");
    assert_eq!(call.header(b"authorization"), Some(b"".as_slice()));
    assert_eq!(h.take_head(), None);
    h.ask(b"GET", b"/api/v1/repos/org/repo/issues", b"");
    assert_eq!(h.status(), 200);
    let call = h.take_head().expect("next entrance captured");
    assert_eq!(&*call.target, b"/api/v1/repos/org/repo/issues");
    assert_eq!(call.header(b"authorization"), Some(b"".as_slice()));
}

#[test]
fn forgejo_pull_is_false_while_mergeability_is_checking() {
    let mut h = Harness::new(fixtures::config());
    h.push(b"work");
    h.call(
        fixtures::PERSON,
        Op::Write(Write::OpenPull {
            title: boxed(b"pull"),
            body: boxed(b"body"),
            head: boxed(b"work"),
            base: boxed(b"main"),
            opens_empty: false,
        }),
    )
    .unwrap();
    skein_fake_forge_domain::check(
        &mut h.domain,
        &h.fake_env,
        b"org/repo",
        FIRST_PULL,
        skein_lib::Duration::from_secs(60),
    );
    let request = Request::Pull { at: fixtures::at(), number: FIRST_PULL };
    h.send(&request);
    assert_eq!(h.status(), 200);
    let Document::Pull(pull) = h.decoded(&request).document else { panic!("pull response") };
    assert!(!pull.mergeable);
    assert!(bytes::find(&h.body(), b"\"mergeable\":false").is_some());
}

#[test]
fn an_issue_title_edit_answers_201_and_reads_back() {
    let mut h = Harness::new(fixtures::config());
    let number = h.issue(b"before");
    let request =
        Request::EditIssue { at: fixtures::at(), number, title: Some(boxed(b"after")), body: None, state: None };
    h.send(&request);
    assert_eq!(h.status(), 201);
    let Document::Issue(issue) = h.decoded(&request).document else { panic!("issue response") };
    assert_eq!(&*issue.title, b"after");
    let Ok(a::Answer::Item { item, .. }) = h.call(fixtures::PERSON, Op::Read(Read::Item { number, after: 0 })) else {
        panic!("edited issue")
    };
    assert_eq!(&*item.title, b"after");
}
#[test]
fn an_issues_close_and_reopen_answer_201() {
    let mut h = Harness::new(fixtures::config());
    let number = h.issue(b"issue");
    for state in [skein_forge::forgejo::document::IssueState::Closed, skein_forge::forgejo::document::IssueState::Open]
    {
        let request = Request::EditIssue { at: fixtures::at(), number, title: None, body: None, state: Some(state) };
        h.send(&request);
        assert_eq!(h.status(), 201, "{state:?}");
        let Document::Issue(issue) = h.decoded(&request).document else { panic!("issue response") };
        assert_eq!(issue.state, state);
    }
}
#[test]
fn a_reviewer_request_answers_201_over_http() {
    let (mut h, number) = reviewer_pull();
    h.send(&Request::RequestReviewers { at: fixtures::at(), number, reviewers: Box::new([boxed(b"person")]) });
    assert_eq!(h.status(), 201);
    assert!(h.body().is_empty());
}
#[test]
fn a_reviewer_removal_answers_204_with_no_body() {
    let (mut h, number) = reviewer_pull();
    h.call(fixtures::ADMIN, Op::Write(Write::RequestReviewers { number, reviewers: Box::new([fixtures::PERSON]) }))
        .expect("seed reviewer");
    h.send(&Request::RemoveReviewers { at: fixtures::at(), number, reviewers: Box::new([boxed(b"person")]) });
    assert_eq!(h.status(), 204);
    assert!(h.body().is_empty());
}
fn reviewer_pull() -> (Harness, u64) {
    let mut h = Harness::new(fixtures::config());
    h.push(b"work");
    let a::Answer::Created(number) = h
        .call(
            fixtures::ADMIN,
            Op::Write(Write::OpenPull {
                title: boxed(b"pull"),
                body: boxed(b"body"),
                head: boxed(b"work"),
                base: boxed(b"main"),
                opens_empty: false,
            }),
        )
        .expect("seed pull")
    else {
        panic!("created pull")
    };
    (h, number)
}
#[test]
fn a_branch_deletion_answers_204_and_removes_the_branch() {
    let mut h = Harness::new(fixtures::config());
    h.push(b"work");
    h.send(&Request::DeleteBranch { at: fixtures::at(), branch: boxed(b"work") });
    assert_eq!(h.status(), 204);
    assert!(h.body().is_empty());
    assert_eq!(
        h.call(fixtures::PERSON, Op::Read(Read::Branch { branch: boxed(b"work") })),
        Err(a::Error::Missing(a::What::Branch))
    );
}

#[test]
fn refs_prefix_returns_404_one_or_many_among_222_branches() {
    let mut h = Harness::with_branches(222);
    h.ask(b"GET", b"/api/v1/repos/org/repo/git/refs/heads/temper%2F", b"");
    assert_eq!(h.status(), 404);
    for index in 0_u64..221 {
        let name = match index {
            0 => boxed(b"temper/0"),
            1 => boxed(b"temper/1"),
            _ => boxed(skein_lib::Decimal::of(index).as_bytes()),
        };
        h.call(fixtures::PERSON, Op::Git(a::Git::Create { branch: name, commit: 1 })).expect("created branch");
    }
    h.ask(b"GET", b"/api/v1/repos/org/repo/git/refs/heads/temper%2F0", b"");
    assert_eq!(h.status(), 200);
    let request = Request::Branches {
        at: fixtures::at(),
        prefix: Some(boxed(b"temper/0")),
        page: skein_forge::forgejo::request::Page { page: None, limit: None },
    };
    assert_eq!(h.decoded(&request).rows.len(), 1);
    h.ask(b"GET", b"/api/v1/repos/org/repo/git/refs/heads/temper%2F", b"");
    assert_eq!(h.status(), 200);
    let request = Request::Branches {
        at: fixtures::at(),
        prefix: Some(boxed(b"temper/")),
        page: skein_forge::forgejo::request::Page { page: None, limit: None },
    };
    let got = h.decoded(&request);
    assert_eq!(got.rows.len(), 2);
    for (row, name) in got.rows.iter().zip([b"temper/0", b"temper/1"]) {
        let Row::Branch(branch) = row else { panic!("branch ref") };
        assert_eq!(branch.name.as_ref(), name);
    }
}
