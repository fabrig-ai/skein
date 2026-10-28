//! The owner's final stop fence (forge-client.md, 7.8).
use super::fixtures::keep::{Harness as Kept, branch, watch};
use super::fixtures::outbox::{Harness, comment, pull};
use super::{LIMITS, at, configured};
use crate::{Condition, Event, Made, Outcome, Request, fire, step};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read, Write};
use skein_lib::{Duration, Env, Queue, Time, Wall};

#[test]
fn settle_with_nothing_sent_is_settled_at_once() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.step(Event::Settle);
    assert_eq!(h.settled.as_deref(), Some([].as_slice()));
    h.step(Event::Settle);
    assert!(h.send().is_none());
    // Stopping before restoration is also empty and does not require PauseOutbox.
    let mut d = configured(&LIMITS);
    let env = Env { now: at(100), wall: Wall::from_nanos(0), limits: LIMITS };
    let mut out = Queue::uncapped();
    step(&mut d, &env, Event::Settle, &mut out);
    assert_eq!(out.pop(), Some(Request::Settled { left: Box::new([]) }));
}

fn sent_merge() -> (Harness, super::Sent) {
    let mut h = Harness::new();
    h.make(
        1,
        Write::Merge { number: 9, head: [1; 32], how: api::MergeHow::Style },
        Condition::Merge { base: Box::from(b"main".as_slice()) },
    );
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = h.one();
    (h, write)
}

#[test]
fn settle_waits_for_a_sent_merge_and_its_proving_read() {
    let (mut h, write) = sent_merge();
    h.step(Event::Settle);
    assert_eq!(h.settled, None);
    h.answer(write, Ok(Answer::Merged([4; 32])));
    assert!(h.outcomes.is_empty());
    assert_eq!(h.settled, None);
    let verify = h.one();
    assert_eq!(verify.op, Op::Read(Read::Pull { number: 9 }));
    let mut current = pull([1; 32]);
    current.merged = Some([4; 32]);
    current.state = api::State::Closed;
    h.answer(verify, Ok(Answer::Pull(current)));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Made { made: Made::Merged([4; 32]), found: false })]);
    assert_eq!(h.settled.as_deref(), Some([].as_slice()));
    assert_eq!(h.writes, 1);
}

#[test]
fn settle_sends_no_write_due_or_queued() {
    for queued in [false, true] {
        let mut h = Harness::new();
        h.make(1, comment(), Condition::None);
        h.clock();
        if queued {
            let mut out = Queue::uncapped();
            crate::outbox::pump(&mut h.domain, &h.env, &mut out);
            h.take(&mut out); // pump queued the write, without sending it
            assert!(h.calls.is_empty());
        }
        h.step(Event::Settle);
        assert_eq!(h.settled.as_deref(), Some([].as_slice()));
        for _ in 0_u32..4 {
            assert!(h.send().is_none());
        }
        assert!(!h.domain.is_ready());
        assert_eq!(h.writes, 0);
        assert!(h.store.get(&1).unwrap().attempt.is_none());
    }
}

#[test]
fn a_lost_answer_while_settling_ends_uncertain_and_settles() {
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let write = h.one();
    h.step(Event::Settle);
    h.answer(write, Err(Error::Timeout));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
    assert_eq!(h.settled.as_deref(), Some([].as_slice()));
    assert!(h.store.get(&1).unwrap().attempt.is_some());
    assert!(h.send().is_none(), "the next start performs the find");
}

#[test]
fn settle_starts_no_listing_poll_or_find() {
    // A recovery find is already due, but its uncertain outcome was reported.
    let mut h = Harness::new();
    h.make(1, comment(), Condition::None);
    h.clock();
    let write = h.one();
    h.answer(write, Err(Error::Timeout));
    h.make(2, Write::CreateBranch { branch: Box::from(b"new".as_slice()), commit: [2; 32] }, Condition::None);
    h.step(Event::Settle);
    for _ in 0_u32..4 {
        assert!(h.send().is_none(), "neither find nor a new listing starts");
    }
    assert_eq!(h.writes, 1);
    assert_eq!(h.settled.as_deref(), Some([].as_slice()));
    // An independently kept branch has its poll due when the stop arrives.
    let mut kept = Kept::new();
    kept.keep(Box::new([watch(branch(), false)]));
    kept.clock();
    let read = kept.one();
    kept.answer(read, Answer::Commit([1; 32]));
    kept.fire(105);
    assert!(kept.domain.is_ready());
    kept.event(Event::Settle);
    assert!(kept.send().is_none(), "a due poll starts no call");
}

#[test]
fn settle_names_an_entry_unfinished_at_its_bound() {
    let (mut h, _write) = sent_merge();
    h.step(Event::Settle);
    let span = h.env.limits.lifetime.saturating_add(h.env.limits.clock_margin).saturating_mul(2);
    let bound = h.env.now.saturating_add(span);
    assert_eq!(h.domain.next_deadline(), Some(bound));
    let mut out = Queue::uncapped();
    h.env.now = Time::from_nanos(bound.as_nanos().checked_sub(1).unwrap());
    fire(&mut h.domain, &h.env, &mut out);
    h.take(&mut out);
    assert_eq!(h.settled, None);
    h.env.now = bound;
    fire(&mut h.domain, &h.env, &mut out);
    h.take(&mut out);
    assert_eq!(h.settled.as_deref(), Some([1].as_slice()));
    assert_eq!(span, Duration::from_secs(22));
    h.step(Event::Settle);
}

#[test]
fn settling_keeps_a_reviews_proving_find_and_an_updates_comparison() {
    use super::fixtures::outbox::recovered_review;
    let mut review = Harness::new();
    review.make(
        1,
        Write::Review {
            number: 9,
            key: Box::from(b"\x01\x00\x03kitreview-key".as_slice()),
            verdict: api::Verdict::Approve,
            body: Box::new([]),
        },
        Condition::Review { head: [1; 32] },
    );
    review.clock();
    let check = review.one();
    review.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = review.one();
    review.step(Event::Settle);
    review.answer(write, Ok(Answer::Reviewed(7)));
    let verify = review.one();
    review.answer(
        verify,
        Ok(Answer::Reviews {
            reviews: Box::new([recovered_review(7, api::Provenance::Original, [1; 32])]),
            more: false,
        }),
    );
    assert_eq!(review.settled.as_deref(), Some([].as_slice()));
    assert_eq!(review.outcomes.as_slice(), &[(1, Outcome::Made { made: Made::Reviewed(7), found: true })]);

    let mut update = Harness::new();
    update.make(
        1,
        Write::Update { number: 9, style: api::UpdateStyle::Merge },
        Condition::Update { head: [1; 32], base: [2; 32] },
    );
    update.clock();
    let check = update.one();
    update.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = update.one();
    update.step(Event::Settle);
    update.answer(write, Ok(Answer::Done));
    let verify = update.one();
    update.answer(verify, Ok(Answer::Pull(pull([3; 32]))));
    assert_eq!(update.settled, None);
    let compare = update.one();
    update.answer(
        compare,
        Ok(Answer::Compare {
            before: [3; 32],
            after: [2; 32],
            contains_before: false,
            files: Box::new([]),
            commits: Box::new([]),
        }),
    );
    assert_eq!(update.settled.as_deref(), Some([].as_slice()));
    assert_eq!(update.writes, 1);
}

#[test]
fn a_write_refused_while_settling_is_not_sent_again() {
    for error in [Error::Unavailable, Error::RateLimited { after: Duration::from_secs(1) }] {
        let mut h = Harness::new();
        h.make(1, comment(), Condition::None);
        h.clock();
        let write = h.one();
        h.step(Event::Settle);
        h.answer(write, Err(error));
        h.fire(102);
        assert!(h.send().is_none());
        assert_eq!(h.writes, 1);
        assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain)]);
        assert_eq!(h.settled.as_deref(), Some([].as_slice()));
    }
}

#[test]
fn a_proving_read_uses_the_reserve_while_background_calls_are_held() {
    let (mut h, write) = sent_merge();
    h.env.limits.reserve = 99;
    // Background work was queued before the stop; it cannot consume its share now.
    crate::calls::queue(
        &mut h.domain.calls,
        crate::calls::Owner::Repository(super::REPO),
        super::REPO,
        Op::Read(Read::Items { since: at(100), page: 1, kind: None }),
        crate::Priority::Slow,
    );
    h.step(Event::Settle);
    h.answer(write, Ok(Answer::Merged([4; 32])));
    let proof = h.one();
    assert_eq!(proof.op, Op::Read(Read::Pull { number: 9 }));
}
