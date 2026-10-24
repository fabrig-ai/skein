use super::fixtures::keep::{
    Harness, branch, comment, current, issue, matches_write, participation, pull, review, summary, watch,
};
use super::{LIMITS, REPO, at};
use crate::{Condition, Effect, Entry, Event, Key, Limits, Made, Outcome, Resource, Stored, What};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Change, Error, Op, Read, Write};
use skein_lib::{List, Token};
#[test]
fn watch_batches_are_atomic_and_leaving_an_outstanding_resource_consumes_its_terminal() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Item { number: 9, after: 0 }));
    let before = h.records.len();
    h.keep(Box::new([watch(issue(), true), watch(issue(), true)]));
    assert_eq!(h.kept.as_slice().last(), Some(&Err(Error::Refused)));
    assert_eq!(h.records.len(), before);
    h.keep(Box::new([]));
    assert!(h.domain.cached(&issue()).is_none());
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(1)]), more: false });
    assert!(h.news.is_empty());
    assert!(h.records.is_empty());
    assert!(!h.domain.is_ready());
}
#[test]
fn a_full_batch_is_refused_before_any_partial_admission() {
    let mut h = Harness::new();
    h.env.limits.resources = 1;
    h.keep(Box::new([watch(issue(), true), watch(branch(), false)]));
    assert_eq!(h.kept.as_slice(), &[Err(Error::TooLarge)]);
    assert!(h.records.is_empty());
    assert!(!h.domain.is_ready());
}
#[test]
fn owned_pull_keeps_ci_and_base_on_backoff_without_reading_forge_reviews() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(pull(), false)]));
    h.clock();
    let item = h.one();
    assert_eq!(item.op, Op::Read(Read::Item { number: 9, after: u64::MAX }));
    h.answer(item, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Pull { number: 9 }));
    h.answer(read, Answer::Pull(current()));
    assert!(!h.domain.is_ready());
    h.fire(105);
    let listing = h.one();
    assert_eq!(listing.op, Op::Read(Read::Items { since: at(100), page: 1, kind: None }));
    h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(105) });
    let item = h.one();
    h.answer(item, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let ci = h.one();
    let mut failed = current();
    failed.ci = api::Ci::Failed;
    h.answer(ci, Answer::Pull(failed));
    assert_eq!(h.domain.cached(&pull()).expect("pull kept").pull.as_ref().expect("pull read").ci, api::Ci::Failed);
}
#[test]
fn own_pushes_and_held_writer_hints_are_not_drift_but_a_later_external_tip_is() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(branch(), false)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Commit([1; 32]));
    h.event(Event::Writer { resource: branch(), taken: true });
    h.event(Event::Hint {
        hint: api::Hint { repository: REPO, change: Change::Branch(Box::from(&b"topic"[..])), key: None },
    });
    h.event(Event::Pushed { resource: branch(), commit: [2; 32] });
    h.event(Event::Writer { resource: branch(), taken: false });
    let read = h.one();
    h.answer(read, Answer::Commit([2; 32]));
    assert_eq!(h.drift, 0);
    h.event(Event::Hint {
        hint: api::Hint { repository: REPO, change: Change::Branch(Box::from(&b"topic"[..])), key: None },
    });
    h.fire(101);
    let listing = h.one();
    h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
    let read = h.one();
    h.answer(read, Answer::Commit([3; 32]));
    assert_eq!(h.drift, 1);
}
#[test]
fn own_comment_echo_never_skips_an_intervening_unread_human_comment() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(5)]), more: false });
    h.news.clear();
    h.event(Event::Make {
        entry: Entry {
            number: 1,
            task: 7,
            repository: REPO,
            effect: Effect {
                write: Write::Post { number: 9, key: Box::from(&b"own-key"[..]), body: Box::from(&b"done"[..]) },
                condition: Condition::None,
            },
            start: None,
            attempt: None,
            failures: 0,
            queued: false,
        },
    });
    h.clock();
    let write = h.one();
    h.answer(write, Answer::Commented(7));
    assert_eq!(h.outcomes.as_slice(), &[Outcome::Made { made: Made::Commented(7), found: false }]);
    assert_eq!(h.record(&issue()).position.comment, 5);
    h.event(Event::Hint { hint: api::Hint { repository: REPO, change: Change::Item(9), key: None } });
    h.fire(101);
    let listing = h.one();
    h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Item { number: 9, after: 0 }));
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(6), comment(7)]), more: false });
    assert_eq!(h.record(&issue()).position.comment, 7);
    assert!(h.record(&issue()).echoes.is_empty());
    match h.news.as_slice() {
        [Answer::Item { comments, .. }] => {
            assert_eq!(comments.len(), 1);
            assert_eq!(comments[0].id, 6);
        }
        _ => panic!("one human comment delivered"),
    }
}
#[test]
fn time_paging_keeps_the_tie_replay_point_when_an_item_moves_during_the_pass() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(Resource { repository: REPO, what: What::Repository }, false)]));
    h.clock();
    h.fire(105);
    let first = h.one();
    assert_eq!(first.op, Op::Read(Read::Items { since: at(100), page: 1, kind: None }));
    h.answer(first, Answer::Items { items: Box::new([summary()]), more: true, now: at(105) });
    let tied = h.one();
    assert_eq!(tied.op, Op::Read(Read::Items { since: at(100), page: 2, kind: None }));
    let mut moved = summary();
    moved.updated = at(105);
    h.answer(tied, Answer::Items { items: Box::new([moved.clone()]), more: true, now: at(106) });
    let next = h.one();
    assert_eq!(next.op, Op::Read(Read::Items { since: at(105), page: 1, kind: None }));
    h.answer(next, Answer::Items { items: Box::new([moved]), more: false, now: at(107) });
    match h.records.get(&Key::Repository(REPO)).expect("repository saved") {
        Stored::Repository(record) => {
            assert_eq!(record.mark, Some(at(100)));
            assert_eq!(record.clock, at(107));
        }
        Stored::Live(_) | Stored::Writer(_) => panic!("repository row"),
    }
}
#[test]
fn a_pending_review_submitted_later_is_delivered_despite_its_older_id() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(pull(), true)]));
    h.clock();
    participation(&mut h, Box::new([review(2, 100)]));
    h.news.clear();
    h.event(Event::Hint { hint: api::Hint { repository: REPO, change: Change::Item(9), key: None } });
    h.fire(101);
    let listing = h.one();
    h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
    participation(&mut h, Box::new([review(1, 101), review(2, 100)]));
    match h.news.as_slice() {
        [Answer::Reviews { reviews, .. }] => {
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0].id, 1);
        }
        _ => panic!("one newly submitted review"),
    }
    assert_eq!(h.record(&pull()).reviews.len(), 2);
}
#[test]
fn a_full_echo_ledger_blocks_new_writes_until_the_inbox_reads_past_it() {
    let mut h = Harness::new();
    h.env.limits.rows = 1;
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    for entry in 1..=2_u64 {
        h.event(Event::Make {
            entry: Entry {
                number: entry,
                task: 7,
                repository: REPO,
                effect: Effect {
                    write: Write::Post {
                        number: 9,
                        key: Box::new([u8::try_from(entry).unwrap()]),
                        body: Box::from(&b"done"[..]),
                    },
                    condition: Condition::None,
                },
                start: None,
                attempt: None,
                failures: 0,
                queued: false,
            },
        });
        h.clock();
        if entry == 1 {
            let write = h.one();
            h.answer(write, Answer::Commented(7));
        }
    }
    assert!(!h.domain.is_ready());
    assert_eq!(h.record(&issue()).echoes.len(), 1);
    h.event(Event::Hint { hint: api::Hint { repository: REPO, change: Change::Item(9), key: None } });
    h.fire(101);
    let listing = h.one();
    h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(6)]), more: true });
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(7)]), more: false });
    let write = h.one();
    assert!(matches_write(&write.op));
    let entry = h.entries.get(&2).expect("top received entry progress");
    assert_eq!(entry.start.expect("write position saved").comment, 7);
}
#[test]
fn a_rate_refused_terminal_after_departure_never_requeues_the_removed_resource() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let read = h.one();
    h.keep(Box::new([]));
    h.event(Event::Answered {
        call: read.call,
        cost: 1,
        free: 0,
        rate: None,
        result: Err(Error::RateLimited { after: skein_lib::Duration::from_secs(1) }),
    });
    h.fire(101);
    assert!(!h.domain.is_ready());
    assert!(h.records.is_empty());
}
#[test]
fn first_adoption_suppresses_only_framed_historical_keys_from_our_writer() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let mut own = comment(1);
    own.author = 1;
    own.key = Some(Box::from(&b"\x01\x00\x03kitold"[..]));
    let mut foreign = comment(2);
    foreign.author = 1;
    foreign.key = Some(Box::from(&b"\x01\x00\x04elseold"[..]));
    let mut copied = comment(3);
    copied.author = 2;
    copied.key = own.key.clone();
    let human = comment(4);
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([own, foreign, copied, human]), more: true });
    let mut malformed = comment(5);
    malformed.author = 1;
    malformed.key = Some(Box::from(&b"\x01\x00\x05testold"[..]));
    let mut own = comment(6);
    own.author = 1;
    own.key = Some(Box::from(&b"\x01\x00\x03kitnext"[..]));
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([malformed, own]), more: false });
    match h.news.as_slice() {
        [Answer::Item { comments: first, .. }, Answer::Item { comments: second, .. }] => {
            assert_eq!(first.len(), 3);
            assert_eq!(first[0].id, 2);
            assert_eq!(first[1].id, 3);
            assert_eq!(first[2].id, 4);
            assert_eq!(second.len(), 1);
            assert_eq!(second[0].id, 5);
        }
        _ => panic!("foreign, copied, human and malformed keys remain news"),
    }
    assert_eq!(h.record(&issue()).position.comment, 6);
}
#[test]
fn first_adoption_of_reviews_requires_namespace_and_authenticated_author() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(pull(), true)]));
    h.clock();
    let mut own = review(1, 90);
    own.author = 1;
    own.key = Some(Box::from(&b"\x01\x00\x03kitold"[..]));
    let mut foreign = review(2, 90);
    foreign.author = 1;
    foreign.key = Some(Box::from(&b"\x01\x00\x04elseold"[..]));
    let mut copied = review(3, 90);
    copied.key = own.key.clone();
    participation(&mut h, Box::new([own, foreign, copied]));
    match h.news.as_slice() {
        [Answer::Item { .. }, Answer::Pull(_), Answer::Reviews { reviews, .. }] => {
            assert_eq!(reviews.len(), 2);
            assert_eq!(reviews[0].id, 2);
            assert_eq!(reviews[1].id, 3);
        }
        _ => panic!("foreign and copied reviews remain news"),
    }
}

#[test]
fn malformed_review_pages_are_rejected_before_any_delivery_or_cursor_mutation() {
    for ids in [[1, 1], [2, 1], [0, 1]] {
        let mut h = Harness::new();
        h.keep(Box::new([watch(pull(), true)]));
        h.clock();
        let read = h.one();
        h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
        let read = h.one();
        h.answer(read, Answer::Pull(current()));
        h.news.clear();
        let before = h.record(&pull()).clone();
        let read = h.one();
        h.answer(read, Answer::Reviews { reviews: Box::new([review(ids[0], 90), review(ids[1], 90)]), more: false });
        assert!(h.news.is_empty());
        assert_eq!(h.record(&pull()), &before);
        assert_eq!(h.failures.as_slice(), &[Error::InvalidAnswer]);
    }
}
#[test]
fn a_repeated_listing_item_cannot_overflow_a_tiny_decisions_output_bound() {
    let mut h =
        Harness::with_limits(Limits { resources: 1, repositories: 1, rows: 10, rate: 100, reserve: 0, ..LIMITS });
    h.keep(Box::new([watch(issue(), false)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let before = h.record(&issue()).clone();
    h.fire(105);
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Items { since: at(100), page: 1, kind: None }));
    let mut repeated = List::with_capacity(10);
    for _ in 0..10_u32 {
        let mut item = summary();
        item.updated = at(105);
        repeated.push(item).expect("bounded malformed page");
    }
    h.answer(read, Answer::Items { items: repeated.into_boxed(), more: false, now: at(105) });
    assert_eq!(h.record(&issue()), &before);
    assert_eq!(h.domain.next_deadline(), Some(at(106)));
}
#[test]
fn a_reserved_background_share_is_admitted_even_with_one_pending_slot_and_replenished_outbox() {
    let mut h = Harness::with_limits(Limits { pending: 1, calls: 1, rate: 8, reserve: 4, ..LIMITS });
    h.keep(Box::new([watch(issue(), false)]));
    for round in 0..2_u64 {
        for index in 1..=2_u64 {
            h.event(Event::Make {
                entry: Entry {
                    number: round * 3 + index,
                    task: 7,
                    repository: REPO,
                    effect: Effect { write: Write::Close { number: 9 }, condition: Condition::None },
                    start: None,
                    attempt: None,
                    failures: 0,
                    queued: false,
                },
            });
            h.clock();
            let write = h.one();
            assert_eq!(write.op, Op::Write(Write::Close { number: 9 }));
            h.answer(write, Answer::Done);
        }
        h.event(Event::Make {
            entry: Entry {
                number: round * 3 + 3,
                task: 7,
                repository: REPO,
                effect: Effect { write: Write::Close { number: 9 }, condition: Condition::None },
                start: None,
                attempt: None,
                failures: 0,
                queued: false,
            },
        });
        h.event(Event::Read {
            owner: Token::new(round + 10),
            repository: REPO,
            read: Read::Branch { branch: Box::from(&b"fresh"[..]) },
        });
        assert_eq!(h.read_refusals.as_slice().last(), Some(&Error::Busy));
        let read = h.one();
        let since = if round == 0 { at(u64::MAX) } else { at(100) };
        assert_eq!(read.op, Op::Read(Read::Items { since, page: 1, kind: None }));
        h.answer(read, Answer::Items { items: Box::new([]), more: false, now: h.env.now });
        let read = h.one();
        assert_eq!(read.op, Op::Read(Read::Item { number: 9, after: u64::MAX }));
        h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
        h.clock();
        let write = h.one();
        assert_eq!(write.op, Op::Write(Write::Close { number: 9 }));
        h.answer(write, Answer::Done);
        assert_eq!(h.outcomes.len(), u32::try_from((round + 1) * 3).expect("small test count"));
        if round == 0 {
            h.fire(160);
        }
    }
}
#[test]
fn a_restart_between_review_pages_never_repeats_delivered_rows_or_metadata() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(pull(), true)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let read = h.one();
    h.answer(read, Answer::Pull(current()));
    let read = h.one();
    h.answer(read, Answer::Reviews { reviews: Box::new([review(1, 90), review(2, 90)]), more: true });
    assert_eq!(h.record(&pull()).review_page, 2);
    h.news.clear();
    h.restart();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let read = h.one();
    h.answer(read, Answer::Pull(current()));
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Reviews { number: 9, page: 2 }));
    // A pending review became visible before our page: the shifted page
    // repeats an older row, whose durable delivery stamp suppresses it.
    h.answer(read, Answer::Reviews { reviews: Box::new([review(2, 90), review(3, 91)]), more: false });
    match h.news.as_slice() {
        [Answer::Reviews { reviews, .. }] => {
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0].id, 3);
        }
        _ => panic!("one new review after recovery, no cached metadata repeat"),
    }
    assert_eq!(h.record(&pull()).review_page, 1);
}
#[test]
fn comment_scan_progress_and_edited_old_versions_are_durable() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(1)]), more: true });
    assert_eq!(h.record(&issue()).comment_after, 1);
    h.news.clear();
    h.restart();
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Item { number: 9, after: 1 }));
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(2)]), more: false });
    assert_eq!(h.record(&issue()).comment_after, 2);
    h.news.clear();
    h.restart();
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Item { number: 9, after: 0 }));
    let mut edited = comment(1);
    edited.revision = 2;
    edited.body = Box::from(&b"edited"[..]);
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([edited.clone(), comment(2)]), more: false });
    match h.news.as_slice() {
        [Answer::Item { comments, .. }] => assert_eq!(comments.as_ref(), &[edited.clone()]),
        _ => panic!("exactly one edited version"),
    }
    h.news.clear();
    h.restart();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([edited, comment(2)]), more: false });
    assert!(h.news.is_empty());
}
#[test]
fn inbox_overflow_never_partially_delivers_or_advances_a_page() {
    let mut h = Harness::new();
    h.env.limits.inbox = 1;
    h.keep(Box::new([watch(issue(), true)]));
    h.clock();
    let read = h.one();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(1)]), more: false });
    h.news.clear();
    h.restart();
    let read = h.one();
    let saved = h.record(&issue()).clone();
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(1), comment(2)]), more: false });
    assert_eq!(h.failures.as_slice(), &[Error::TooLarge]);
    assert!(h.news.is_empty());
    assert_eq!(h.record(&issue()), &saved);
    h.env.limits.inbox = 2;
    h.restart();
    let read = h.one();
    assert_eq!(read.op, Op::Read(Read::Item { number: 9, after: 0 }));
    h.answer(read, Answer::Item { item: summary(), comments: Box::new([comment(1), comment(2)]), more: false });
    match h.news.as_slice() {
        [Answer::Item { comments, .. }] => assert_eq!(comments.as_ref(), &[comment(2)]),
        _ => panic!("once after capacity reconciliation"),
    }
}
#[test]
fn a_first_inbox_read_never_hides_an_edited_or_unknown_completed_own_echo() {
    for provenance in [api::Provenance::Revised, api::Provenance::Unknown] {
        let mut h = Harness::new();
        h.keep(Box::new([watch(issue(), true)]));
        h.clock();
        let read = h.one();
        h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
        h.news.clear();
        h.event(Event::Make {
            entry: Entry {
                number: 1,
                task: 7,
                repository: REPO,
                effect: Effect {
                    write: Write::Post {
                        number: 9,
                        key: Box::from(&b"\x01\x00\x03kitown"[..]),
                        body: Box::from(&b"done"[..]),
                    },
                    condition: Condition::None,
                },
                start: None,
                attempt: None,
                failures: 0,
                queued: false,
            },
        });
        h.clock();
        let write = h.one();
        h.answer(write, Answer::Commented(7));
        h.event(Event::Hint { hint: api::Hint { repository: REPO, change: Change::Item(9), key: None } });
        h.fire(101);
        let listing = h.one();
        h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
        let read = h.one();
        let mut changed = comment(7);
        changed.author = 1;
        changed.provenance = provenance;
        changed.key = Some(Box::from(&b"\x01\x00\x03kitown"[..]));
        h.answer(read, Answer::Item { item: summary(), comments: Box::new([changed.clone()]), more: false });
        match h.news.as_slice() {
            [Answer::Item { comments, .. }] => assert_eq!(comments.as_ref(), &[changed]),
            _ => panic!("changed or unproven body remains news before first inbox read"),
        }
    }
}

#[test]
fn writer_moves_require_forward_ancestry_and_authenticated_identity() {
    for (actor, forward, hint_head, expected_drift) in
        [(1, true, [2; 32], 0), (2, true, [2; 32], 1), (1, false, [2; 32], 1), (2, true, [3; 32], 0)]
    {
        let mut h = Harness::new();
        h.keep(Box::new([watch(branch(), false)]));
        h.clock();
        let read = h.one();
        h.answer(read, Answer::Commit([1; 32]));
        h.event(Event::Writer { resource: branch(), taken: true });
        h.event(Event::Hint {
            hint: api::Hint {
                repository: REPO,
                change: Change::BranchMoved { branch: Box::from(&b"topic"[..]), head: hint_head, actor },
                key: None,
            },
        });
        h.fire(101);
        let listing = h.one();
        h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
        let read = h.one();
        assert_eq!(read.op, Op::Read(Read::Branch { branch: Box::from(&b"topic"[..]) }));
        h.answer(read, Answer::Commit([2; 32]));
        let compare = h.one();
        assert_eq!(compare.op, Op::Read(Read::Compare { before: [1; 32], after: [2; 32] }));
        h.answer(
            compare,
            Answer::Compare {
                before: [1; 32],
                after: [2; 32],
                contains_before: forward,
                files: Box::new([]),
                commits: Box::new([]),
            },
        );
        assert_eq!(h.drift, expected_drift);
        assert_eq!(h.news.as_slice(), &[Answer::Commit([1; 32])]);
    }
}

fn missing(h: &mut Harness, sent: super::Sent) {
    h.event(Event::Answered { call: sent.call, cost: 1, free: 0, rate: None, result: Err(Error::Missing) });
}

#[test]
fn a_branch_read_from_before_our_push_is_read_again() {
    for writer in [false, true] {
        let mut h = Harness::new();
        h.keep(Box::new([watch(branch(), false)]));
        h.clock();
        let read = h.one();
        if writer {
            h.event(Event::Writer { resource: branch(), taken: true });
        }
        h.event(Event::Pushed { resource: branch(), commit: [1; 32] });
        missing(&mut h, read);
        assert_eq!(h.drift, 0, "an absence read before our push cannot judge it");
        assert!(h.failures.is_empty());
        assert_eq!(h.record(&branch()).cached.tip, Some([1; 32]));
        let read = h.one();
        assert_eq!(read.op, Op::Read(Read::Branch { branch: Box::from(&b"topic"[..]) }));
        h.answer(read, Answer::Commit([1; 32]));
        assert_eq!(h.drift, 0);
        assert_eq!(h.record(&branch()).cached.tip, Some([1; 32]));
    }
}
#[test]
fn a_branch_still_missing_on_its_read_after_our_push_drifts() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(branch(), false)]));
    h.clock();
    let read = h.one();
    h.event(Event::Pushed { resource: branch(), commit: [1; 32] });
    missing(&mut h, read);
    assert_eq!(h.drift, 0);
    let read = h.one();
    missing(&mut h, read);
    assert_eq!(h.drift, 1, "a branch deleted after our push still drifts");
    assert_eq!(h.failures.as_slice(), &[Error::Missing]);
    assert_eq!(h.record(&branch()).cached.tip, None);
}
#[test]
fn missing_branch_rests_at_slow_and_reports_each_absence_once() {
    for cached in [false, true] {
        let mut h = Harness::new();
        h.env.limits.poll = skein_lib::Duration::from_secs(1000);
        h.env.limits.poll_max = h.env.limits.poll;
        h.keep(Box::new([watch(branch(), false)]));
        h.clock();
        if cached {
            h.event(Event::Pushed { resource: branch(), commit: [1; 32] });
        }
        let read = h.one();
        missing(&mut h, read);
        assert_eq!(h.failures.as_slice(), &[Error::Missing]);
        assert_eq!(h.drift, u32::from(cached));
        assert_eq!(h.record(&branch()).cached.tip, None);
        for now in [160, 220, 280] {
            h.fire(now - 1);
            assert!(h.send().is_none(), "absence waits the whole slow rest");
            h.fire(now);
            let read = h.one();
            assert_eq!(read.op, Op::Read(Read::Branch { branch: Box::from(&b"topic"[..]) }));
            missing(&mut h, read);
            assert_eq!(h.failures.as_slice(), &[Error::Missing]);
            assert_eq!(h.drift, u32::from(cached));
        }
        h.fire(340);
        let read = h.one();
        h.answer(read, Answer::Commit([2; 32]));
        assert_eq!(h.news.as_slice(), &[Answer::Commit([2; 32])]);
        let drift = h.drift;
        h.fire(400);
        let read = h.one();
        missing(&mut h, read);
        assert_eq!(h.failures.as_slice(), &[Error::Missing, Error::Missing]);
        assert_eq!(h.drift, drift + 1);
        h.restart();
        let read = h.one();
        missing(&mut h, read);
        assert_eq!(h.failures.as_slice(), &[Error::Missing, Error::Missing, Error::Missing]);
        assert_eq!(h.drift, drift + 1, "cleared stored tip prevents deletion drift after restart");
        assert_eq!(h.record(&branch()).cached.tip, None);
    }
}

#[test]
fn missing_issue_and_pull_rest_at_slow_and_suppress_repeated_news() {
    for resource in [issue(), pull()] {
        let mut h = Harness::new();
        h.env.limits.poll = skein_lib::Duration::from_secs(1000);
        h.env.limits.poll_max = h.env.limits.poll;
        h.keep(Box::new([watch(resource.clone(), false)]));
        h.clock();
        let read = h.one();
        missing(&mut h, read);
        for now in [160, 220] {
            h.fire(now - 1);
            assert!(h.send().is_none());
            h.fire(now);
            let read = h.one();
            missing(&mut h, read);
            assert_eq!(h.failures.as_slice(), &[Error::Missing]);
        }
        // Finding the watched resource ends the absence.
        h.fire(280);
        let read = h.one();
        h.answer(read, Answer::Item { item: summary(), comments: Box::new([]), more: false });
        if resource == pull() {
            let read = h.one();
            h.answer(read, Answer::Pull(current()));
        }
        h.fire(340);
        let read = h.one();
        missing(&mut h, read);
        assert_eq!(h.failures.as_slice(), &[Error::Missing, Error::Missing]);
    }
}

#[test]
fn missing_uses_hinted_rest_and_unavailable_keeps_backoff() {
    let mut h = Harness::new();
    h.keep(Box::new([watch(branch(), false)]));
    h.clock();
    let read = h.one();
    h.event(Event::Hint {
        hint: api::Hint { repository: REPO, change: Change::Branch(Box::from(&b"topic"[..])), key: None },
    });
    missing(&mut h, read);
    h.fire(101);
    let listing = h.one();
    h.answer(listing, Answer::Items { items: Box::new([]), more: false, now: at(101) });
    let read = h.one();
    missing(&mut h, read);
    assert_eq!(h.failures.as_slice(), &[Error::Missing]);

    let mut h = Harness::new();
    h.keep(Box::new([watch(branch(), false)]));
    h.clock();
    for now in [101, 102, 103] {
        let read = h.one();
        h.event(Event::Answered { call: read.call, cost: 1, free: 0, rate: None, result: Err(Error::Unavailable) });
        assert!(h.send().is_none());
        h.fire(now);
    }
    assert_eq!(h.failures.as_slice(), &[Error::Unavailable; 3]);
    assert_eq!(h.one().op, Op::Read(Read::Branch { branch: Box::from(&b"topic"[..]) }));
}

#[test]
fn missing_pull_stays_absent_across_successful_item_phases() {
    let mut h = Harness::new();
    h.env.limits.poll = skein_lib::Duration::from_secs(1000);
    h.env.limits.poll_max = h.env.limits.poll;
    h.keep(Box::new([watch(pull(), false)]));
    h.clock();
    for now in [100, 160, 220] {
        h.fire(now);
        let item = h.one();
        h.answer(item, Answer::Item { item: summary(), comments: Box::new([]), more: false });
        let read = h.one();
        assert_eq!(read.op, Op::Read(Read::Pull { number: 9 }));
        missing(&mut h, read);
        assert_eq!(h.failures.as_slice(), &[Error::Missing]);
        h.fire(now + 59);
        assert!(h.send().is_none());
    }
    h.fire(280);
    let item = h.one();
    h.answer(item, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let read = h.one();
    h.answer(read, Answer::Pull(current()));
    h.fire(340);
    let item = h.one();
    h.answer(item, Answer::Item { item: summary(), comments: Box::new([]), more: false });
    let read = h.one();
    missing(&mut h, read);
    assert_eq!(h.failures.as_slice(), &[Error::Missing, Error::Missing]);
}
