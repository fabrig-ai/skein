//! Canned exchanges for every call, without a connected component.
use crate::api::{self, Answer, Error, Op, Read};
use crate::forgejo::plans::plan::{Range, Step};
use crate::forgejo::tests::fixtures::{
    self, ATTEMPT, BASE, BOUNDS, BRANCH, EMPTY_COMPARE, EMPTY_STATUS, HEAD, JOB, PAGE_SIZE, PULL, RAW_BYTES, REPO,
    REPOSITORY, ROWS, RUN, Run,
};
use skein_lib::Time;
const ISSUE: &[u8] = br#"{"number":1,"state":"open","user":{"id":2,"login":"writer"},"labels":[],"updated_at":"1970-01-01T00:00:00Z","title":"t","body":"b"}"#;
fn summary() -> api::Summary {
    api::Summary {
        number: 1,
        kind: api::Kind::Issue,
        state: api::State::Open,
        author: 2,
        labels: Box::new([]),
        title: Box::from(&b"t"[..]),
        body: Box::from(&b"b"[..]),
        key: None,
        updated: Time::ZERO,
    }
}
fn pull() -> api::Pull {
    api::Pull {
        queued: None,
        dequeued: None,
        number: 1,
        state: api::State::Open,
        head: Box::from(&b"work"[..]),
        base: Box::from(&b"main"[..]),
        commit: HEAD,
        base_commit: Some(BASE),
        merged: None,
        mergeable: true,
        reviewers: Box::new([]),
        ci: api::Ci::None,
    }
}
pub(super) struct ReadCase {
    pub(super) name: &'static str,
    pub(super) read: Read,
    pub(super) replies: &'static [(u16, &'static [u8])],
    pub(super) expected: Result<Answer, Error>,
    sends: &'static [&'static str],
    range: Option<Range>,
    kept: bool,
    login: Option<&'static [u8]>,
}
pub(super) fn read_cases() -> [[ReadCase; 5]; 4] {
    [find_cases(), review_cases(), content_cases(), repository_cases()]
}
fn find_cases() -> [ReadCase; 5] {
    [
        ReadCase {
            name: "locate",
            read: Read::Locate { owner: Box::from(&b"fixture"[..]), name: Box::from(&b"specimen"[..]) },
            replies: &[(200, REPOSITORY)],
            expected: Ok(Answer::Located { repository: 1 }),
            sends: &["GET /api/v1/repos/fixture/specimen"],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "items",
            read: Read::Items { since: Time::ZERO, page: 1, kind: None },
            replies: &[(200, REPOSITORY), (200, b"[]")],
            expected: Ok(Answer::Items { items: Box::new([]), more: false, now: Time::ZERO }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/issues?state=all&since=1970-01-01T00%3A00%3A00Z&sort=leastupdate&page=1&limit=2",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "item",
            read: Read::Item { number: 1, after: 0 },
            replies: &[(200, REPOSITORY), (200, ISSUE), (200, b"[]")],
            expected: Ok(Answer::Item { item: summary(), comments: Box::new([]), more: false }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/issues/1",
                "GET /api/v1/repos/fixture/specimen/issues/1/comments",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "pull",
            read: Read::Pull { number: 1 },
            replies: &[(200, REPOSITORY), (200, PULL), (200, EMPTY_STATUS)],
            expected: Ok(Answer::Pull(pull())),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/pulls/1",
                "GET /api/v1/repos/fixture/specimen/commits/0100000000000000000000000000000000000000/status?page=1&limit=2",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "pull for",
            read: Read::PullFor { head: Box::from(&b"work"[..]), base: Box::from(&b"main"[..]) },
            replies: &[(200, REPOSITORY), (200, PULL), (200, EMPTY_STATUS)],
            expected: Ok(Answer::Pull(pull())),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/pulls/main/work",
                "GET /api/v1/repos/fixture/specimen/commits/0100000000000000000000000000000000000000/status?page=1&limit=2",
            ],
            range: None,
            kept: true,
            login: None,
        },
    ]
}
fn review_cases() -> [ReadCase; 5] {
    [
        ReadCase {
            name: "reviews",
            read: Read::Reviews { number: 1, page: 1 },
            replies: &[(200, REPOSITORY), (200, b"[]")],
            expected: Ok(Answer::Reviews { reviews: Box::new([]), more: false }),
            sends: &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/pulls/1/reviews?page=1&limit=2"],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "statuses",
            read: Read::Statuses { commit: HEAD, page: 1 },
            replies: &[(200, REPOSITORY), (200, EMPTY_STATUS)],
            expected: Ok(Answer::Statuses { ci: api::Ci::None, statuses: Box::new([]), more: false }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/commits/0100000000000000000000000000000000000000/status?page=1&limit=2",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "remarks",
            read: Read::Remarks { number: 1, review: 7, page: 1 },
            replies: &[(200, REPOSITORY), (200, b"[]")],
            expected: Ok(Answer::Remarks { remarks: Box::new([]), more: false }),
            sends: &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/pulls/1/reviews/7/comments"],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "branch",
            read: Read::Branch { branch: Box::from(&b"work"[..]) },
            replies: &[(200, REPOSITORY), (200, BRANCH)],
            expected: Ok(Answer::Commit(HEAD)),
            sends: &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/branches/work"],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "branches",
            read: Read::Branches { prefix: Box::from(&b"m"[..]) },
            replies: &[(200, REPOSITORY), (200, b"[]")],
            expected: Ok(Answer::Branches(Box::new([]))),
            sends: &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/git/refs/heads/m"],
            range: None,
            kept: true,
            login: None,
        },
    ]
}
fn content_cases() -> [ReadCase; 5] {
    [
        ReadCase {
            name: "pull files",
            read: Read::PullFiles { number: 1, head: HEAD, page: 1 },
            replies: &[(200, REPOSITORY), (200, PULL), (200, b"[]")],
            expected: Ok(Answer::PullFiles { head: HEAD, files: Box::new([]), more: false }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/pulls/1",
                "GET /api/v1/repos/fixture/specimen/pulls/1/files?page=1&limit=2",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "compare",
            read: Read::Compare { before: BASE, after: HEAD },
            replies: &[(200, REPOSITORY), (200, EMPTY_COMPARE), (200, EMPTY_COMPARE)],
            expected: Ok(Answer::Compare {
                before: BASE,
                after: HEAD,
                contains_before: true,
                files: Box::new([]),
                commits: Box::new([]),
            }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/compare/0300000000000000000000000000000000000000...0100000000000000000000000000000000000000?verification=false",
                "GET /api/v1/repos/fixture/specimen/compare/0100000000000000000000000000000000000000...0300000000000000000000000000000000000000?files=false&verification=false",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "checks",
            read: Read::Checks { commit: HEAD },
            replies: &[(200, REPOSITORY), (200, EMPTY_STATUS)],
            expected: Ok(Answer::Checks(Box::new([]))),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/commits/0100000000000000000000000000000000000000/status?page=1&limit=2",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "job",
            read: Read::Job { attempt: ATTEMPT, max_bytes: RAW_BYTES },
            replies: &[(200, REPOSITORY), (200, RUN), (200, JOB), (200, b"abc")],
            expected: Ok(Answer::Job {
                attempt: ATTEMPT,
                step: Some(api::Step { number: 1, name: Box::from(&b"x"[..]) }),
                log: Box::from(&b"abc"[..]),
                truncated: false,
            }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/actions/runs/7",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10/logs?attempt=3&step=1",
            ],
            range: Some(Range::Tail(RAW_BYTES)),
            kept: true,
            login: None,
        },
        ReadCase {
            name: "file",
            read: Read::File { head: HEAD, path: Box::from(&b"x"[..]), max_bytes: RAW_BYTES },
            replies: &[(200, REPOSITORY), (200, b"abc")],
            expected: Ok(Answer::File {
                head: HEAD,
                path: Box::from(&b"x"[..]),
                bytes: Box::from(&b"abc"[..]),
                truncated: false,
            }),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/raw/x?ref=0100000000000000000000000000000000000000",
            ],
            range: Some(Range::Head(RAW_BYTES)),
            kept: true,
            login: None,
        },
    ]
}
fn repository_cases() -> [ReadCase; 5] {
    [
        ReadCase {
            name: "protection",
            read: Read::Protection { branch: Box::from(&b"work"[..]) },
            replies: &[(200, REPOSITORY), (200, BRANCH), (200, b"[]")],
            expected: Ok(Answer::Protection(None)),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/repos/fixture/specimen/branches/work",
                "GET /api/v1/repos/fixture/specimen/branch_protections",
            ],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "settings",
            read: Read::Settings,
            replies: &[(200, REPOSITORY)],
            expected: Ok(Answer::Settings(api::Settings {
                default_branch: Box::from(&b"main"[..]),
                merge: true,
                squash: false,
                rebase: false,
            })),
            sends: &["GET /api/v1/repositories/1"],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "collaborators",
            read: Read::Collaborators { page: 1 },
            replies: &[(200, REPOSITORY), (200, b"[]")],
            expected: Ok(Answer::Collaborators { collaborators: Box::new([]), more: false }),
            sends: &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/collaborators?page=1&limit=2"],
            range: None,
            kept: true,
            login: None,
        },
        ReadCase {
            name: "permission",
            read: Read::Permission { user: 7 },
            replies: &[
                (200, REPOSITORY),
                (200, br#"{"ok":true,"data":[{"id":7,"login":"seven"}]}"#),
                (200, br#"{"permission":"write","user":{"id":7,"login":"seven"}}"#),
            ],
            expected: Ok(Answer::Permission(api::Permission::Write)),
            sends: &[
                "GET /api/v1/repositories/1",
                "GET /api/v1/users/search?uid=7&limit=1",
                "GET /api/v1/repos/fixture/specimen/collaborators/seven/permission",
            ],
            range: None,
            kept: true,
            login: Some(b"seven"),
        },
        ReadCase {
            name: "missing branch keeps the row",
            read: Read::Branch { branch: Box::from(&b"gone"[..]) },
            replies: &[(200, REPOSITORY), (404, b"{}")],
            expected: Err(Error::Missing),
            sends: &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/branches/gone"],
            range: None,
            kept: true,
            login: None,
        },
    ]
}
#[test]
fn read_calls_follow_canned_exchanges() {
    for case in read_cases().into_iter().flatten() {
        let mut run = Run::new(Op::Read(case.read));
        let mut step = run.begin();
        for (status, body) in case.replies {
            step = run.answer(*status, body);
        }
        assert_eq!(step, Step::Done(case.expected), "{}", case.name);
        run.assert_sent(case.sends);
        assert_eq!(run.last_range(), case.range, "{}", case.name);
        assert_eq!(
            run.tables().repository(REPO).cloned(),
            if case.kept { Some(fixtures::specimen()) } else { None },
            "{}",
            case.name
        );
        assert_eq!(run.tables().user(REPO.forge, 7), case.login, "{}", case.name);
    }
}
#[test]
fn user_search_uses_id_and_one_result() {
    let mut run = Run::known(Op::Read(Read::Permission { user: 7 }));
    run.begin();
    run.assert_sent(&["GET /api/v1/users/search?uid=7&limit=1"]);
    assert_eq!(run.answer(200, br#"{"ok":true,"data":[]}"#), Step::Done(Err(Error::Missing)));
}
#[test]
fn page_size_is_passed_to_each_listing() {
    for (size, limit) in [(PAGE_SIZE, b"2".as_slice()), (ROWS, b"8".as_slice()), (ONE_ROW_PAGE, b"1".as_slice())] {
        for (read, before, target) in listings() {
            let mut run = Run::with(crate::forgejo::plans::plan::Bounds { page_size: size, ..BOUNDS }, Op::Read(read));
            run.begin();
            for body in before {
                run.answer(200, body);
            }
            run.assert_listing(target, limit);
        }
    }
}
type Listing = (Read, &'static [&'static [u8]], &'static str);
fn listings() -> [Listing; 7] {
    [
        (
            Read::Items { since: Time::ZERO, page: 1, kind: None },
            &[],
            "issues?state=all&since=1970-01-01T00%3A00%3A00Z&sort=leastupdate&page=1&limit=",
        ),
        (Read::Reviews { number: 1, page: 1 }, &[], "pulls/1/reviews?page=1&limit="),
        (
            Read::Statuses { commit: HEAD, page: 1 },
            &[],
            "commits/0100000000000000000000000000000000000000/status?page=1&limit=",
        ),
        (Read::Collaborators { page: 1 }, &[], "collaborators?page=1&limit="),
        (Read::PullFiles { number: 1, head: HEAD, page: 1 }, &[PULL], "pulls/1/files?page=1&limit="),
        (
            Read::PullFor { head: Box::from(&b"work"[..]), base: Box::from(&b"main"[..]) },
            &[CLOSED_PULL],
            "pulls?state=open&sort=oldest&page=1&limit=",
        ),
        (Read::Checks { commit: HEAD }, &[], "commits/0100000000000000000000000000000000000000/status?page=1&limit="),
    ]
}
const CLOSED_PULL: &[u8] = br#"{"number":1,"state":"closed","head":{"repo_id":1,"ref":"work","sha":"0100000000000000000000000000000000000000"},"base":{"repo_id":1,"ref":"main","sha":"0300000000000000000000000000000000000000"},"merged":false,"merge_commit_sha":null,"merge_base":"0300000000000000000000000000000000000000","mergeable":true,"requested_reviewers":[]}"#;
#[test]
fn remarks_are_sliced_without_wire_paging() {
    let page_rows = usize::try_from(PAGE_SIZE).expect("fixture page fits usize");
    let one_row = usize::try_from(ONE_ROW_PAGE).expect("fixture page fits usize");
    for (size, expected, more) in [
        (PAGE_SIZE, &REMARKS[..page_rows], true),
        (ROWS, &REMARKS[..], false),
        (ONE_ROW_PAGE, &REMARKS[..one_row], true),
    ] {
        let mut run = Run::with(
            crate::forgejo::plans::plan::Bounds { page_size: size, ..BOUNDS },
            Op::Read(Read::Remarks { number: 1, review: 7, page: 1 }),
        );
        run.begin();
        let result = run.answer(200, REMARKS_JSON);
        assert_eq!(result, Step::Done(Ok(Answer::Remarks { remarks: remarks(expected), more })), "page size {size}");
        run.assert_sent(&["GET /api/v1/repos/fixture/specimen/pulls/1/reviews/7/comments"]);
    }
}
// A page one row below the protocol fixture exercises the smallest nonempty page.
const ONE_ROW_PAGE: u32 = PAGE_SIZE.saturating_sub(1);
// Three comments straddle a two-row page, with a one-row page as its lower bound.
const REMARKS: [u64; 3] = [1, 2, 3];
const REMARKS_JSON: &[u8] = br#"[{"id":1,"user":{"id":2,"login":"writer"},"path":"x","position":1,"body":"b"},{"id":2,"user":{"id":2,"login":"writer"},"path":"x","position":1,"body":"b"},{"id":3,"user":{"id":2,"login":"writer"},"path":"x","position":1,"body":"b"}]"#;
fn remark(id: u64) -> api::Remark {
    api::Remark { id, author: 2, path: Box::from(&b"x"[..]), line: 1, body: Box::from(&b"b"[..]) }
}
#[test]
fn branch_slashes_have_golden_targets() {
    let mut run = Run::known(Op::Read(Read::Branch { branch: Box::from(&b"a/b"[..]) }));
    run.begin();
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/branches/a%2Fb"]);
}
#[test]
fn far_future_times_have_golden_targets() {
    let mut run = Run::known(Op::Read(Read::Items { since: Time::from_nanos(u64::MAX), page: 1, kind: None }));
    run.begin();
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/issues?state=all&since=2554-07-21T23%3A34%3A33Z&sort=leastupdate&page=1&limit=2"]);
}
#[test]
fn long_body_revision_has_big_endian_golden() {
    let body = crate::forgejo::document::Body::Long(5);
    assert_eq!(super::super::reading::revision(&body, Some(skein_lib::Wall::from_nanos(3))), 0x46c3_95ed_52b0_b56f);
}
#[test]
fn kept_body_revision_has_big_endian_golden() {
    let body = crate::forgejo::document::Body::Kept(Box::from(&b"abc"[..]));
    assert_eq!(super::super::reading::revision(&body, None), 0xba78_16bf_8f01_cfea);
}

fn remarks(ids: &[u64]) -> Box<[api::Remark]> {
    let mut rows = skein_lib::List::with_capacity(ROWS);
    for id in ids {
        rows.push(remark(*id)).unwrap();
    }
    rows.into_boxed()
}

pub(super) mod write_cases;
use crate::api::Write;
use crate::forgejo::plans::{
    Failure,
    failure::{self, Asked, Classified, Drop},
};
use write_cases::{REVIEWERS, begun, create_issue, open_pull};

#[test]
fn write_calls_follow_canned_exchanges() {
    for group in write_cases::cases() {
        for case in group {
            let mut run = Run::known(Op::Write(case.write));
            for (id, login) in case.users {
                run.tables().keep_user(REPO.forge, *id, Box::from(*login));
            }
            let mut step = run.begin();
            for (status, body) in case.replies {
                step = run.answer(*status, body);
            }
            assert_eq!(step, Step::Done(case.expected), "{}", case.name);
            run.assert_sent(case.sends);
            run.assert_bodies(case.bodies);
        }
    }
}

#[test]
fn refusals_match_status_and_whole_message() {
    for (asked, status, message, retry, error, drop) in write_cases::failures() {
        let refusal = crate::forgejo::failure::refusal(status, message);
        assert_eq!(
            failure::classify(asked, status, refusal, retry),
            Classified { error, drop },
            "{asked:?} {status} {message:?} {retry:?}"
        );
    }
    let mut run = begun(create_issue());
    let retry = skein_lib::Duration::from_secs(5);
    let step = run.answer_with(
        429,
        br#"{"message":"slow"}"#,
        crate::forgejo::plans::Fields { retry_after: Some(retry), ..fixtures::fields() },
    );
    assert_eq!(step, Step::Done(Err(Error::RateLimited { after: retry })));
}

#[test]
fn first_failure_ends_the_plan() {
    for (factory, replies, sends) in first_failures() {
        let mut run = factory(Op::Read(Read::Item { number: 1, after: 0 }));
        run.begin();
        for (status, body) in replies {
            run.answer(*status, body);
        }
        assert_eq!(run.answer(500, br#"{"message":"failed"}"#), Step::Done(Err(Error::Timeout)));
        run.assert_sent(sends);
    }
}

#[test]
fn unseen_write_is_unavailable() {
    let mut run = begun(create_issue());
    assert_eq!(run.fail(Failure::Unseen), Step::Done(Err(Error::Unavailable)));
    run.assert_sent(&["POST /api/v1/repos/fixture/specimen/issues"]);
}

#[test]
fn seen_write_is_timeout() {
    let mut run = begun(create_issue());
    assert_eq!(run.fail(Failure::Seen), Step::Done(Err(Error::Timeout)));
    run.assert_sent(&["POST /api/v1/repos/fixture/specimen/issues"]);
}

#[test]
fn open_pull_read_failure_is_unavailable() {
    let failures: [fn(&mut Run) -> Step; 2] = [seen, malformed];
    for (replies, sends) in write_cases::open_failures() {
        for fail in failures {
            let mut run = begun(open_pull());
            for (status, body) in replies {
                run.answer(*status, body);
            }
            assert_eq!(fail(&mut run), Step::Done(Err(Error::Unavailable)), "{replies:?}");
            run.assert_sent(sends);
        }
    }
}

#[test]
fn reviewers_are_added_before_removed() {
    let mut run = Run::known(Op::Write(Write::SetReviewers { number: 1, reviewers: Box::from([7]) }));
    run.tables().keep_user(REPO.forge, 7, Box::from(&b"seven"[..]));
    run.begin();
    run.answer(200, REVIEWERS);
    run.answer(201, b"");
    assert_eq!(run.answer(204, b""), Step::Done(Ok(Answer::Done)));
    run.assert_sent(&[
        "GET /api/v1/repos/fixture/specimen/pulls/1",
        "POST /api/v1/repos/fixture/specimen/pulls/1/requested_reviewers",
        "DELETE /api/v1/repos/fixture/specimen/pulls/1/requested_reviewers",
    ]);
    run.assert_bodies(&[None, Some(br#"{"reviewers":["seven"]}"#), Some(br#"{"reviewers":["eight"]}"#)]);
}

fn seen(run: &mut Run) -> Step {
    run.fail(Failure::Seen)
}
fn malformed(run: &mut Run) -> Step {
    run.answer(200, b"{")
}
type FirstFailure = (fn(Op) -> Run, &'static [(u16, &'static [u8])], &'static [&'static str]);
fn first_failures() -> [FirstFailure; 2] {
    [
        (Run::known, &[], &["GET /api/v1/repos/fixture/specimen/issues/1"]),
        (
            Run::new,
            &[(200, REPOSITORY)],
            &["GET /api/v1/repositories/1", "GET /api/v1/repos/fixture/specimen/issues/1"],
        ),
    ]
}

mod turn;

mod artifact;
