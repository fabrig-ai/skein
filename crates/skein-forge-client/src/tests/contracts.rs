//! Contracts at the owner's boundary (forge-client.md, sections 2 and 7).
use super::{LIMITS, configured};
use crate::{Domain, Limits};

const KEY_BYTES: u32 = 7; // version + two length bytes + kit + one identity byte
const NAMESPACE_BYTES: u32 = 65535; // namespace length field
const MAX_KEY: u32 = 65539; // version + length + largest namespace + identity
const PREFIX_BYTES: usize = 3; // version and two namespace-length bytes
const REVIEWERS: [u64; 2] = [7, 8]; // two distinct reviewer identities
const REVIEWER_BYTES: u32 = 16; // two protocol u64 reviewer slots
const BRANCH_BYTES: u32 = 3; // kit
const STAMP_BYTES: u64 = 35; // kit + comment and echo's two 16-byte stamps
const REVIEW_BYTES: u64 = 51; // STAMP_BYTES + one 16-byte review stamp
const LABEL_BYTES: u64 = 70; // REVIEW_BYTES + label's 16-byte slot + kit
const CACHED_PULL_BYTES: u64 = 92; // LABEL_BYTES + kit head/base + two u64 reviewers
const ITEMS_BYTES: u32 = 131; // 128-byte summary slot + kit title
const ITEM_BYTES: u32 = 100; // 96-byte comment slot + kit body + one-byte key
const PULL_BYTES: u32 = 22; // kit head/base + two u64 reviewers
const JOB_BYTES: u32 = 5; // kit step name + ab log
const JOB_LOG_ROOM: u32 = 4; // job request's four-byte log allowance
const ROW_BOUND: u32 = 4; // LIMITS.rows: four rows per answer
const OVER_ROWS: u32 = 5; // one past ROW_BOUND
const ROW_BYTE_ROOM: u32 = 83; // five 16-byte context slots + kit branch
const NO_ROWS: u32 = 0; // scalar answers carry no rows
const OVER_WATCHES: u32 = 5; // one past LIMITS.resources (four)
const OVER_BRANCH_BYTES: usize = 257; // one past LIMITS.op_bytes (256)
const DEPLOYMENT: [u8; 16] = [7; 16];

#[test]
fn keys_frame_the_namespace_and_opaque_identity() {
    let key = crate::effect_key(b"kit", b"a", KEY_BYTES);
    assert_eq!(key.as_deref(), Some(b"\x01\x00\x03kita".as_slice()));
}

#[test]
fn invalid_key_frames_are_refused() {
    const OVERLONG: usize = 65536; // one past the u16 namespace length
    let long = skein_lib::bytes::zeroed(OVERLONG);
    for (name, namespace, identity, max) in [
        ("empty namespace", b"".as_slice(), b"a".as_slice(), KEY_BYTES),
        ("long namespace", long.as_ref(), b"a", u32::MAX),
        ("empty identity", b"kit", b"", KEY_BYTES),
        ("short cap", b"kit", b"a", KEY_BYTES - 1),
    ] {
        assert_eq!(crate::effect_key(namespace, identity, max), None, "{name}");
    }
}

#[test]
fn the_largest_namespace_keeps_its_identity() {
    const REPEATS: usize = 21845; // 65535 bytes / kit's three bytes
    let mut namespace = skein_lib::List::with_capacity(NAMESPACE_BYTES);
    for _ in 0..REPEATS {
        for byte in b"kit" {
            namespace.push(*byte).expect("largest namespace room");
        }
    }
    let key = crate::effect_key(namespace.as_slice(), b"a", MAX_KEY).expect("largest namespace fits");
    assert_eq!(key.len(), usize::try_from(MAX_KEY).expect("key bound fits usize"));
    assert_eq!(key.get(..PREFIX_BYTES), Some([1, 255, 255].as_slice()));
    assert_eq!(key.last(), Some(&b'a'));
}

#[test]
fn opaque_identities_are_stable_and_distinct() {
    let first = crate::effect_key(b"kit", b"a", KEY_BYTES);
    assert_eq!(first.as_deref(), Some(b"\x01\x00\x03kita".as_slice()));
    assert_eq!(first, crate::effect_key(b"kit", b"a", KEY_BYTES));
    assert_ne!(first, crate::effect_key(b"kit", b"b", KEY_BYTES));
    assert_ne!(first, crate::effect_key(b"b", b"a", KEY_BYTES));
}

#[test]
fn binding_replaces_the_namespace_before_restore() {
    let mut h = Harness::new(LIMITS);
    h.domain = configured(&LIMITS);
    assert!(h.step(Event::Restored { clock: crate::RecoveryClock::Monotonic }).is_empty());
    let mut d = h.domain;
    assert!(d.bind_deployment(DEPLOYMENT, &LIMITS));
    let key = crate::effect_key(&DEPLOYMENT, b"a", LIMITS.op_bytes).expect("deployment frame fits");
    assert!(crate::identity::historical(&d, &key));
}

#[test]
fn binding_without_configuration_changes_nothing() {
    let mut d = Domain::new(&LIMITS);
    assert!(!d.bind_deployment(DEPLOYMENT, &LIMITS));
    assert_eq!(d.config, None);
}

#[test]
fn binding_past_op_bytes_preserves_the_namespace() {
    const KIT_BYTES: u32 = 3; // namespace kit, smaller than the deployment id
    let mut d = configured(&LIMITS);
    assert!(!d.bind_deployment(DEPLOYMENT, &Limits { op_bytes: KIT_BYTES, ..LIMITS }));
    assert_eq!(d.config.expect("configured").namespace.as_ref(), b"kit");
}

use super::{Harness, REPO, at};
use crate::{Cached, Condition, Delivery, Echo, Effect, Event, RepositoryRecord, Request, Stored};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Read, Write};
use skein_lib::Token;

fn stamped_row() -> crate::LiveRecord {
    let mut row = super::fixtures::keep::measure_row();
    row.comments = Box::new([Delivery { id: 1, revision: 1 }]);
    row.echoes = Box::new([Echo::Comment(1)]);
    row
}

fn reviewed_row() -> crate::LiveRecord {
    let mut row = stamped_row();
    row.reviews = Box::new([Delivery { id: 2, revision: 1 }]);
    row
}

fn labelled_row() -> crate::LiveRecord {
    let mut row = reviewed_row();
    let mut item = empty_summary();
    item.labels = Box::new([Box::from(&b"kit"[..])]);
    row.cached = Box::new(Cached { item: Some(item), pull: None, tip: None });
    row
}

#[test]
fn repository_measure_has_no_payload() {
    let repository = Stored::Repository(RepositoryRecord { repository: REPO, mark: None, clock: at(100) });
    assert_eq!(crate::measure::stored(&repository), Some(0));
}

#[test]
fn branch_measure_counts_its_name() {
    let row = super::fixtures::keep::measure_row();
    assert_eq!(crate::measure::stored(&Stored::Live(row)), Some(u64::from(BRANCH_BYTES)));
}

#[test]
fn stored_measure_counts_comment_and_echo_stamps() {
    assert_eq!(crate::measure::stored(&Stored::Live(stamped_row())), Some(STAMP_BYTES));
}

#[test]
fn stored_measure_counts_review_stamps() {
    assert_eq!(crate::measure::stored(&Stored::Live(reviewed_row())), Some(REVIEW_BYTES));
}

#[test]
fn stored_measure_counts_cached_label_width() {
    assert_eq!(crate::measure::stored(&Stored::Live(labelled_row())), Some(LABEL_BYTES));
}

#[test]
fn stored_measure_counts_cached_pull_reviewers() {
    let mut row = labelled_row();
    row.cached.pull = Some(super::fixtures::keep::reviewed_pull(Box::new(REVIEWERS)));
    assert_eq!(crate::measure::stored(&Stored::Live(row)), Some(CACHED_PULL_BYTES));
}

#[test]
fn effect_measure_uses_fixed_widths() {
    const OPEN_PULL_BYTES: u32 = 6; // kit head and kit base
    for (name, write, condition, expected) in [
        (
            "reviewers",
            Write::SetReviewers { number: 1, reviewers: Box::new(REVIEWERS) },
            Condition::None,
            REVIEWER_BYTES,
        ),
        (
            "pull",
            Write::OpenPull {
                title: Box::new([]),
                body: Box::new([]),
                head: Box::from(&b"kit"[..]),
                base: Box::from(&b"kit"[..]),
            },
            Condition::None,
            OPEN_PULL_BYTES,
        ),
        (
            "merge",
            Write::Merge { number: 1, head: [1; 32], how: api::MergeHow::Style },
            Condition::Merge { base: Box::from(&b"kit"[..]) },
            BRANCH_BYTES,
        ),
    ] {
        assert_eq!(crate::measure::effect(&Effect { write, condition }), Some(u64::from(expected)), "{name}");
    }
}

fn empty_summary() -> api::Summary {
    let mut item = super::fixtures::keep::summary();
    item.title = Box::new([]);
    item
}

fn items_answer() -> Answer {
    let mut item = empty_summary();
    item.title = Box::from(&b"kit"[..]);
    Answer::Items { items: Box::new([item]), more: false, now: at(100) }
}

fn item_answer() -> Answer {
    let mut comment = super::fixtures::keep::comment(1);
    comment.body = Box::from(&b"kit"[..]);
    comment.key = Some(Box::from(&b"a"[..]));
    Answer::Item { item: empty_summary(), comments: Box::new([comment]), more: false }
}

fn pull_answer() -> Answer {
    Answer::Pull(super::fixtures::keep::reviewed_pull(Box::new(REVIEWERS)))
}

const ATTEMPT: api::JobAttempt = api::JobAttempt { head: [1; 32], run: 1, job: 1, attempt: 1 };
fn job_answer() -> Answer {
    Answer::Job {
        attempt: ATTEMPT,
        step: Some(api::Step { number: 1, name: Box::from(&b"kit"[..]) }),
        log: Box::from(&b"ab"[..]),
        truncated: false,
    }
}

#[test]
fn items_measure_uses_fixed_summary_width() {
    assert_eq!(skein_forge::measure::answer(&items_answer()), Some(u64::from(ITEMS_BYTES)));
}

#[test]
fn item_measure_uses_fixed_comment_width() {
    assert_eq!(skein_forge::measure::answer(&item_answer()), Some(u64::from(ITEM_BYTES)));
}

#[test]
fn pull_measure_uses_fixed_reviewer_width() {
    assert_eq!(skein_forge::measure::answer(&pull_answer()), Some(u64::from(PULL_BYTES)));
}

#[test]
fn job_measure_counts_step_and_log() {
    assert_eq!(skein_forge::measure::answer(&job_answer()), Some(u64::from(JOB_BYTES)));
}

const BYTE_CASES: usize = 4; // items, item, pull, job
type ByteCase = (&'static str, Read, fn() -> Answer, u32);
fn byte_cases() -> [ByteCase; BYTE_CASES] {
    [
        ("items", Read::Items { since: at(0), page: 1, kind: None }, items_answer, ITEMS_BYTES),
        ("item", Read::Item { number: 9, after: 0 }, item_answer, ITEM_BYTES),
        ("pull", Read::Pull { number: 9 }, pull_answer, PULL_BYTES),
        ("job", Read::Job { attempt: ATTEMPT, max_bytes: JOB_LOG_ROOM }, job_answer, JOB_BYTES),
    ]
}

fn admitted(read: Read, answer: Answer, cap: u32) -> Box<[Request]> {
    let mut h = Harness::new(Limits { answer_bytes: cap, ..LIMITS });
    h.step(Event::Read { owner: Token::new(1), repository: REPO, read });
    let sent = h.one();
    h.answer(&sent, 1, Ok(answer))
}

#[test]
fn answer_bytes_at_bound_are_admitted() {
    for (name, read, answer, bytes) in byte_cases() {
        let answer = answer();
        let expected = Request::Read { owner: Token::new(1), result: Ok(answer.clone()) };
        assert_eq!(admitted(read, answer, bytes).as_ref(), &[expected], "{name}");
    }
}

#[test]
fn answer_bytes_past_bound_are_refused() {
    for (name, read, answer, bytes) in byte_cases() {
        let answer = answer();
        let expected = Request::Read { owner: Token::new(1), result: Err(Error::TooLarge) };
        assert_eq!(admitted(read, answer, bytes - 1).as_ref(), &[expected], "{name}");
    }
}

fn empty_rows(rows: u32) -> Box<[Box<[u8]>]> {
    let mut values = skein_lib::List::with_capacity(rows);
    for _ in 0..rows {
        values.push(Box::from([])).expect("row fixture fits");
    }
    values.into_boxed()
}

fn branch_rows(rows: u32) -> Box<[Box<[u8]>]> {
    let mut names = skein_lib::List::with_capacity(rows);
    for _ in 0..rows {
        names.push(Box::from(&b"main"[..])).expect("fixture rows");
    }
    names.into_boxed()
}

const ROW_CASES: usize = 4; // branches, labels, reviewers, protection contexts
fn row_reads() -> [(&'static str, Read); ROW_CASES] {
    [
        ("branches", Read::Branches { prefix: Box::from(&b"m"[..]) }),
        ("labels", Read::Item { number: 9, after: 0 }),
        ("reviewers", Read::Pull { number: 9 }),
        ("protection", Read::Protection { branch: Box::from(&b"kit"[..]) }),
    ]
}
fn row_cases(rows: u32) -> [(&'static str, Read, Answer); ROW_CASES] {
    let mut item = empty_summary();
    item.labels = empty_rows(rows);
    let mut reviewers = skein_lib::List::with_capacity(rows);
    for reviewer in 1..=u64::from(rows) {
        reviewers.push(reviewer).expect("reviewer fixture fits");
    }
    let answers = [
        Answer::Branches(branch_rows(rows)),
        Answer::Item { item, comments: Box::new([]), more: false },
        Answer::Pull(super::fixtures::keep::reviewed_pull(reviewers.into_boxed())),
        Answer::Protection(Some(api::Protection {
            queue: false,
            signatures: false,
            linear: false,
            code_owners: false,
            bypass: api::Bypass::Never,
            branch: Box::from(&b"kit"[..]),
            contexts: empty_rows(rows),
            approvals: 0,
            dismiss_stale: false,
        })),
    ];
    let mut answers = answers.into_iter();
    row_reads().map(|(name, read)| (name, read, answers.next().expect("one answer per row case")))
}

#[test]
fn answer_rows_at_bound_fit() {
    for (name, _, answer) in row_cases(ROW_BOUND) {
        assert!(crate::measure::answer_rows(&answer, ROW_BOUND), "{name}");
    }
}

#[test]
fn answer_rows_past_bound_do_not_fit() {
    for (name, _, answer) in row_cases(OVER_ROWS) {
        assert!(!crate::measure::answer_rows(&answer, ROW_BOUND), "{name}");
    }
}

#[test]
fn row_answers_at_bound_are_admitted() {
    for (name, read, answer) in row_cases(ROW_BOUND) {
        let expected = Request::Read { owner: Token::new(1), result: Ok(answer.clone()) };
        assert_eq!(admitted(read, answer, ROW_BYTE_ROOM).as_ref(), &[expected], "{name}");
    }
}

#[test]
fn row_answers_past_bound_are_refused() {
    for (name, read, answer) in row_cases(OVER_ROWS) {
        let expected = Request::Read { owner: Token::new(1), result: Err(Error::TooLarge) };
        assert_eq!(admitted(read, answer, ROW_BYTE_ROOM).as_ref(), &[expected], "{name}");
    }
}

#[test]
fn scalar_answers_need_no_row_room() {
    assert!(crate::measure::answer_rows(&Answer::Done, NO_ROWS));
}

#[test]
fn scheduling_reads_emit_no_terminal() {
    for (name, read, _, bytes) in byte_cases() {
        for cap in [bytes, bytes - 1] {
            let mut h = Harness::new(Limits { answer_bytes: cap, ..LIMITS });
            assert!(
                h.step(Event::Read { owner: Token::new(1), repository: REPO, read: read.clone() }).is_empty(),
                "{name} {cap}"
            );
        }
    }
    for (name, read) in row_reads() {
        let mut h = Harness::new(LIMITS);
        assert!(h.step(Event::Read { owner: Token::new(1), repository: REPO, read }).is_empty(), "{name}");
    }
}

#[test]
fn keep_count_refusal_precedes_restore_busy() {
    let mut h = Harness::new(LIMITS);
    let mut watches = skein_lib::List::with_capacity(OVER_WATCHES);
    for number in 0..u64::from(OVER_WATCHES) {
        watches
            .push(crate::Watch {
                resource: crate::Resource { repository: REPO, what: crate::What::Issue(number) },
                participating: false,
            })
            .expect("five watches");
    }
    let watches = watches.into_boxed();
    assert_eq!(
        h.step(Event::Keep { owner: Token::new(1), watches }).as_ref(),
        &[Request::Kept { owner: Token::new(1), result: Err(Error::TooLarge) }]
    );
}

#[test]
fn make_unready_precedes_missing_writer() {
    let mut h = Harness::new(LIMITS);
    let entry = super::fixtures::entry(Write::Close { number: 9 });
    assert_eq!(
        h.step(Event::Make { entry }).as_ref(),
        &[Request::Outcome { entry: 1, task: 7, outcome: crate::Outcome::Failed(Error::Busy) }]
    );
}

#[test]
fn read_size_refusal_precedes_full_slab() {
    let mut h = Harness::new(LIMITS);
    for owner in 1..=8 {
        h.read(owner);
    }
    assert!(!h.domain.has_room());
    let got = h.step(Event::Read {
        owner: Token::new(9),
        repository: REPO,
        read: Read::Branch { branch: Box::new([b'a'; OVER_BRANCH_BYTES]) },
    });
    assert_eq!(got.as_ref(), &[Request::Read { owner: Token::new(9), result: Err(Error::TooLarge) }]);
}

#[test]
fn binding_after_identity_use_preserves_the_namespace() {
    let row = super::fixtures::keep::measure_row();
    let entry = super::fixtures::entry(Write::Close { number: 9 });
    for event in [
        Event::Restore { record: Stored::Live(row) },
        Event::Restore {
            record: Stored::Repository(RepositoryRecord { repository: REPO, mark: None, clock: at(100) }),
        },
        Event::Keep { owner: Token::new(1), watches: Box::new([]) },
        Event::Make { entry: entry.clone() },
        Event::Remake { entry },
    ] {
        let mut h = Harness::new(LIMITS);
        h.domain = configured(&LIMITS);
        h.step(event);
        assert!(!h.domain.bind_deployment(DEPLOYMENT, &LIMITS));
        assert_eq!(h.domain.config.as_ref().expect("configured").namespace.as_ref(), b"kit");
    }
}

#[test]
fn an_answered_busy_is_an_invalid_read_answer() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    let sent = h.one();
    assert_eq!(
        h.answer(&sent, 1, Err(Error::Busy)).as_ref(),
        &[Request::Read { owner: Token::new(1), result: Err(Error::InvalidAnswer) }]
    );
    assert!(h.facts().contains(&crate::Fact::Failed { priority: crate::Priority::Fresh, error: Error::InvalidAnswer }));
}

#[test]
fn an_answered_busy_makes_a_write_uncertain() {
    let mut h = super::fixtures::outbox::Harness::new();
    h.make(1, super::fixtures::outbox::comment(), Condition::None);
    h.clock();
    let write = h.one();
    h.answer(write, Err(Error::Busy));
    assert_eq!(h.store.get(&1).expect("progress saved").failures, 1);
    assert_eq!(h.outcomes.as_slice(), &[(1, crate::Outcome::Uncertain)]);
    assert_eq!(h.one().op, api::Op::Read(Read::Item { number: 9, after: 0 }));
    assert_eq!(h.writes, 1);
}

#[test]
fn branch_prefix_round_trips_in_the_call_and_rejects_foreign_answers() {
    let mut h = Harness::new(LIMITS);
    let read = Read::Branches { prefix: Box::from(&b"temper/"[..]) };
    h.step(Event::Read { owner: Token::new(1), repository: REPO, read: read.clone() });
    let sent = h.one();
    let op = &sent.op;
    assert_eq!(op, &api::Op::Read(read));
    assert_eq!(skein_forge::measure::op(op), Some(7));
    assert_eq!(
        h.answer(&sent, 1, Ok(Answer::Branches(Box::new([Box::from(&b"foreign/main"[..])])))).as_ref(),
        &[Request::Read { owner: Token::new(1), result: Err(Error::InvalidAnswer) }]
    );
}

#[test]
fn an_empty_branch_prefix_is_named_empty_without_a_call() {
    let mut h = Harness::new(LIMITS);
    assert_eq!(
        h.step(Event::Read { owner: Token::new(1), repository: REPO, read: Read::Branches { prefix: Box::new([]) } })
            .as_ref(),
        &[Request::Read { owner: Token::new(1), result: Err(Error::Empty) }]
    );
}
