//! Write plans through the component's scripted byte peer.
use super::fixtures::{self, Harness};
use alloc::boxed::Box;
use skein_forge::api::{Answer, BranchCreation, Check, Error, Op, Verdict, Write};
use skein_forge::forgejo::failure::Refusal;
use skein_forge::forgejo::plans::failure::{self, Asked, Classified, Drop};
use skein_lib::{Duration, bytes};
const ISSUE: &[u8] = br#"{"number":7,"state":"open","user":{"id":2,"login":"writer"},"labels":[],"updated_at":"2020-01-01T00:00:00Z","title":"title","body":"body"}"#;
const COMMENT: &[u8] = br#"{"id":9,"user":{"id":2,"login":"writer"},"body":"body"}"#;
const REVIEW: &[u8] = br#"{"id":11,"user":{"id":2,"login":"writer"},"state":"APPROVED","commit_id":"0100000000000000000000000000000000000000","body":"body","official":true,"dismissed":false}"#;
const STATUS: &[u8] =
    br#"{"creator":null,"status":"success","context":"gate","description":"","target_url":"","updated_at":"2020-01-01T00:00:00Z"}"#;
const BRANCH: &[u8] = br#"{"name":"work","commit":{"id":"0100000000000000000000000000000000000000"},"effective_branch_protection_name":""}"#;
fn boxed(bytes: &[u8]) -> Box<[u8]> {
    Box::from(bytes)
}
fn head() -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[0] = 1;
    bytes
}
fn creation_rows() -> [CreationRow; 3] {
    [
        (
            "issue",
            Write::CreateIssue {
                key: boxed(&[0xab, 0xff]),
                title: boxed(b"title"),
                body: boxed(b"body"),
                labels: Box::new([]),
            },
            b"POST /api/v1/repos/fixture/specimen/issues ".as_slice(),
            201,
            ISSUE,
            Answer::Created(7),
        ),
        (
            "comment",
            Write::Post { number: 7, key: boxed(&[0xab, 0xff]), body: boxed(b"body") },
            b"POST /api/v1/repos/fixture/specimen/issues/7/comments ".as_slice(),
            201,
            COMMENT,
            Answer::Commented(9),
        ),
        (
            "review",
            Write::Review { number: 7, key: boxed(&[0xab, 0xff]), verdict: Verdict::Approve, body: boxed(b"body") },
            b"POST /api/v1/repos/fixture/specimen/pulls/7/reviews ".as_slice(),
            200,
            REVIEW,
            Answer::Reviewed(11),
        ),
    ]
}
#[test]
fn creations_carry_the_key_marker_at_the_head_of_their_body() {
    for (name, write, request, code, body, expected) in creation_rows() {
        let mut h = fixtures::known();
        h.call(1, fixtures::REPO, Op::Write(write));
        h.run(Duration::from_millis(1));
        let written = h.request_bytes(fixtures::FORGE);
        assert!(bytes::find(&written, request).is_some(), "{name}: route");
        assert!(bytes::find(&written, br#""body":"<!-- kit:key abff -->\nbody""#).is_some(), "{name}: marked body");
        h.answer(fixtures::FORGE, &fixtures::response(code, body));
        h.run(Duration::ZERO);
        let answered = h.answered(1).expect("creation answered");
        assert_eq!((answered.cost, &answered.result), (1, &Ok(expected)), "{name}");
    }
}
fn write_rows() -> [WriteRow; 7] {
    [
        (
            "edit",
            Write::Edit { number: 7, title: Some(boxed(b"changed")), body: None },
            b"PATCH /api/v1/repos/fixture/specimen/issues/7 ".as_slice(),
            br#"{"title":"changed"}"#.as_slice(),
            201,
            ISSUE,
            Answer::Done,
        ),
        (
            "close",
            Write::Close { number: 7 },
            b"PATCH /api/v1/repos/fixture/specimen/issues/7 ".as_slice(),
            br#"{"state":"closed"}"#.as_slice(),
            201,
            ISSUE,
            Answer::Done,
        ),
        (
            "reopen",
            Write::Reopen { number: 7 },
            b"PATCH /api/v1/repos/fixture/specimen/issues/7 ".as_slice(),
            br#"{"state":"open"}"#.as_slice(),
            201,
            ISSUE,
            Answer::Done,
        ),
        (
            "update",
            Write::Update { number: 7, style: skein_forge::api::UpdateStyle::Merge },
            b"POST /api/v1/repos/fixture/specimen/pulls/7/update?style=merge ".as_slice(),
            b"\r\n\r\n".as_slice(),
            200,
            b"".as_slice(),
            Answer::Done,
        ),
        (
            "status",
            Write::Status {
                commit: head(),
                context: boxed(b"gate"),
                check: Check::Passed,
                description: Box::new([]),
                link: Box::new([]),
            },
            b"POST /api/v1/repos/fixture/specimen/statuses/0100000000000000000000000000000000000000 ".as_slice(),
            br#"{"state":"success","context":"gate"}"#.as_slice(),
            201,
            STATUS,
            Answer::Done,
        ),
        (
            "branch",
            Write::CreateBranch { branch: boxed(b"work/a"), commit: head() },
            b"POST /api/v1/repos/fixture/specimen/branches ".as_slice(),
            br#"{"new_branch_name":"work/a","old_ref_name":"0100000000000000000000000000000000000000"}"#.as_slice(),
            201,
            BRANCH,
            Answer::Branch(BranchCreation::Created),
        ),
        (
            "delete",
            Write::DeleteBranch { branch: boxed(b"work/a") },
            b"DELETE /api/v1/repos/fixture/specimen/branches/work%2Fa ".as_slice(),
            b"\r\n\r\n".as_slice(),
            204,
            b"".as_slice(),
            Answer::Done,
        ),
    ]
}
#[test]
fn straightforward_writes_use_their_methods_and_literal_bodies() {
    for (name, write, request, json, code, body, expected) in write_rows() {
        let mut h = fixtures::known();
        h.call(1, fixtures::REPO, Op::Write(write));
        h.run(Duration::from_millis(1));
        let written = h.request_bytes(fixtures::FORGE);
        assert!(bytes::find(&written, request).is_some(), "{name}: route");
        assert!(bytes::find(&written, json).is_some(), "{name}: body");
        h.answer(fixtures::FORGE, &fixtures::response(code, body));
        h.run(Duration::ZERO);
        let answered = h.answered(1).expect("write answered");
        assert_eq!((answered.cost, &answered.result), (1, &Ok(expected)), "{name}");
    }
}
#[test]
fn text_that_is_not_utf8_is_refused_with_nothing_written() {
    let rows = [
        (
            "title",
            Write::CreateIssue {
                key: boxed(b"key"),
                title: boxed(&[0xff]),
                body: boxed(b"body"),
                labels: Box::new([]),
            },
        ),
        ("comment", Write::Post { number: 7, key: boxed(b"key"), body: boxed(&[0xff]) }),
        ("review", Write::Review { number: 7, key: boxed(b"key"), verdict: Verdict::Comment, body: boxed(&[0xff]) }),
        ("edit", Write::Edit { number: 7, title: None, body: Some(boxed(&[0xff])) }),
        (
            "context",
            Write::Status {
                commit: head(),
                context: boxed(&[0xff]),
                check: Check::Pending,
                description: Box::new([]),
                link: Box::new([]),
            },
        ),
        ("branch", Write::CreateBranch { branch: boxed(&[0xff]), commit: head() }),
    ];
    for (name, write) in rows {
        let mut h = Harness::ready();
        let before = h.request_bytes(fixtures::FORGE);
        h.call(1, fixtures::REPO, Op::Write(write));
        h.run(Duration::from_millis(1));
        let answered = h.answered(1).expect("refused before resolving");
        assert_eq!((answered.cost, &answered.result), (0, &Err(Error::Refused)), "{name}");
        assert_eq!(h.request_bytes(fixtures::FORGE), before, "{name}: no request");
    }
}
fn failure_rows() -> [FailureRow; 25] {
    [
        ("redirect", Asked::Read, 301, None, None, Error::Unavailable, Drop::Path),
        ("missing", Asked::Read, 404, None, None, Error::Missing, Drop::Path),
        ("missing job", Asked::Job, 404, None, None, Error::MissingJob, Drop::Path),
        (
            "rate",
            Asked::Read,
            429,
            None,
            Some(Duration::from_secs(5)),
            Error::RateLimited { after: Duration::from_secs(5) },
            Drop::Nothing,
        ),
        (
            "rate without header",
            Asked::Write,
            429,
            None,
            None,
            Error::RateLimited { after: Duration::ZERO },
            Drop::Nothing,
        ),
        ("quota", Asked::Write, 413, None, None, Error::Full, Drop::Nothing),
        ("archived", Asked::Write, 423, None, None, Error::Forbidden, Drop::Nothing),
        ("style", Asked::Merge, 405, Some(Refusal::MergeStyle), None, Error::Refused, Drop::Row),
        ("old head", Asked::Merge, 409, Some(Refusal::HeadOutOfDate), None, Error::Stale, Drop::Nothing),
        ("delete protection", Asked::DeleteBranch, 403, None, None, Error::Protected, Drop::Nothing),
        ("unknown merge", Asked::Merge, 405, None, None, Error::Refused, Drop::Nothing),
        ("unknown read", Asked::Read, 418, None, None, Error::InvalidAnswer, Drop::Nothing),
        ("before POST", Asked::BeforeWrite, 500, None, None, Error::Unavailable, Drop::Nothing),
        ("unknown before POST", Asked::BeforeWrite, 418, None, None, Error::Unavailable, Drop::Nothing),
        ("already open", Asked::OpenPull, 409, None, None, Error::Exists, Drop::Nothing),
        ("closed", Asked::Merge, 405, Some(Refusal::MergeClosed), None, Error::Closed, Drop::Nothing),
        ("merge conflict", Asked::Merge, 405, Some(Refusal::MergeConflict), None, Error::Conflict, Drop::Nothing),
        ("merge protection", Asked::Merge, 405, Some(Refusal::MergeProtected), None, Error::Protected, Drop::Nothing),
        ("update conflict", Asked::Update, 409, Some(Refusal::UpdateConflict), None, Error::Conflict, Drop::Nothing),
        ("empty", Asked::Write, 422, Some(Refusal::Empty), None, Error::Empty, Drop::Nothing),
        ("unknown 422", Asked::OpenPull, 422, None, None, Error::Refused, Drop::Nothing),
        ("token", Asked::Write, 401, None, None, Error::Forbidden, Drop::Nothing),
        ("permission", Asked::Write, 403, None, None, Error::Forbidden, Drop::Nothing),
        ("5xx", Asked::Write, 500, None, None, Error::Timeout, Drop::Nothing),
        ("unexpected success", Asked::Write, 202, None, None, Error::Timeout, Drop::Nothing),
    ]
}
#[test]
fn failures_classify_by_status_call_and_message() {
    for (name, asked, status, refusal, retry, error, drop) in failure_rows() {
        assert_eq!(failure::classify(asked, status, refusal, retry), Classified { error, drop }, "{name}");
    }
}
#[test]
fn a_branch_already_there_answers_exists_without_retry() {
    let mut h = fixtures::known();
    h.call(1, fixtures::REPO, Op::Write(Write::CreateBranch { branch: boxed(b"work"), commit: head() }));
    h.answer(fixtures::FORGE, &fixtures::response(409, br#"{"message":"branch already exists"}"#));
    h.run(Duration::ZERO);
    let answered = h.answered(1).expect("branch answered");
    assert_eq!((answered.cost, &answered.result), (1, &Ok(Answer::Branch(BranchCreation::Exists))));
}
#[test]
fn a_malformed_write_answer_is_timeout() {
    let mut h = fixtures::known();
    h.call(
        1,
        fixtures::REPO,
        Op::Write(Write::CreateIssue {
            key: boxed(b"key"),
            title: boxed(b"title"),
            body: boxed(b"body"),
            labels: Box::new([]),
        }),
    );
    h.answer(fixtures::FORGE, &fixtures::response(201, br#"{"number":7}"#));
    h.run(Duration::ZERO);
    let answered = h.answered(1).expect("malformed creation answered");
    assert_eq!((answered.cost, &answered.result), (1, &Err(Error::Timeout)));
}
#[test]
fn write_refusals_read_only_known_entire_messages() {
    let rows = [
        ("known", br#"{"message":"[Title]: Required"}"#.as_slice(), Error::Empty),
        ("unknown", br#"{"message":"other words"}"#.as_slice(), Error::Refused),
        ("not JSON", b"arbitrary".as_slice(), Error::Refused),
    ];
    for (name, body, expected) in rows {
        let mut h = fixtures::known();
        h.call(
            1,
            fixtures::REPO,
            Op::Write(Write::CreateIssue {
                key: boxed(b"key"),
                title: boxed(b"title"),
                body: boxed(b"body"),
                labels: Box::new([]),
            }),
        );
        h.answer(fixtures::FORGE, &fixtures::response(422, body));
        h.run(Duration::ZERO);
        let answered = h.answered(1).expect("refusal answered");
        assert_eq!((answered.cost, &answered.result), (1, &Err(expected)), "{name}");
    }
}
type CreationRow = (&'static str, Write, &'static [u8], u16, &'static [u8], Answer);
type WriteRow = (&'static str, Write, &'static [u8], &'static [u8], u16, &'static [u8], Answer);
type FailureRow = (&'static str, Asked, u16, Option<Refusal>, Option<Duration>, Error, Drop);
const PULL: &[u8] = br#"{"number":7,"state":"open","head":{"repo_id":1,"ref":"work","sha":"0100000000000000000000000000000000000000"},"base":{"repo_id":1,"ref":"main","sha":"0200000000000000000000000000000000000000"},"merged":false,"merge_commit_sha":null,"merge_base":"0200000000000000000000000000000000000000","mergeable":true,"requested_reviewers":[]}"#;
const MERGED: &[u8] = br#"{"number":7,"state":"closed","head":{"repo_id":1,"ref":"work","sha":"0100000000000000000000000000000000000000"},"base":{"repo_id":1,"ref":"main","sha":"0200000000000000000000000000000000000000"},"merged":true,"merge_commit_sha":"0300000000000000000000000000000000000000","merge_base":"0200000000000000000000000000000000000000","mergeable":true,"requested_reviewers":[]}"#;
const REVIEWERS: &[u8] = br#"{"number":7,"state":"open","head":{"repo_id":1,"ref":"work","sha":"0100000000000000000000000000000000000000"},"base":{"repo_id":1,"ref":"main","sha":"0200000000000000000000000000000000000000"},"merged":false,"merge_commit_sha":null,"merge_base":"0200000000000000000000000000000000000000","mergeable":true,"requested_reviewers":[{"id":2,"login":"writer"},{"id":3,"login":"extra"}]}"#;
fn opening(h: &mut Harness) {
    h.call(
        1,
        fixtures::REPO,
        Op::Write(Write::OpenPull {
            title: boxed(b"title"),
            body: boxed(b"body"),
            head: boxed(b"work"),
            base: boxed(b"main"),
        }),
    );
    h.run(Duration::ZERO);
}
fn response(h: &mut Harness, code: u16, body: &[u8]) {
    h.answer(fixtures::FORGE, &fixtures::response(code, body));
    h.run(Duration::ZERO);
}
#[test]
fn open_pull_with_nothing_to_merge_compares_and_writes_nothing() {
    let mut h = fixtures::known();
    opening(&mut h);
    response(&mut h, 200, BRANCH);
    response(&mut h, 200, BRANCH);
    response(&mut h, 200, br#"{"total_commits":0,"commits":[],"files":[]}"#);
    let answered = h.answered(1).expect("empty comparison answered");
    assert_eq!((answered.cost, &answered.result), (3, &Err(Error::NothingToMerge)));
    let written = h.request_bytes(fixtures::FORGE);
    assert!(bytes::find(&written, b"GET /api/v1/repos/fixture/specimen/branches/work ").is_some());
    assert!(bytes::find(&written, b"GET /api/v1/repos/fixture/specimen/branches/main ").is_some());
    assert!(bytes::find(&written, b"GET /api/v1/repos/fixture/specimen/compare/0100000000000000000000000000000000000000...0100000000000000000000000000000000000000?files=false&verification=false ").is_some());
    assert!(bytes::find(&written, b"POST ").is_none(), "nothing written after comparison");
}
#[test]
fn open_pull_posts_only_after_its_comparison_has_commits() {
    let mut h = fixtures::known();
    opening(&mut h);
    response(&mut h, 200, BRANCH);
    response(&mut h, 200, BRANCH);
    response(
        &mut h,
        200,
        br#"{"total_commits":1,"commits":[{"sha":"0100000000000000000000000000000000000000"}],"files":[]}"#,
    );
    let written = h.request_bytes(fixtures::FORGE);
    assert!(bytes::find(&written, br#"{"head":"work","base":"main","title":"title","body":"body"}"#).is_some());
    response(&mut h, 201, PULL);
    let answered = h.answered(1).expect("opened");
    assert_eq!((answered.cost, &answered.result), (4, &Ok(Answer::Created(7))));
}
#[test]
fn opening_before_its_post_turns_uncertain_reads_into_unavailable() {
    let rows = [
        ("5xx", 500, b"".as_slice(), Error::Unavailable),
        ("malformed", 200, b"{}".as_slice(), Error::Unavailable),
        ("missing", 404, b"".as_slice(), Error::Missing),
        ("unknown 4xx", 418, br#"{"message":"other"}"#.as_slice(), Error::Unavailable),
    ];
    for (name, code, body, expected) in rows {
        let mut h = fixtures::known();
        opening(&mut h);
        response(&mut h, code, body);
        let answered = h.answered(1).expect("failed branch read");
        assert_eq!((answered.cost, &answered.result), (1, &Err(expected)), "{name}");
    }
}
#[test]
fn openings_deadline_before_post_is_unavailable_after_post_timeout() {
    for (name, post, expected, cost) in [("before", false, Error::Unavailable, 1), ("after", true, Error::Timeout, 4)] {
        let mut h = fixtures::known();
        opening(&mut h);
        if post {
            response(&mut h, 200, BRANCH);
            response(&mut h, 200, BRANCH);
            response(&mut h, 200, br#"{"total_commits":1,"commits":[],"files":[]}"#);
        }
        h.run(Duration::from_secs(10));
        let answered = h.answered(1).expect("deadline answered");
        assert_eq!((answered.cost, &answered.result), (cost, &Err(expected)), "{name}");
    }
}
#[test]
fn a_merge_sends_the_rows_style_and_reads_its_commit() {
    let mut h = fixtures::known();
    h.call(
        1,
        fixtures::REPO,
        Op::Write(Write::Merge { number: 7, head: head(), how: skein_forge::api::MergeHow::Style }),
    );
    h.run(Duration::ZERO);
    let written = h.request_bytes(fixtures::FORGE);
    assert!(
        bytes::find(&written, br#"{"Do":"squash","head_commit_id":"0100000000000000000000000000000000000000"}"#)
            .is_some()
    );
    response(&mut h, 200, b"");
    assert!(bytes::find(&h.request_bytes(fixtures::FORGE), b"GET /api/v1/repos/fixture/specimen/pulls/7 ").is_some());
    response(&mut h, 200, MERGED);
    let mut commit = [0; 32];
    commit[0] = 3;
    let answered = h.answered(1).expect("merge answered");
    assert_eq!((answered.cost, &answered.result), (2, &Ok(Answer::Merged(commit))));
}
#[test]
fn every_failure_of_a_merges_read_is_timeout() {
    let rows = [
        ("missing", 404, b"".as_slice()),
        ("rate", 429, br#"{"message":"wait"}"#.as_slice()),
        ("malformed", 200, b"{}".as_slice()),
        ("unmerged", 200, PULL),
    ];
    for (name, code, body) in rows {
        let mut h = fixtures::known();
        h.call(
            1,
            fixtures::REPO,
            Op::Write(Write::Merge { number: 7, head: head(), how: skein_forge::api::MergeHow::Style }),
        );
        response(&mut h, 200, b"");
        response(&mut h, code, body);
        let answered = h.answered(1).expect("uncertain merge answered");
        assert_eq!((answered.cost, &answered.result), (2, &Err(Error::Timeout)), "{name}");
    }
}
#[test]
fn a_merge_style_refusal_drops_the_repository_row() {
    let mut h = fixtures::known();
    h.call(
        1,
        fixtures::REPO,
        Op::Write(Write::Merge { number: 7, head: head(), how: skein_forge::api::MergeHow::Style }),
    );
    response(&mut h, 405, br#"{"message":"squash is not an allowed merge style for this repository"}"#);
    let answered = h.answered(1).expect("style refused");
    assert_eq!((answered.cost, &answered.result), (1, &Err(Error::Refused)));
    h.call(2, fixtures::REPO, Op::Write(Write::Update { number: 7, style: skein_forge::api::UpdateStyle::Merge }));
    h.run(Duration::ZERO);
    assert!(bytes::find(&h.request_bytes(fixtures::FORGE), b"GET /api/v1/repositories/1 ").is_some());
}
#[test]
fn set_reviewers_looks_up_missing_ids_then_adds_and_removes_only_the_difference() {
    let mut h = fixtures::known();
    h.call(1, fixtures::REPO, Op::Write(Write::SetReviewers { number: 7, reviewers: Box::from([2, 4]) }));
    response(&mut h, 200, REVIEWERS);
    assert!(bytes::find(&h.request_bytes(fixtures::FORGE), b"GET /api/v1/users/search?uid=4&limit=1 ").is_some());
    response(&mut h, 200, br#"{"ok":true,"data":[{"id":4,"login":"missing"}]}"#);
    assert!(bytes::find(&h.request_bytes(fixtures::FORGE), br#"{"reviewers":["missing"]}"#).is_some());
    response(&mut h, 201, b"");
    let written = h.request_bytes(fixtures::FORGE);
    assert!(bytes::find(&written, b"DELETE /api/v1/repos/fixture/specimen/pulls/7/requested_reviewers ").is_some());
    assert!(bytes::find(&written, br#"{"reviewers":["extra"]}"#).is_some());
    response(&mut h, 204, b"");
    let answered = h.answered(1).expect("reviewers answered");
    assert_eq!((answered.cost, &answered.result), (4, &Ok(Answer::Done)));
}
#[test]
fn set_reviewers_with_no_difference_writes_only_the_pull_read() {
    let mut h = fixtures::known();
    h.call(1, fixtures::REPO, Op::Write(Write::SetReviewers { number: 7, reviewers: Box::from([2, 3]) }));
    response(&mut h, 200, REVIEWERS);
    let answered = h.answered(1).expect("reviewers unchanged");
    assert_eq!((answered.cost, &answered.result), (1, &Ok(Answer::Done)));
    let written = h.request_bytes(fixtures::FORGE);
    assert!(bytes::find(&written, b"POST ").is_none());
    assert!(bytes::find(&written, b"DELETE ").is_none());
}
#[test]
fn a_missing_reviewer_fails_before_any_reviewer_write() {
    let mut h = fixtures::known();
    h.call(1, fixtures::REPO, Op::Write(Write::SetReviewers { number: 7, reviewers: Box::from([4]) }));
    response(&mut h, 200, PULL);
    response(&mut h, 200, br#"{"ok":true,"data":[]}"#);
    let answered = h.answered(1).expect("reviewer missing");
    assert_eq!((answered.cost, &answered.result), (2, &Err(Error::Missing)));
    assert!(bytes::find(&h.request_bytes(fixtures::FORGE), b"POST ").is_none());
}
#[test]
fn write_answers_with_another_object_format_are_uncertain() {
    let rows = [
        ("branch", Write::CreateBranch { branch: boxed(b"work"), commit: head() }, 201, BRANCH),
        (
            "review",
            Write::Review { number: 7, key: boxed(b"key"), verdict: Verdict::Comment, body: boxed(b"body") },
            201,
            REVIEW,
        ),
        ("reviewers", Write::SetReviewers { number: 7, reviewers: Box::from([2]) }, 200, REVIEWERS),
    ];
    for (name, write, code, body) in rows {
        let mut h = fixtures::known();
        let mut row = fixtures::specimen();
        row.object_format = skein_forge::forgejo::id::ObjectFormat::Sha256;
        h.known(fixtures::REPO, row);
        h.call(1, fixtures::REPO, Op::Write(write));
        response(&mut h, code, body);
        let answered = h.answered(1).expect("bad format answered");
        assert_eq!((answered.cost, &answered.result), (1, &Err(Error::Timeout)), "{name}");
    }
}

const LABEL_CATALOG: &[u8] = br#"[{"id":7,"name":"finding"}]"#;
fn label_catalog_answered(write: Write, catalog: &[u8]) -> Harness {
    let mut h = fixtures::known();
    h.call(1, fixtures::REPO, Op::Write(write));
    h.run(Duration::from_millis(1));
    h.answer(fixtures::FORGE, &fixtures::response(200, catalog));
    h.run(Duration::ZERO);
    h
}
#[test]
fn label_creation_charges_its_catalog_and_post() {
    let mut h =
        label_catalog_answered(Write::CreateLabel { name: boxed(b"finding"), color: [0x12, 0xab, 0xef] }, b"[]");
    h.answer(fixtures::FORGE, &fixtures::response(201, br#"{"id":7,"name":"finding"}"#));
    h.run(Duration::ZERO);
    let answer = h.answered(1).expect("creation answered");
    assert_eq!((answer.cost, &answer.result), (2, &Ok(Answer::Done)));
}
#[test]
fn finding_an_existing_label_charges_only_the_catalog() {
    let h = label_catalog_answered(Write::CreateLabel { name: boxed(b"finding"), color: [0; 3] }, LABEL_CATALOG);
    let answer = h.answered(1).expect("lookup answered");
    assert_eq!((answer.cost, &answer.result), (1, &Err(Error::Exists)));
}
#[test]
fn an_unknown_label_costs_one_read_and_no_write() {
    let h = label_catalog_answered(Write::Labels { number: 7, labels: Box::new([boxed(b"absent")]) }, LABEL_CATALOG);
    let answer = h.answered(1).expect("absence answered");
    assert_eq!((answer.cost, &answer.result), (1, &Err(Error::Missing)));
    assert!(h.request_lines(fixtures::FORGE).is_empty());
}
#[test]
fn replacing_labels_charges_the_catalog_and_put() {
    let mut h =
        label_catalog_answered(Write::Labels { number: 7, labels: Box::new([boxed(b"finding")]) }, LABEL_CATALOG);
    h.answer(fixtures::FORGE, &fixtures::response(200, LABEL_CATALOG));
    h.run(Duration::ZERO);
    let answer = h.answered(1).expect("replacement answered");
    assert_eq!((answer.cost, &answer.result), (2, &Ok(Answer::Done)));
}
