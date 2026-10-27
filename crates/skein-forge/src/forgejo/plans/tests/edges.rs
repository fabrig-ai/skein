//! Canned call boundaries, delivery hints, markers and statuses.
mod paths_and_headers;
use crate::api::{Error, Op, Read};
use crate::forgejo::plans::plan::Step;
use crate::forgejo::tests::fixtures::{ATTEMPT, JOB, PULL, RAW_BYTES, RUN, Run};

#[test]
fn pull_files_stops_at_a_stale_head() {
    let mut run = Run::known(Op::Read(Read::PullFiles { number: 1, head: [0; 32], page: 1 }));
    run.begin();
    assert_eq!(run.answer(200, PULL), Step::Done(Err(Error::Stale)));
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/pulls/1"]);
}

#[test]
fn job_mismatch_is_missing_job() {
    for (attempt, replies, sends) in job_failures() {
        let mut run = Run::known(Op::Read(Read::Job { attempt, max_bytes: RAW_BYTES }));
        let mut step = run.begin();
        for (status, body) in replies {
            step = run.answer(*status, body);
        }
        assert_eq!(step, Step::Done(Err(Error::MissingJob)), "{attempt:?} {replies:?}");
        run.assert_sent(sends);
    }
}

const OTHER_RUN_JOB: &[u8] =
    br#"{"id":10,"run_id":8,"attempt":3,"steps":[{"number":1,"name":"x","status":"failure"}]}"#;
const MISSING: &[u8] = b"{}";
type JobCase = (api::JobAttempt, &'static [(u16, &'static [u8])], &'static [&'static str]);
fn job_failures() -> [JobCase; 6] {
    [
        (
            api::JobAttempt { head: [0; 32], ..ATTEMPT },
            &[(200, RUN)][..],
            &["GET /api/v1/repos/fixture/specimen/actions/runs/7"][..],
        ),
        (
            ATTEMPT,
            &[(200, RUN), (200, OTHER_RUN_JOB)][..],
            &[
                "GET /api/v1/repos/fixture/specimen/actions/runs/7",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10",
            ][..],
        ),
        (
            api::JobAttempt { attempt: 4, ..ATTEMPT },
            &[(200, RUN), (200, JOB)][..],
            &[
                "GET /api/v1/repos/fixture/specimen/actions/runs/7",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10",
            ][..],
        ),
        (ATTEMPT, &[(404, MISSING)][..], &["GET /api/v1/repos/fixture/specimen/actions/runs/7"][..]),
        (
            ATTEMPT,
            &[(200, RUN), (404, MISSING)][..],
            &[
                "GET /api/v1/repos/fixture/specimen/actions/runs/7",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10",
            ][..],
        ),
        (
            ATTEMPT,
            &[(200, RUN), (200, JOB), (404, MISSING)][..],
            &[
                "GET /api/v1/repos/fixture/specimen/actions/runs/7",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10",
                "GET /api/v1/repos/fixture/specimen/actions/jobs/10/logs?attempt=3&step=1",
            ][..],
        ),
    ]
}

use super::calls::{read_cases, write_cases};
use crate::api::{self, Answer, Change, Hint, Write};
use crate::forgejo::{
    delivery::{self, Delivery, JobEnd, RefType},
    document::Body,
    id::{ObjectFormat, ObjectId},
    plans::hint::hint,
    request::ReviewEvent,
    tests::fixtures::{self, HEAD, REPO, WORD},
};
use crate::marker;
use skein_lib::Duration;

#[test]
fn subscriptions_match_the_literal_contract() {
    assert_eq!(
        delivery::SUBSCRIBED,
        [
            &b"issues"[..],
            b"issue_assign",
            b"issue_label",
            b"issue_milestone",
            b"pull_request",
            b"pull_request_assign",
            b"pull_request_label",
            b"pull_request_milestone",
            b"pull_request_sync",
            b"pull_request_review_request",
            b"issue_comment",
            b"pull_request_comment",
            b"pull_request_review_approved",
            b"pull_request_review_rejected",
            b"pull_request_review_comment",
            b"push",
            b"create",
            b"delete",
            b"workflow_job_success",
            b"workflow_job_failure",
            b"workflow_job_cancelled",
            b"workflow_job_skipped",
        ]
    );
}

#[test]
fn deliveries_map_subscribed_events_to_hints() {
    for (header, delivery, change, key) in deliveries() {
        assert_eq!(delivery::header(&delivery), header);
        assert_eq!(hint(REPO.forge, WORD, &delivery), Some(Hint { repository: REPO, change, key }), "{header:?}");
    }
}

#[test]
fn irrelevant_delivery_has_no_hint() {
    for delivery in [
        Delivery::Push {
            repository: u64::from(REPO.repository),
            reference: Box::from(&b"refs/tags/v1"[..]),
            after: object(HEAD),
            pusher: fixtures::START_USER,
        },
        Delivery::Ref {
            created: true,
            repository: u64::from(REPO.repository),
            reference: Box::from(&b"refs/tags/v1"[..]),
            ref_type: RefType::Tag,
        },
        Delivery::Ref {
            created: false,
            repository: u64::from(REPO.repository),
            reference: Box::from(&b"refs/tags/v1"[..]),
            ref_type: RefType::Tag,
        },
        Delivery::Pull { repository: 4_294_967_296, number: ITEM_NUMBER },
    ] {
        assert_eq!(hint(REPO.forge, WORD, &delivery), None, "{delivery:?}");
    }
}

// PULL's issue number, shared by the status and delivery fixtures.
const ITEM_NUMBER: u64 = 1;
// JOB has one failed step, numbered 1.
const STEP_NUMBER: u32 = 1;
// forge.md 6: the owner's key is at most op_bytes (the protocol fixture's 512).
const OP_BYTES: usize = 512;
const KEY: [u8; OP_BYTES] = [0xab; OP_BYTES];
const ONE_WORD: &[u8] = b"a";
const LONGEST_WORD: &[u8] = b"abcdefghijklmnop";
// Twice OP_BYTES plus the literal word's length plus the 15 framing bytes.
const ONE_MARKER_BYTES: u32 = 1040;
const LONGEST_MARKER_BYTES: u32 = 1055;
const KIT_MARKER_BYTES: u32 = 1042;
const MARKERS: [(&[u8], u32); 3] =
    [(ONE_WORD, ONE_MARKER_BYTES), (LONGEST_WORD, LONGEST_MARKER_BYTES), (WORD, KIT_MARKER_BYTES)];

#[test]
fn marker_words_at_both_bounds_are_valid() {
    for word in [ONE_WORD, LONGEST_WORD, WORD] {
        assert!(marker::valid_word(word));
    }
}

#[test]
fn markers_accept_words_at_both_bounds() {
    for (word, size) in MARKERS {
        assert_eq!(marker::length(word, u32::try_from(OP_BYTES).unwrap()), Some(size));
        let encoded = marker::encode(word, &KEY, b"", size).expect("the exact marker fits");
        assert_eq!(encoded.len(), usize::try_from(size).unwrap());
        assert_eq!(marker::decode(word, &encoded), Some(Box::from(KEY)));
    }
}

#[test]
fn markers_past_the_limit_are_refused() {
    for (word, size) in MARKERS {
        assert_eq!(marker::encode(word, &KEY, b"", size.saturating_sub(1)), None, "{word:?}");
    }
}

#[test]
fn malformed_word_is_refused() {
    for word in [&b""[..], b"abcdefghijklmnopq", b"A", b"a b", b"a:b"] {
        assert!(!marker::valid_word(word), "{word:?}");
    }
}

#[test]
fn another_word_has_no_key() {
    assert_eq!(marker::decode(b"kit", b"<!-- other:key ab -->\n"), None);
}

#[test]
fn success_statuses_match_each_exchange() {
    for case in successful_reads().into_boxed() {
        let mut run = Run::new(Op::Read(case.read));
        assert_eq!(replies(&mut run, case.replies), Step::Done(case.expected), "{}", case.name);
    }
    for case in successful_writes().into_boxed() {
        let mut run = write_run(case.write, case.users);
        assert_eq!(replies(&mut run, case.replies), Step::Done(case.expected), "{}", case.name);
    }
    let mut run = write_run(Write::SetReviewers { number: ITEM_NUMBER, reviewers: Box::new([]) }, &[(8, b"eight")]);
    assert_eq!(replies(&mut run, &[(200, write_cases::REVIEWERS), (204, b"")]), Step::Done(Ok(Answer::Done)));
    for (op, answers, expected) in partial_reads() {
        let mut run = Run::known(op);
        assert_eq!(replies(&mut run, answers), Step::Done(Ok(expected)));
    }
}

#[test]
fn unexpected_read_status_is_invalid_answer() {
    for status in 201..=299 {
        let mut run = Run::known(Op::Read(Read::Branches { prefix: Box::from(&b"m"[..]) }));
        run.begin();
        assert_eq!(run.answer(status, b"[]"), Step::Done(Err(Error::InvalidAnswer)), "{status}");
    }
}

#[test]
fn unexpected_write_status_is_timeout() {
    for case in successful_writes().into_boxed() {
        let ((accepted, body), earlier) = case.replies.split_last().expect("a write has an answer");
        for status in unexpected_statuses(*accepted).into_boxed() {
            let mut run = write_run(case.write.clone(), case.users);
            replies(&mut run, earlier);
            assert_eq!(run.answer(status, body), Step::Done(Err(Error::Timeout)), "{} {status}", case.name);
        }
    }
}

#[test]
fn unexpected_open_pull_read_status_is_unavailable() {
    for (answers, sends) in write_cases::open_failures() {
        for status in [201, 202, 204, 206, 299] {
            let mut run = Run::known(Op::Write(write_cases::open_pull()));
            replies(&mut run, answers);
            assert_eq!(run.answer(status, b"{}"), Step::Done(Err(Error::Unavailable)), "{status} {sends:?}");
            run.assert_sent(sends);
        }
    }
}

const RETRY: Duration = Duration::from_secs(5);
// Merge sends one POST before reading the result (forge.md 5.2).
const MERGE_POST_REQUESTS: usize = 1;
const MERGE_SENDS: [&str; 2] =
    ["POST /api/v1/repos/fixture/specimen/pulls/1/merge", "GET /api/v1/repos/fixture/specimen/pulls/1"];

#[test]
fn merge_read_429_is_timeout() {
    for retry_after in [None, Some(RETRY)] {
        let mut run = write_cases::begun(Write::Merge { number: ITEM_NUMBER, head: HEAD, how: api::MergeHow::Style });
        run.answer(200, b"");
        assert_eq!(
            run.answer_with(429, b"{}", super::super::Fields { retry_after, ..fixtures::fields() }),
            Step::Done(Err(Error::Timeout))
        );
        run.assert_sent(&MERGE_SENDS);
    }
}

#[test]
fn merge_write_429_is_rate_limited() {
    let mut run = write_cases::begun(Write::Merge { number: ITEM_NUMBER, head: HEAD, how: api::MergeHow::Style });
    assert_eq!(run.answer(429, b"{}"), Step::Done(Err(Error::RateLimited { after: Duration::ZERO })));
    run.assert_sent(&MERGE_SENDS[..MERGE_POST_REQUESTS]);
}

/// A missing Retry-After gives zero; X-HasMore's fullness fallback is covered separately.
#[test]
fn rate_limited_without_retry_after_waits_zero() {
    let mut run = Run::known(Op::Read(Read::Branches { prefix: Box::from(&b"m"[..]) }));
    run.begin();
    assert_eq!(run.answer(429, b"{}"), Step::Done(Err(Error::RateLimited { after: Duration::ZERO })));
}

fn successful_reads() -> skein_lib::List<super::calls::ReadCase> {
    let cases = read_cases();
    let mut rows = skein_lib::List::with_capacity(
        u32::try_from(cases.len().saturating_mul(cases.first().unwrap().len())).unwrap(),
    );
    for case in cases.into_iter().flatten() {
        if case.expected.is_ok() {
            rows.push(case).ok().expect("the existing read matrix fits");
        }
    }
    rows
}
fn successful_writes() -> skein_lib::List<write_cases::WriteCase> {
    let cases = write_cases::cases();
    let mut rows = skein_lib::List::with_capacity(
        u32::try_from(cases.len().saturating_mul(cases.first().unwrap().len())).unwrap(),
    );
    for case in cases.into_iter().flatten() {
        if case.expected.is_ok() {
            rows.push(case).ok().expect("the existing write matrix fits");
        }
    }
    rows
}
fn unexpected_statuses(accepted: u16) -> skein_lib::List<u16> {
    const PROBES: [u16; 6] = [200, 201, 202, 204, 206, 299];
    let mut rows = skein_lib::List::with_capacity(u32::try_from(PROBES.len()).unwrap());
    for status in PROBES {
        if status != accepted {
            rows.push(status).expect("the probe array fits");
        }
    }
    rows
}
fn write_run(write: Write, users: &[(u64, &[u8])]) -> Run {
    let mut run = Run::known(Op::Write(write));
    for (id, login) in users {
        run.tables().keep_user(REPO.forge, *id, Box::from(*login));
    }
    run
}
fn replies(run: &mut Run, replies: &[(u16, &[u8])]) -> Step {
    let mut step = run.begin();
    for (status, body) in replies {
        step = run.answer(*status, body);
    }
    step
}
fn object(bytes: [u8; 32]) -> ObjectId {
    ObjectId { format: ObjectFormat::Sha1, bytes }
}
type DeliveryCase = (&'static [u8], Delivery, Change, Option<Box<[u8]>>);
fn deliveries() -> impl Iterator<Item = DeliveryCase> {
    item_deliveries().into_iter().chain(branch_deliveries()).chain(job_deliveries())
}
fn item_deliveries() -> [DeliveryCase; 7] {
    let body = Body::Kept(Box::from(&b"<!-- kit:key ab -->\n"[..]));
    let key = Some(Box::from(&b"\xab"[..]));
    [
        (
            b"issues",
            Delivery::Issue { repository: u64::from(REPO.repository), number: ITEM_NUMBER, body: body.clone() },
            Change::Item(ITEM_NUMBER),
            key.clone(),
        ),
        (
            b"pull_request",
            Delivery::Pull { repository: u64::from(REPO.repository), number: ITEM_NUMBER },
            Change::Item(ITEM_NUMBER),
            None,
        ),
        (
            b"issue_comment",
            Delivery::Comment { repository: u64::from(REPO.repository), number: ITEM_NUMBER, body: body.clone() },
            Change::Item(ITEM_NUMBER),
            key.clone(),
        ),
        (
            b"pull_request_approved",
            Delivery::Review {
                event: ReviewEvent::Approved,
                repository: u64::from(REPO.repository),
                number: ITEM_NUMBER,
                body: body.clone(),
            },
            Change::Item(ITEM_NUMBER),
            key.clone(),
        ),
        (
            b"pull_request_rejected",
            Delivery::Review {
                event: ReviewEvent::RequestChanges,
                repository: u64::from(REPO.repository),
                number: ITEM_NUMBER,
                body: body.clone(),
            },
            Change::Item(ITEM_NUMBER),
            key.clone(),
        ),
        (
            b"pull_request_comment",
            Delivery::Review {
                event: ReviewEvent::Comment,
                repository: u64::from(REPO.repository),
                number: ITEM_NUMBER,
                body,
            },
            Change::Item(ITEM_NUMBER),
            key,
        ),
        (
            b"issues",
            Delivery::Issue {
                repository: u64::from(REPO.repository),
                number: ITEM_NUMBER,
                body: Body::Long(u64::from(ONE_MARKER_BYTES)),
            },
            Change::Item(ITEM_NUMBER),
            None,
        ),
    ]
}
fn branch_deliveries() -> [DeliveryCase; 4] {
    let branch: Box<[u8]> = Box::from(&b"work"[..]);
    let reference: Box<[u8]> = Box::from(&b"refs/heads/work"[..]);
    [
        (
            b"push",
            Delivery::Push {
                repository: u64::from(REPO.repository),
                reference: reference.clone(),
                after: object(HEAD),
                pusher: fixtures::START_USER,
            },
            Change::BranchMoved { branch: branch.clone(), head: HEAD, actor: fixtures::START_USER },
            None,
        ),
        (
            b"push",
            Delivery::Push {
                repository: u64::from(REPO.repository),
                reference: reference.clone(),
                after: object([0; 32]),
                pusher: fixtures::START_USER,
            },
            Change::Branch(branch.clone()),
            None,
        ),
        (
            b"create",
            Delivery::Ref {
                created: true,
                repository: u64::from(REPO.repository),
                reference: reference.clone(),
                ref_type: RefType::Branch,
            },
            Change::Branch(branch.clone()),
            None,
        ),
        (
            b"delete",
            Delivery::Ref {
                created: false,
                repository: u64::from(REPO.repository),
                reference,
                ref_type: RefType::Branch,
            },
            Change::Branch(branch),
            None,
        ),
    ]
}
fn job_deliveries() -> [DeliveryCase; 4] {
    [
        (
            b"workflow_job_success",
            Delivery::Job { end: JobEnd::Success, repository: u64::from(REPO.repository), commit: object(HEAD) },
            Change::Commit(HEAD),
            None,
        ),
        (
            b"workflow_job_failure",
            Delivery::Job { end: JobEnd::Failure, repository: u64::from(REPO.repository), commit: object(HEAD) },
            Change::Commit(HEAD),
            None,
        ),
        (
            b"workflow_job_cancelled",
            Delivery::Job { end: JobEnd::Cancelled, repository: u64::from(REPO.repository), commit: object(HEAD) },
            Change::Commit(HEAD),
            None,
        ),
        (
            b"workflow_job_skipped",
            Delivery::Job { end: JobEnd::Skipped, repository: u64::from(REPO.repository), commit: object(HEAD) },
            Change::Commit(HEAD),
            None,
        ),
    ]
}

type PartialRead = (Op, &'static [(u16, &'static [u8])], Answer);
fn partial_reads() -> [PartialRead; 2] {
    [
        (
            Op::Read(Read::File { head: HEAD, path: Box::from(&b"x"[..]), max_bytes: RAW_BYTES }),
            &[(206, b"abc")],
            Answer::File { head: HEAD, path: Box::from(&b"x"[..]), bytes: Box::from(&b"abc"[..]), truncated: false },
        ),
        (
            Op::Read(Read::Job { attempt: ATTEMPT, max_bytes: RAW_BYTES }),
            &[(200, RUN), (200, JOB), (206, b"abc")],
            Answer::Job {
                attempt: ATTEMPT,
                step: Some(api::Step { number: STEP_NUMBER, name: Box::from(&b"x"[..]) }),
                log: Box::from(&b"abc"[..]),
                truncated: false,
            },
        ),
    ]
}

use crate::forgejo::{
    decode::Malformed,
    limits::Limits,
    plans::{Body as AnswerBody, Heard},
};
use fixtures::BOUNDS;
// This response exceeds the local inner bound of two labels by one (forge.md 5.3).
const INNER_BOUND: u32 = 2;
const THREE_LABELS: &[u8] = br#"{"number":1,"state":"open","user":{"id":2,"login":"writer"},"labels":[{"name":"a"},{"name":"b"},{"name":"c"}],"updated_at":"1970-01-01T00:00:00Z","title":"t","body":"b"}"#;
fn bounded_run(op: Op) -> Run {
    let mut run = Run::with_limits(Limits { inner: INNER_BOUND, ..fixtures::limits() }, BOUNDS, op);
    run.begin();
    run
}
fn inner_overflow(run: &mut Run) -> Option<Step> {
    run.hear(Heard::Head { status: 200, fields: fixtures::fields() });
    run.hear(Heard::Done(AnswerBody::Malformed(Malformed::Bound { field: b"labels" })))
}

#[test]
fn write_inner_overflow_is_timeout() {
    let mut run = bounded_run(Op::Write(write_cases::create_issue()));
    assert_eq!(run.answer(201, THREE_LABELS), Step::Done(Err(Error::Timeout)));
}

#[test]
fn read_inner_overflow_is_too_large() {
    let mut run = bounded_run(Op::Read(Read::Item { number: ITEM_NUMBER, after: 0 }));
    assert_eq!(run.answer(200, THREE_LABELS), Step::Done(Err(Error::TooLarge)));
    for read in [Read::PullFiles { number: ITEM_NUMBER, head: HEAD, page: 1 }, Read::Checks { commit: HEAD }] {
        let mut run = bounded_run(Op::Read(read));
        assert_eq!(inner_overflow(&mut run), Some(Step::Done(Err(Error::TooLarge))));
    }
}

#[test]
fn open_pull_inner_overflow_is_too_large() {
    let mut run = bounded_run(Op::Write(write_cases::open_pull()));
    assert_eq!(inner_overflow(&mut run), Some(Step::Done(Err(Error::TooLarge))));
}

#[test]
fn log_body_overflow_is_invalid_answer() {
    let mut run = Run::known(Op::Read(Read::Job { attempt: ATTEMPT, max_bytes: RAW_BYTES }));
    replies(&mut run, &[(200, RUN), (200, JOB)]);
    run.hear(Heard::Head { status: 200, fields: fixtures::fields() });
    assert_eq!(
        run.hear(Heard::Done(AnswerBody::Malformed(Malformed::Bound { field: b"log" }))),
        Some(Step::Done(Err(Error::InvalidAnswer)))
    );
}

const FILE_PATH: &[u8] = b"x";
const SHORT_RAW: &[u8] = b"abc";
const FULL_RAW: &[u8] = b"abcd";
const JOB_TAIL: &[u8] = b"bcd";
const CHANGED_FILES: &[u8] = br#"[{"filename":"x","status":"changed","previous_filename":""}]"#;
const CHANGED_COMPARE: &[u8] =
    br#"{"total_commits":0,"commits":[],"files":[{"filename":"x","status":"changed","previous_filename":""}]}"#;
fn raw_body(run: &mut Run, bytes: &[u8], more: bool) -> Option<Step> {
    run.hear(Heard::Head { status: 200, fields: fixtures::fields() });
    run.hear(Heard::Done(AnswerBody::Raw { bytes: Box::from(bytes), more }))
}

#[test]
fn file_cuts_to_remaining_answer_room() {
    for (bytes, more) in [(FULL_RAW, false), (SHORT_RAW, true)] {
        let mut run = Run::with(
            super::super::plan::Bounds { answer_bytes: RAW_BYTES, ..BOUNDS },
            Op::Read(Read::File { head: HEAD, path: Box::from(FILE_PATH), max_bytes: RAW_BYTES }),
        );
        run.begin();
        assert_eq!(
            raw_body(&mut run, bytes, more),
            Some(Step::Done(Ok(Answer::File {
                head: HEAD,
                path: Box::from(FILE_PATH),
                bytes: Box::from(SHORT_RAW),
                truncated: true,
            })))
        );
    }
}

#[test]
fn job_cuts_to_remaining_answer_room() {
    let mut run = Run::with(
        super::super::plan::Bounds { answer_bytes: RAW_BYTES, ..BOUNDS },
        Op::Read(Read::Job { attempt: ATTEMPT, max_bytes: RAW_BYTES }),
    );
    replies(&mut run, &[(200, RUN), (200, JOB)]);
    assert_eq!(
        raw_body(&mut run, JOB_TAIL, true),
        Some(Step::Done(Ok(Answer::Job {
            attempt: ATTEMPT,
            step: Some(api::Step { number: STEP_NUMBER, name: Box::from(FILE_PATH) }),
            log: Box::from(JOB_TAIL),
            truncated: true,
        })))
    );
}

#[test]
fn file_sides_past_remaining_room_are_too_large() {
    // The one-file metadata and its path fit, leaving exactly SHORT_RAW's three bytes.
    let answer_bytes =
        u32::try_from(crate::measure::FILE_BYTES.saturating_add(FILE_PATH.len()).saturating_add(SHORT_RAW.len()))
            .unwrap();
    for (read, answers, sends) in side_cases() {
        let mut run = Run::with(super::super::plan::Bounds { answer_bytes, ..BOUNDS }, Op::Read(read));
        replies(&mut run, answers);
        assert_eq!(run.answer(200, FULL_RAW), Step::Done(Err(Error::TooLarge)));
        run.assert_sent(sends);
    }
}

type SideCase = (Read, &'static [(u16, &'static [u8])], &'static [&'static str]);
fn side_cases() -> [SideCase; 2] {
    [
        (
            Read::PullFiles { number: ITEM_NUMBER, head: HEAD, page: 1 },
            &[(200, PULL), (200, CHANGED_FILES)],
            &[
                "GET /api/v1/repos/fixture/specimen/pulls/1",
                "GET /api/v1/repos/fixture/specimen/pulls/1/files?page=1&limit=2",
                "GET /api/v1/repos/fixture/specimen/raw/x?ref=0300000000000000000000000000000000000000",
            ],
        ),
        (
            Read::Compare { before: fixtures::BASE, after: HEAD },
            &[(200, CHANGED_COMPARE), (200, fixtures::EMPTY_COMPARE)],
            &[
                "GET /api/v1/repos/fixture/specimen/compare/0300000000000000000000000000000000000000...0100000000000000000000000000000000000000?verification=false",
                "GET /api/v1/repos/fixture/specimen/compare/0100000000000000000000000000000000000000...0300000000000000000000000000000000000000?files=false&verification=false",
                "GET /api/v1/repos/fixture/specimen/raw/x?ref=0300000000000000000000000000000000000000",
            ],
        ),
    ]
}

// The asked id differs from the response's id; owner 2 is REPOSITORY's fixture owner.
const ASKED_REPOSITORY: api::Repository = api::Repository { forge: REPO.forge, repository: 7 };
const ANSWERED_REPOSITORY: api::Repository = api::Repository { forge: REPO.forge, repository: 8 };
const OWNER_ID: u64 = 2;
const OTHER_REPOSITORY: &[u8] = br#"{"id":8,"owner":{"id":2,"login":"fixture"},"name":"specimen","object_format_name":"sha1","default_branch":"main","default_merge_style":"squash","allow_merge_commits":true,"allow_squash_merge":false,"allow_rebase":false,"html_url":"http://127.0.0.1:3000/fixture/specimen"}"#;
fn unresolved(op: Op) -> Run {
    let mut run = Run::at_repository(ASKED_REPOSITORY, fixtures::limits(), BOUNDS, op);
    run.begin();
    run
}
fn assert_no_repository(run: &mut Run) {
    assert_eq!(run.tables().repository(ASKED_REPOSITORY), None);
    assert_eq!(run.tables().repository(ANSWERED_REPOSITORY), None);
    assert_eq!(run.tables().user(REPO.forge, OWNER_ID), None);
}

#[test]
fn repository_id_mismatch_is_invalid_answer() {
    for read in [Read::Branches { prefix: Box::from(&b"m"[..]) }, Read::Settings] {
        let mut run = unresolved(Op::Read(read));
        assert_eq!(run.answer(200, OTHER_REPOSITORY), Step::Done(Err(Error::InvalidAnswer)));
        assert_no_repository(&mut run);
        run.assert_sent(&["GET /api/v1/repositories/7"]);
    }
}

#[test]
fn write_repository_mismatch_is_timeout() {
    let mut run = unresolved(Op::Write(write_cases::create_issue()));
    assert_eq!(run.answer(200, OTHER_REPOSITORY), Step::Done(Err(Error::Timeout)));
    assert_no_repository(&mut run);
    run.assert_sent(&["GET /api/v1/repositories/7"]);
}

#[test]
fn open_pull_repository_mismatch_is_unavailable() {
    let mut run = unresolved(Op::Write(write_cases::open_pull()));
    assert_eq!(run.answer(200, OTHER_REPOSITORY), Step::Done(Err(Error::Unavailable)));
    assert_no_repository(&mut run);
    run.assert_sent(&["GET /api/v1/repositories/7"]);
}

#[test]
fn locate_accepts_a_nonzero_repository_id() {
    let mut run =
        unresolved(Op::Read(Read::Locate { owner: Box::from(&b"fixture"[..]), name: Box::from(&b"specimen"[..]) }));
    assert_eq!(run.answer(200, OTHER_REPOSITORY), Step::Done(Ok(Answer::Located { repository: 8 })));
    assert_eq!(run.tables().repository(ANSWERED_REPOSITORY), Some(&fixtures::specimen()));
    assert_eq!(run.tables().user(REPO.forge, OWNER_ID), Some(&b"fixture"[..]));
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen"]);
}

#[test]
fn branch_prefix_uses_one_refs_request_for_two_matches() {
    // The refs route returns only matches; unrelated repository branches have no rows here.
    let mut run = Run::with(BOUNDS, Op::Read(Read::Branches { prefix: Box::from(&b"temper/"[..]) }));
    run.begin();
    let body = br#"[{"ref":"refs/heads/temper/one","object":{"type":"commit","sha":"0100000000000000000000000000000000000000"}},{"ref":"refs/heads/temper/two","object":{"type":"commit","sha":"0100000000000000000000000000000000000000"}}]"#;
    assert_eq!(
        run.answer(200, body),
        Step::Done(Ok(Answer::Branches(Box::from([Box::from(&b"temper/one"[..]), Box::from(&b"temper/two"[..]),]))))
    );
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/git/refs/heads/temper%2F"]);
}

#[test]
fn branch_prefix_past_eight_matches_is_too_large() {
    let mut run = Run::with(
        crate::forgejo::plans::plan::Bounds { rows: 8, ..BOUNDS },
        Op::Read(Read::Branches { prefix: Box::from(&b"temper/"[..]) }),
    );
    run.begin();
    let length = br#"{"ref":"refs/heads/temper/0"}"#.len().checked_mul(9).unwrap().checked_add(10).unwrap();
    let mut body = skein_lib::Writer::new(length);
    body.put(b"[").unwrap();
    for index in 0_u64..9 {
        if index != 0 {
            body.put(b",").unwrap();
        }
        body.put(br#"{"ref":"refs/heads/temper/"#).unwrap();
        body.put(skein_lib::Decimal::of(index).as_bytes()).unwrap();
        body.put(br#""}"#).unwrap();
    }
    body.put(b"]").unwrap();
    assert_eq!(run.answer(200, &body.finish()), Step::Done(Err(Error::TooLarge)));
}

#[test]
fn empty_branch_prefix_is_named_empty_before_repository_resolution() {
    let mut run = Run::new(Op::Read(Read::Branches { prefix: Box::new([]) }));
    assert_eq!(run.begin(), Step::Done(Err(Error::Empty)));
    run.assert_sent(&[]);
}

#[test]
fn missing_branch_prefix_is_an_empty_answer_and_keeps_the_repository() {
    let mut run = Run::known(Op::Read(Read::Branches { prefix: Box::from(&b"temper/"[..]) }));
    run.begin();
    assert_eq!(run.answer(404, b"{}"), Step::Done(Ok(Answer::Branches(Box::new([])))));
    assert_eq!(run.tables().repository(REPO), Some(&fixtures::specimen()));
}

#[test]
fn refs_namespace_does_not_reduce_the_branch_name_bound() {
    for name in [&[b'x'; 64][..], &[b'x'; 65][..]] {
        let mut limits = fixtures::limits();
        limits.string_bytes = 64;
        limits.json.string = 75;
        let mut run = Run::with_limits(limits, BOUNDS, Op::Read(Read::Branches { prefix: Box::from(&b"x"[..]) }));
        run.begin();
        let length =
            br#"[{"ref":"refs/heads/"#.len().checked_add(name.len()).unwrap().checked_add(br#""}]"#.len()).unwrap();
        let mut body = skein_lib::Writer::new(length);
        body.put(br#"[{"ref":"refs/heads/"#).unwrap();
        body.put(name).unwrap();
        body.put(br#""}]"#).unwrap();
        let expected = if name.len() == 64 {
            Ok(Answer::Branches(Box::new([Box::from(name)])))
        } else {
            Err(Error::InvalidAnswer)
        };
        assert_eq!(run.answer(200, &body.finish()), Step::Done(expected));
    }
}
