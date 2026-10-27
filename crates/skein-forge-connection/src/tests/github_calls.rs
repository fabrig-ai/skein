//! Ordinary calls use only their owner's accepted installation.
use super::fixtures::{self as f, FORGE, Harness, REPO};
use crate::{
    Request,
    installations::{Clock, Tokens},
};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read, Write};
use skein_lib::{Duration, Secret, Time};

fn branch() -> Op {
    Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) })
}
fn locate(owner: &[u8]) -> Op {
    Op::Read(Read::Locate { owner: Box::from(owner), name: Box::from(b"widgets".as_slice()) })
}
fn known() -> Harness {
    let mut h = f::github_calls();
    f::github_repository(&mut h, REPO, 22);
    h
}
fn begin(h: &mut Harness, op: Op) {
    h.call(1, REPO, op);
    h.run(Duration::ZERO);
    h.connected(FORGE);
}
fn answered(h: &mut Harness, cost: u32, result: Result<Answer, Error>) {
    h.run(Duration::ZERO);
    let answer = h.answered(1).expect("one terminal");
    assert_eq!((answer.cost, &answer.result), (cost, &result));
}
#[test]
fn protection_joins_sources_and_preserves_result_and_cost() {
    const RULES_PATH: &[u8] = b"GET /repos/acme/widgets/rules/branches/main?per_page=100&page=1 HTTP/1.1";
    const RULESET_PATH: &[u8] = b"GET /repos/acme/widgets/rulesets/11 HTTP/1.1";
    const CLASSIC_PATH: &[u8] = b"GET /repos/acme/widgets/branches/main/protection HTTP/1.1";
    const ABSENT: &[u8] = br#"{"message":"Branch not protected"}"#;
    const RULES: &[u8] = br#"[
        {"ruleset_id":11,"type":"pull_request","parameters":{"required_approving_review_count":2,"dismiss_stale_reviews_on_push":true,"require_code_owner_review":true}},
        {"ruleset_id":11,"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"build"},{"context":"lint"}]}},
        {"ruleset_id":11,"type":"merge_queue"}
    ]"#;
    const BYPASS: &[u8] = br#"{"current_user_can_bypass":"always"}"#;
    const CLASSIC: &[u8] = br#"{"required_status_checks":{"contexts":["lint","classic"]},"required_pull_request_reviews":{"required_approving_review_count":3,"dismiss_stale_reviews":false,"require_code_owner_reviews":false},"required_signatures":{"enabled":true},"required_linear_history":{"enabled":true}}"#;
    // One charged request for rules, one for classic, and one for each distinct ruleset.
    const ABSENT_COST: u32 = 2;
    const JOINED_COST: u32 = 3;
    for (name, responses, cost, expected) in [
        (
            "absent",
            [(RULES_PATH, 200, b"[]".as_slice()), (CLASSIC_PATH, 404, ABSENT)].as_slice(),
            ABSENT_COST,
            Ok(Answer::Protection(None)),
        ),
        (
            "joined",
            [(RULES_PATH, 200, RULES), (RULESET_PATH, 200, BYPASS), (CLASSIC_PATH, 200, CLASSIC)].as_slice(),
            JOINED_COST,
            Ok(Answer::Protection(Some(api::Protection {
                branch: Box::from(*b"main"),
                contexts: Box::new([Box::from(*b"build"), Box::from(*b"lint"), Box::from(*b"classic")]),
                approvals: 3,
                dismiss_stale: true,
                code_owners: true,
                linear: true,
                signatures: true,
                queue: true,
                bypass: api::Bypass::Never,
            }))),
        ),
        (
            "forbidden",
            [(RULES_PATH, 200, b"[]".as_slice()), (CLASSIC_PATH, 403, br#"{"message":"Forbidden"}"#)].as_slice(),
            ABSENT_COST,
            Err(Error::Forbidden),
        ),
    ] {
        let mut h = known();
        h.call(1, REPO, Op::Read(Read::Protection { branch: Box::from(*b"main") }));
        h.run(Duration::ZERO);
        if h.answered(1).is_none() {
            h.connected(FORGE);
            for (path, status, body) in responses {
                assert_eq!(h.request_lines(FORGE).as_slice().last().expect("request").as_ref(), *path, "{name}");
                h.answer(FORGE, &f::response(*status, body));
                h.run(Duration::ZERO);
            }
        }
        let answer = h.answered(1).expect("one protection terminal");
        assert_eq!((&answer.result, answer.cost), (&expected, cost), "{name}");
    }
}
#[test]
fn repository_account_selects_its_accepted_installation() {
    let mut h = known();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().recent = Some(2);
    begin(&mut h, branch());
    assert!(f::header(&h.request_bytes(FORGE), b"Authorization: ") == b"Bearer artificial-full-one");
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(&mut h, 1, Ok(Answer::Commit([0; 32])));
    assert_eq!(h.component.test_github().recent, Some(1));
}
#[test]
fn locate_selects_its_owner_login() {
    let mut h = known();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().recent = Some(2);
    h.call(1, api::Repository { repository: 0, ..REPO }, locate(b"acme"));
    h.run(Duration::ZERO);
    h.connected(FORGE);
    assert!(f::header(&h.request_bytes(FORGE), b"Authorization: ") == b"Bearer artificial-full-one");
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Ok(Answer::Located { repository: 1 }));
}
#[test]
fn rename_keeps_repository_account_routing() {
    let mut h = known();
    h.component.test_github_mut().rows.get_mut(&1).expect("row").account.login = Box::from(b"renamed".as_slice());
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(&mut h, 1, Ok(Answer::Commit([0; 32])));
}
#[test]
fn unaccepted_account_is_not_installed_without_io() {
    for op in [branch(), locate(b"acme")] {
        let mut h = known();
        h.component.test_github_mut().rows.get_mut(&1).expect("row").accepted = false;
        f::github_account(&mut h, 2, 23, b"stray", b"artificial-full-two");
        h.call(1, REPO, op);
        answered(&mut h, 0, Err(Error::NotInstalled));
        assert_eq!(h.connections_opened(), 0);
    }
}
#[test]
fn suspended_account_is_forbidden_without_io() {
    for op in [branch(), locate(b"acme")] {
        let mut h = known();
        h.component.test_github_mut().rows.get_mut(&1).expect("row").suspended = true;
        h.call(1, REPO, op);
        answered(&mut h, 0, Err(Error::Forbidden));
        assert_eq!(h.connections_opened(), 0);
    }
}
#[test]
fn missing_repository_succeeds_on_its_recent_installation() {
    let mut h = f::github_calls();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().recent = Some(2);
    begin(&mut h, branch());
    assert!(f::header(&h.request_bytes(FORGE), b"Authorization: ") == b"Bearer artificial-full-two");
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    h.run(Duration::ZERO);
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(&mut h, 2, Ok(Answer::Commit([0; 32])));
    let bytes = h.all_request_bytes(FORGE);
    assert!(super::github_start::contains(&bytes, b"Bearer artificial-full-one\r\n"));
    assert_eq!(h.request_lines(FORGE).len(), 2);
}
#[test]
fn discovery_404_searches_the_next_candidate() {
    let mut h = f::github_calls();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    h.run(Duration::ZERO);
    h.connected(FORGE);
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 2, Err(Error::Missing));
    assert_eq!(h.connections_for(FORGE), 0);
}
#[test]
fn discovered_unaccepted_owner_keeps_discovery_cost() {
    let mut h = f::github_calls();
    h.component.test_github_mut().rows.get_mut(&1).expect("row").account.id = 23;
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Err(Error::NotInstalled));
    assert_eq!(h.request_lines(FORGE).len(), 1);
}
#[test]
fn discovered_settings_returns_without_duplicate_read() {
    let mut h = f::github_calls();
    begin(&mut h, Op::Read(Read::Settings));
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(
        &mut h,
        1,
        Ok(Answer::Settings(api::Settings {
            default_branch: Box::from(b"main".as_slice()),
            merge: true,
            squash: true,
            rebase: false,
        })),
    );
    assert_eq!(h.request_lines(FORGE).len(), 1);
}
#[test]
fn expired_full_waits_without_holding_a_call_socket() {
    let mut h = known();
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::ZERO;
    h.call(1, REPO, branch());
    h.run(Duration::ZERO);
    assert_eq!(h.connections_opened(), 0);
    assert!(h.answered(1).is_none());
    let row = h.component.test_github_mut().rows.get_mut(&1).expect("row");
    row.api = Some(Secret::new(Box::from(b"renewed-full".as_slice())));
    row.full = Clock { expiry: Time::from_nanos(3_600_000_000_000), ..Clock::empty() };
    h.run(Duration::ZERO);
    h.connected(FORGE);
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(&mut h, 1, Ok(Answer::Commit([0; 32])));
}
#[test]
fn token_wait_ends_at_original_deadline_without_cost() {
    let mut h = known();
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::ZERO;
    h.call(1, REPO, branch());
    h.run(h.env.limits.deadline);
    answered(&mut h, 0, Err(Error::Unavailable));
    assert_eq!(h.connections_opened(), 0);
}
#[test]
fn still_valid_full_serves_during_contents_or_full_renewal() {
    for scope in [skein_forge::github::request::Scope::Full, skein_forge::github::request::Scope::Contents] {
        let mut h = known();
        h.component.test_github_mut().rows.get_mut(&1).expect("row").state = Tokens::Minting { scope };
        begin(&mut h, branch());
        h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
        answered(&mut h, 1, Ok(Answer::Commit([0; 32])));
    }
}
#[test]
fn call_401_is_forbidden_and_mints_once() {
    let mut h = known();
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(401));
    answered(&mut h, 1, Err(Error::Forbidden));
    let row = h.component.test_github().rows.get(&1).expect("row");
    assert_eq!(row.refused_revision, Some(1));
    h.run(Duration::ZERO);
    assert_eq!(h.component.test_github().mint, Some((1, skein_forge::github::request::Scope::Full)));
    assert_eq!(skein_lib::bytes::count(&h.all_request_bytes(FORGE), b"GET /repos/", u32::MAX), 1);
}
#[test]
fn call_403_does_not_mint() {
    let mut h = known();
    begin(&mut h, branch());
    h.answer(FORGE, &f::response(403, br#"{"message":"Forbidden"}"#));
    answered(&mut h, 1, Err(Error::Forbidden));
    assert_eq!(h.component.test_github().mint, None);
}
#[test]
fn late_401_cannot_renew_a_newer_full_revision() {
    let mut h = known();
    begin(&mut h, branch());
    h.component.test_github_mut().rows.get_mut(&1).expect("row").token_revision = 2;
    h.answer(FORGE, &f::status(401));
    answered(&mut h, 1, Err(Error::Forbidden));
    assert_eq!(h.component.test_github().mint, None);
}
#[test]
fn github_call_keeps_one_deadline_and_first_byte_cost() {
    let mut h = f::github_calls();
    begin(&mut h, branch());
    h.run(Duration::from_secs(9));
    h.answer(FORGE, &f::response_with_fields(200, f::GITHUB_REPOSITORY, b"Connection: close\r\n"));
    h.run(Duration::ZERO);
    h.connected(FORGE);
    h.run(Duration::from_secs(1));
    answered(&mut h, 2, Err(Error::Timeout));
}
#[test]
fn unseen_connect_failure_costs_zero_and_seen_reset_costs_one() {
    let mut h = known();
    h.call(1, REPO, branch());
    h.run(Duration::ZERO);
    h.refuse(FORGE);
    answered(&mut h, 0, Err(Error::Unavailable));
    let mut h = known();
    begin(&mut h, branch());
    h.reset(FORGE);
    answered(&mut h, 1, Err(Error::Timeout));
}
fn merge() -> Op {
    Op::Write(Write::Merge { number: 7, head: [0; 32], how: api::MergeHow::Style })
}
fn reviewers() -> Op {
    Op::Write(Write::SetReviewers { number: 7, reviewers: Box::from([]) })
}
const QUEUE_REFUSAL: &[u8] =
    br#"{"message":"Repository rule violations found\n\nChanges must be made through the merge queue\n\n"}"#;
fn enqueue(h: &mut Harness) {
    begin(h, merge());
    h.answer(FORGE, &f::response(405, QUEUE_REFUSAL));
    h.answer(FORGE, &f::ok(f::GITHUB_GRAPHQL));
}
#[test]
fn merge_without_a_method_keeps_the_dialect_refusal() {
    let mut h = known();
    let mut row = h.component.github_tables().repository(REPO).expect("row").clone();
    row.merge = None;
    h.component.github_tables().keep_repository(REPO, row);
    begin(&mut h, merge());
    answered(&mut h, 0, Err(Error::Refused));
    assert!(h.request_lines(FORGE).is_empty());
}
#[test]
fn merge_and_reviewers_require_the_repository_installation() {
    for op in [merge(), reviewers()] {
        let mut h = known();
        h.component.test_github_mut().rows.get_mut(&1).expect("row").accepted = false;
        h.call(1, REPO, op);
        answered(&mut h, 0, Err(Error::NotInstalled));
        assert_eq!(h.connections_opened(), 0);
    }
}
#[test]
fn sent_enqueue_is_never_replayed_after_loss_or_deadline() {
    for reset in [true, false] {
        let mut h = known();
        enqueue(&mut h);
        if reset {
            h.reset(FORGE);
        } else {
            h.run(h.env.limits.deadline);
        }
        answered(&mut h, 3, Err(Error::Timeout));
        h.call(2, REPO, branch());
        h.run(Duration::ZERO);
        h.connected(FORGE);
        h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
        h.run(Duration::ZERO);
        assert_eq!(h.answered(2).expect("fresh call").result, Ok(Answer::Commit([0; 32])));
        assert_eq!(skein_lib::bytes::count(&h.all_request_bytes(FORGE), b"enqueuePullRequest", u32::MAX), 1);
    }
}
#[test]
fn merge_401_is_forbidden_and_renews_full_without_replay() {
    let mut h = known();
    begin(&mut h, merge());
    h.answer(FORGE, &f::status(401));
    answered(&mut h, 1, Err(Error::Forbidden));
    h.run(Duration::ZERO);
    assert_eq!(h.component.test_github().rows.get(&1).expect("row").refused_revision, Some(1));
    assert_eq!(h.component.test_github().mint, Some((1, skein_forge::github::request::Scope::Full)));
    assert_eq!(skein_lib::bytes::count(&h.all_request_bytes(FORGE), b"PUT /repos/", u32::MAX), 1);
}
#[test]
fn queue_continuation_renews_only_its_selected_installations_token() {
    let mut h = known();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    begin(&mut h, merge());
    h.answer(FORGE, &f::response(405, QUEUE_REFUSAL));
    h.component.test_github_mut().recent = Some(2);
    let row = h.component.test_github_mut().rows.get_mut(&1).expect("selected installation");
    row.api = Some(Secret::new(Box::from(b"renewed-full-one".as_slice())));
    row.token_revision = 2;
    h.answer(FORGE, &f::ok(f::GITHUB_GRAPHQL));
    h.answer(FORGE, &f::ok(br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":1}}}}"#));
    answered(&mut h, 3, Ok(Answer::Queued));
    let requests = h.all_request_bytes(FORGE);
    assert_eq!(skein_lib::bytes::count(&requests, b"Bearer artificial-full-one\r\n", u32::MAX), 2);
    assert_eq!(skein_lib::bytes::count(&requests, b"Bearer renewed-full-one\r\n", u32::MAX), 1);
    assert_eq!(skein_lib::bytes::count(&requests, b"Bearer artificial-full-two\r\n", u32::MAX), 0);
}
#[test]
fn revoked_queue_route_never_enqueues_under_another_installation() {
    let mut h = known();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    begin(&mut h, merge());
    h.answer(FORGE, &f::response(405, QUEUE_REFUSAL));
    h.component.test_github_mut().rows.get_mut(&1).expect("selected installation").forget();
    h.component.test_github_mut().recent = Some(2);
    h.answer(FORGE, &f::ok(f::GITHUB_GRAPHQL));
    answered(&mut h, 2, Err(Error::Forbidden));
    assert_eq!(skein_lib::bytes::count(&h.all_request_bytes(FORGE), b"enqueuePullRequest", u32::MAX), 0);
}
#[test]
fn loss_and_reacceptance_discard_discovery_and_its_staged_row() {
    let mut h = f::github_calls();
    begin(&mut h, branch());
    h.component.test_github_mut().rows.get_mut(&1).expect("row").revision = 2;
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Err(Error::Forbidden));
    assert!(h.component.github_tables().repository(REPO).is_none());
}
#[test]
fn revoked_multistep_continuation_is_forbidden() {
    let mut h = known();
    begin(&mut h, Op::Read(Read::Branches { prefix: Box::from(&b"m"[..]) }));
    h.component.test_github_mut().rows.get_mut(&1).expect("row").forget();
    h.answer(
        FORGE,
        &f::response_with_fields(
            200,
            br#"[{"name":"main"}]"#,
            b"Link: <https://api.github.com/x?page=2>; rel=\"next\"\r\n",
        ),
    );
    answered(&mut h, 1, Err(Error::Forbidden));
    assert_eq!(h.request_lines(FORGE).len(), 1);
}
#[test]
fn close_during_discovery_or_token_wait_has_no_terminal() {
    for waiting in [false, true] {
        let mut h = f::github_calls();
        if waiting {
            h.component.test_github_mut().identity_checked = false;
        }
        h.call(1, REPO, branch());
        h.run(Duration::ZERO);
        if !waiting {
            h.connected(FORGE);
        }
        h.send(Request::Close);
        h.run(Duration::ZERO);
        assert!(h.closed);
        assert!(h.answered(1).is_none());
    }
}
#[test]
fn held_answer_is_offered_once() {
    let mut h = known();
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    h.run(Duration::ZERO);
    let answer = h.answers.remove(&1).expect("answer");
    h.send(Request::Refused(answer));
    h.send(Request::Offer);
    h.run(Duration::ZERO);
    assert_eq!(h.answered(1).expect("offered").cost, 1);
    h.send(Request::Offer);
    h.run(Duration::ZERO);
    h.send(Request::Close);
    h.run(Duration::ZERO);
    assert!(h.closed);
}
#[test]
fn absent_dialect_tables_keep_nothing_and_present_tables_keep_forge_keys() {
    let mut h = known();
    assert!(h.component.forgejo_tables().repository(REPO).is_none());
    let row = h.component.github_tables().repository(REPO).expect("row").clone();
    h.component.github_tables().keep_repository(f::OTHER_REPO, row);
    assert!(h.component.github_tables().repository(REPO).is_some());
    assert!(h.component.github_tables().repository(f::OTHER_REPO).is_some());
    let mut h = f::known();
    f::github_repository(&mut h, REPO, 22);
    assert!(h.component.github_tables().repository(REPO).is_none());
}

#[test]
fn every_additional_read_runs_through_the_component() {
    for (label, op, bodies, expected) in [
        (
            "Checks",
            Read::Checks { commit: [0; 32] },
            &[f::GITHUB_COMBINED, f::GITHUB_RUNS][..],
            Answer::Checks(Box::from([])),
        ),
        (
            "Permission",
            Read::Permission { user: 31 },
            &[br#"{"role_name":"write","user":{"id":31,"login":"alice"}}"#.as_slice()][..],
            Answer::Permission(api::Permission::Write),
        ),
        (
            "Collaborators",
            Read::Collaborators { page: 1 },
            &[b"[]".as_slice()][..],
            Answer::Collaborators { collaborators: Box::from([]), more: false },
        ),
        (
            "Remarks",
            Read::Remarks { number: 7, review: 8, page: 1 },
            &[b"[]".as_slice()][..],
            Answer::Remarks { remarks: Box::from([]), more: false },
        ),
        (
            "Reviews",
            Read::Reviews { number: 7, page: 1 },
            &[b"[]".as_slice()][..],
            Answer::Reviews { reviews: Box::from([]), more: false },
        ),
    ] {
        let mut h = known();
        h.component.github_tables().keep_user(FORGE, 31, Box::from(b"alice".as_slice()));
        begin(&mut h, Op::Read(op));
        for body in bodies {
            h.answer(FORGE, &f::ok(body));
        }
        h.run(Duration::ZERO);
        let answer = h.answered(1).expect(label);
        assert_eq!(answer.result, Ok(expected), "{label}");
        assert_eq!(answer.cost, u32::try_from(bodies.len()).expect("fixture count"), "{label}");
    }
}
#[test]
fn every_landed_write_runs_once_with_full_installation_authorization() {
    for (label, write, code, body, expected) in [
        (
            "SetReviewers",
            Write::SetReviewers { number: 7, reviewers: Box::from([]) },
            200,
            f::GITHUB_PULL,
            Answer::Done,
        ),
        (
            "Merge",
            Write::Merge { number: 7, head: [0; 32], how: api::MergeHow::Style },
            200,
            br#"{"sha":"0000000000000000000000000000000000000000"}"#,
            Answer::Merged([0; 32]),
        ),
        (
            "DeleteBranch",
            Write::DeleteBranch { branch: Box::from(b"topic".as_slice()) },
            204,
            b"".as_slice(),
            Answer::Done,
        ),
        ("Close", Write::Close { number: 7 }, 200, b"{}", Answer::Done),
        ("Reopen", Write::Reopen { number: 7 }, 200, b"{}", Answer::Done),
        (
            "Edit",
            Write::Edit { number: 7, title: Some(Box::from(b"title".as_slice())), body: None },
            200,
            b"{}",
            Answer::Done,
        ),
        ("Status", status_write(), 201, b"{}", Answer::Done),
        (
            "CreateBranch",
            Write::CreateBranch { branch: Box::from(b"topic".as_slice()), commit: [0; 32] },
            201,
            b"{}",
            Answer::Branch(api::BranchCreation::Created),
        ),
        (
            "OpenPull",
            Write::OpenPull {
                title: Box::from(b"title".as_slice()),
                body: Box::from(b"body".as_slice()),
                head: Box::from(b"topic".as_slice()),
                base: Box::from(b"main".as_slice()),
            },
            201,
            br#"{"number":7}"#,
            Answer::Created(7),
        ),
        (
            "CreateIssue",
            Write::CreateIssue {
                key: Box::from([1]),
                title: Box::from(b"title".as_slice()),
                body: Box::from(b"body".as_slice()),
            },
            201,
            br#"{"number":7}"#,
            Answer::Created(7),
        ),
        (
            "Post",
            Write::Post { number: 7, key: Box::from([1]), body: Box::from(b"body".as_slice()) },
            201,
            br#"{"id":8}"#,
            Answer::Commented(8),
        ),
        (
            "Review",
            Write::Review {
                number: 7,
                key: Box::from([1]),
                verdict: api::Verdict::Approve,
                body: Box::from(b"body".as_slice()),
            },
            200,
            br#"{"id":8}"#,
            Answer::Reviewed(8),
        ),
    ] {
        let mut h = known();
        begin(&mut h, Op::Write(write));
        h.answer(FORGE, &f::response(code, body));
        h.run(Duration::ZERO);
        let answer = h.answered(1).expect(label);
        assert_eq!(answer.result, Ok(expected), "{label}");
        assert_eq!(answer.cost, 1, "{label}");
        assert!(super::github_start::contains(
            &h.all_request_bytes(FORGE),
            b"Authorization: Bearer artificial-full-one\r\n"
        ));
    }
}
#[test]
fn original_reads_keep_the_dialects_typed_answers() {
    for (label, op, bodies) in [
        ("Items", Read::Items { since: Time::ZERO, page: 1, kind: None }, &[b"[]".as_slice()][..]),
        ("Item", Read::Item { number: 7, after: 0 }, &[f::GITHUB_ISSUE, f::GITHUB_COMMENTS][..]),
        (
            "Pull",
            Read::Pull { number: 7 },
            &[f::GITHUB_PULL, f::GITHUB_COMBINED, f::GITHUB_RUNS, f::GITHUB_GRAPHQL][..],
        ),
        ("Statuses", Read::Statuses { commit: [0; 32], page: 1 }, &[f::GITHUB_COMBINED][..]),
        ("Branches", Read::Branches { prefix: Box::from(&b"m"[..]) }, &[br#"[{"name":"main"}]"#.as_slice()][..]),
    ] {
        let mut h = known();
        begin(&mut h, Op::Read(op));
        for body in bodies {
            h.answer(FORGE, &f::ok(body));
        }
        h.run(Duration::ZERO);
        let answer = h.answered(1).expect(label);
        assert!(answer.result.is_ok(), "{label}: {:?}", answer.result);
        assert_eq!(answer.cost, u32::try_from(bodies.len()).expect("fixture count"), "{label}");
    }
}

#[test]
fn update_reads_its_head_then_sends_one_pinned_write() {
    let mut h = known();
    begin(&mut h, Op::Write(Write::Update { number: 7, style: api::UpdateStyle::Merge }));
    h.answer(FORGE, &f::ok(f::GITHUB_PULL));
    h.answer(FORGE, &f::response(202, b"{}"));
    answered(&mut h, 2, Ok(Answer::Done));
}
#[test]
fn pull_for_checks_both_lists_before_missing() {
    let mut h = known();
    begin(
        &mut h,
        Op::Read(Read::PullFor { head: Box::from(b"feature".as_slice()), base: Box::from(b"main".as_slice()) }),
    );
    h.answer(FORGE, &f::ok(b"[]"));
    h.answer(FORGE, &f::ok(b"[]"));
    answered(&mut h, 2, Err(Error::Missing));
}

#[test]
fn discovered_settings_waits_for_its_owner_without_a_second_read() {
    let mut h = f::github_calls();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::ZERO;
    h.component.test_github_mut().recent = Some(2);
    begin(&mut h, Op::Read(Read::Settings));
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    h.run(Duration::ZERO);
    assert!(h.answered(1).is_none());
    assert_eq!(h.connections_opened(), 1);
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::from_nanos(3_600_000_000_000);
    h.component.test_github_mut().rows.get_mut(&1).expect("row").api =
        Some(Secret::new(Box::from(b"renewed".as_slice())));
    answered(
        &mut h,
        1,
        Ok(Answer::Settings(api::Settings {
            default_branch: Box::from(b"main".as_slice()),
            merge: true,
            squash: true,
            rebase: false,
        })),
    );
    assert_eq!(h.connections_opened(), 1);
}
#[test]
fn another_installation_proceeds_while_one_full_is_expired() {
    let mut h = known();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    let other = api::Repository { repository: 2, ..REPO };
    f::github_repository(&mut h, other, 23);
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::ZERO;
    h.call(1, REPO, branch());
    h.call(2, other, branch());
    h.run(Duration::ZERO);
    h.connected(FORGE);
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    h.run(Duration::ZERO);
    assert!(h.answered(1).is_none());
    let answer = h.answered(2).expect("other installation answered");
    assert_eq!((answer.cost, &answer.result), (1, &Ok(Answer::Commit([0; 32]))));
    assert_eq!(h.connections_opened(), 1);
}
#[test]
fn multiple_401s_share_one_full_revision_renewal() {
    let mut h = known();
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(401));
    h.call(2, REPO, branch());
    h.run(Duration::ZERO);
    h.connected(FORGE);
    h.answer(FORGE, &f::status(401));
    h.run(Duration::ZERO);
    assert_eq!(h.answered(2).expect("second terminal").result, Err(Error::Forbidden));
    assert_eq!(h.component.test_github().rows.get(&1).expect("row").refused_revision, Some(1));
    assert_eq!(h.component.test_github().mint, Some((1, skein_forge::github::request::Scope::Full)));
    assert_eq!(h.connections_for(FORGE), 1);
}
#[test]
fn replacement_discards_discovery_without_a_table_fill() {
    let mut h = f::github_calls();
    begin(&mut h, branch());
    h.component.test_github_mut().forget();
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Err(Error::Forbidden));
    assert!(h.component.github_tables().repository(REPO).is_none());
}
#[test]
fn a_written_single_step_can_finish_after_eligibility_loss() {
    let mut h = known();
    begin(&mut h, branch());
    h.component.test_github_mut().rows.get_mut(&1).expect("row").forget();
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(&mut h, 1, Ok(Answer::Commit([0; 32])));
}
#[test]
fn a_401_head_discards_unread_body_before_next_call() {
    let mut h = known();
    begin(&mut h, Op::Write(Write::Close { number: 7 }));
    h.answer(FORGE, b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 1000\r\n\r\n");
    answered(&mut h, 1, Err(Error::Forbidden));
    assert_eq!(h.connections_for(FORGE), 1); // Only the independent renewal socket remains.
}

#[test]
fn calls_without_a_repository_use_the_most_recent_accepted_row() {
    let mut h = f::github_calls();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().recent = Some(2);
    h.call(1, api::Repository { repository: 0, ..REPO }, Op::Read(Read::Settings));
    h.run(Duration::ZERO);
    h.connected(FORGE);
    assert!(f::header(&h.request_bytes(FORGE), b"Authorization: ") == b"Bearer artificial-full-two");
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Missing));
}
#[test]
fn a_gone_recent_row_falls_back_in_installation_id_order() {
    let mut h = f::github_calls();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().rows.get_mut(&2).expect("row").forget();
    h.component.test_github_mut().recent = Some(2);
    h.call(1, api::Repository { repository: 0, ..REPO }, Op::Read(Read::Settings));
    h.run(Duration::ZERO);
    h.connected(FORGE);
    assert!(f::header(&h.request_bytes(FORGE), b"Authorization: ") == b"Bearer artificial-full-one");
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Missing));
}
#[test]
fn mixed_dialects_keep_independent_repository_and_user_rows() {
    let mut settings = f::github_settings();
    let mut forgejo = f::settings().forges.into_boxed().into_iter().next().expect("forge");
    forgejo.number = f::OTHER_FORGE;
    settings.forges.push(forgejo).expect("two forges");
    let mut component = crate::Component::new(settings, &f::limits(), &f::shared()).expect("mixed settings");
    component.forgejo_tables().keep_repository(REPO, f::specimen());
    component.forgejo_tables().keep_user(FORGE, 9, Box::from(b"forgejo".as_slice()));
    let mut h = known();
    let row = h.component.github_tables().repository(REPO).expect("GitHub row").clone();
    component.github_tables().keep_repository(REPO, row.clone());
    component.github_tables().keep_repository(f::OTHER_REPO, row);
    component.github_tables().keep_user(FORGE, 9, Box::from(b"github".as_slice()));
    assert_eq!(component.forgejo_tables().repository(REPO), Some(&f::specimen()));
    assert_eq!(component.github_tables().repository(REPO).expect("row").owner_id, 22);
    assert_eq!(component.github_tables().repository(f::OTHER_REPO).expect("forge key").owner_id, 22);
    assert_eq!(component.forgejo_tables().user(FORGE, 9), Some(b"forgejo".as_slice()));
    assert_eq!(component.github_tables().user(FORGE, 9), Some(b"github".as_slice()));
}
#[test]
fn close_during_a_normal_multistep_call_has_no_terminal() {
    let mut h = known();
    begin(&mut h, Op::Read(Read::Item { number: 7, after: 0 }));
    h.answer(FORGE, &f::ok(f::GITHUB_ISSUE));
    h.send(Request::Close);
    h.run(Duration::ZERO);
    assert!(h.closed);
    assert!(h.answered(1).is_none());
}
#[test]
#[expect(clippy::disallowed_macros, reason = "inspect Secret redaction in an ordinary call's state")]
fn ordinary_calls_keep_full_values_out_of_debug_and_request_lines() {
    let mut h = known();
    begin(&mut h, branch());
    let debug = std::format!("{:?} {:?}", h.component, h.up);
    assert!(!super::github_start::contains(debug.as_bytes(), b"artificial-full-one"));
    for line in h.request_lines(FORGE).into_boxed() {
        assert!(!super::github_start::contains(&line, b"artificial-full-one"));
    }
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(&mut h, 1, Ok(Answer::Commit([0; 32])));
}

#[test]
fn bot_permission_uses_its_installation_levels_without_io() {
    use skein_forge::github::document::{Level, Levels};
    for (contents, pull_requests, expected) in [
        (Level::Write, Level::Write, api::Permission::Write),
        (Level::Read, Level::Write, api::Permission::Read),
        (Level::Write, Level::Read, api::Permission::Read),
        (Level::Write, Level::None, api::Permission::Read),
        (Level::Read, Level::None, api::Permission::Read),
        (Level::Read, Level::Read, api::Permission::Read),
        (Level::None, Level::Write, api::Permission::None),
        (Level::None, Level::Read, api::Permission::None),
        (Level::None, Level::None, api::Permission::None),
    ] {
        let mut h = known();
        f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
        h.component.test_github_mut().recent = Some(2);
        h.component.test_github_mut().rows.get_mut(&1).expect("owner installation").permissions =
            Levels { contents, pull_requests };
        h.call(1, REPO, Op::Read(Read::Permission { user: 9 }));
        answered(&mut h, 0, Ok(Answer::Permission(expected)));
        assert_eq!(h.connections_opened(), 0);
    }
}
#[test]
fn bot_permission_discovers_repository_before_using_installation_levels() {
    let mut h = f::github_calls();
    begin(&mut h, Op::Read(Read::Permission { user: 9 }));
    assert_eq!(
        h.request_lines(FORGE).as_slice().last().expect("repository read").as_ref(),
        b"GET /repositories/1 HTTP/1.1"
    );
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Missing));
    assert_eq!(h.connections_for(FORGE), 0);
}

#[test]
fn bot_permission_after_repository_discovery_sends_no_collaborator_request() {
    let mut h = f::github_calls();
    begin(&mut h, Op::Read(Read::Permission { user: 9 }));
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Ok(Answer::Permission(api::Permission::None)));
    assert_eq!(h.request_lines(FORGE).len(), 1);
}

fn status_write() -> Write {
    Write::Status {
        commit: [0; 32],
        context: Box::from(b"ci".as_slice()),
        check: api::Check::Passed,
        description: Box::new([]),
        link: Box::new([]),
    }
}
