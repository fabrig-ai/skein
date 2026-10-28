use super::fixtures::outbox::{Harness, comment, pull};
use super::{REPO, Sent};
use crate::{Condition, Event, Made, Outcome, RecoveryClock, Request};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Change, Error, Op, Read, Write};
use skein_lib::{Duration, Queue, Time};

fn merge(h: &mut Harness, entry: u64, number: u64) {
    h.make(
        entry,
        Write::Merge { number, head: [1; 32], how: api::MergeHow::Style },
        Condition::Merge { base: Box::from(&b"main"[..]) },
    );
}
fn submit(h: &mut Harness) -> Sent {
    merge(h, 1, 9);
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let sent = h.one();
    assert_eq!(sent.op, Op::Write(Write::Merge { number: 9, head: [1; 32], how: api::MergeHow::Style }));
    sent
}
fn fire_at(h: &mut Harness, now: Time) {
    h.env.now = now;
    let mut out = Queue::uncapped();
    crate::fire(&mut h.domain, &h.env, &mut out);
    h.take(&mut out);
    h.domain.reclaim();
}
fn next_look(h: &mut Harness) -> Sent {
    let due = h.domain.next_deadline().expect("queued look armed");
    fire_at(h, due);
    let look = h.one();
    assert_eq!(look.op, Op::Read(Read::Pull { number: 9 }));
    look
}
fn queued(h: &mut Harness) -> Sent {
    let sent = submit(h);
    h.answer(sent, Ok(Answer::Queued));
    next_look(h)
}
fn still_queued() -> api::Pull {
    api::Pull { queued: Some(1), ..pull([1; 32]) }
}
fn hint(h: &mut Harness, change: Change) {
    h.step(Event::Hint { hint: api::Hint { repository: REPO, change, key: None } });
}
fn merged(head: [u8; 32]) -> api::Pull {
    api::Pull { merged: Some([4; 32]), state: api::State::Closed, ..pull(head) }
}
fn restart(h: &mut Harness) -> Sent {
    let sent = submit(h);
    h.answer(sent, Ok(Answer::Queued));
    h.restart(200, 200, RecoveryClock::Monotonic);
    h.step(Event::PauseOutbox);
    assert!(h.send().is_none());
    h.step(Event::SettleOutbox);
    assert_eq!(h.outbox_done, 0);
    let look = h.one();
    assert_eq!(look.op, Op::Read(Read::Pull { number: 9 }));
    look
}

#[test]
fn a_merge_answered_queued_is_kept_and_reported() {
    let mut h = Harness::new();
    let sent = submit(&mut h);
    let mut out = Queue::uncapped();
    crate::step(
        &mut h.domain,
        &h.env,
        Event::Answered { call: sent.call, cost: 1, free: 0, rate: None, result: Ok(Answer::Queued) },
        &mut out,
    );
    assert_eq!(out.len(), 2);
    let Request::Progress { entry } = out.pop().expect("progress first") else {
        panic!("queue progress");
    };
    assert!(entry.queued);
    assert!(entry.attempt.is_some());
    assert_eq!(out.pop(), Some(Request::Outcome { entry: 1, task: 7, outcome: Outcome::Queued }));
    h.store.insert(1, entry).expect("durable progress fits");
    assert!(h.store.get(&1).expect("entry kept").queued);
    assert!(h.send().is_none());
}
#[test]
fn a_queued_merge_is_never_sent_again() {
    let mut h = Harness::new();
    let mut look = queued(&mut h);
    let end = h.env.now.saturating_add(h.env.limits.lifetime).saturating_add(h.env.limits.settle);
    for _ in 0_u32..20 {
        assert!(h.env.now <= end.saturating_add(h.env.limits.poll));
        h.answer(look, Ok(still_queued_answer()));
        if h.env.now > end {
            break;
        }
        look = next_look(&mut h);
    }
    assert!(h.env.now > end);
    assert_eq!(h.writes, 1);
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued)]);
}
fn still_queued_answer() -> Answer {
    Answer::Pull(still_queued())
}
#[test]
fn the_first_look_comes_after_a_poll() {
    let mut h = Harness::new();
    let sent = submit(&mut h);
    h.answer(sent, Ok(Answer::Queued));
    assert!(h.send().is_none());
    assert_eq!(h.domain.next_deadline(), Some(h.env.now.saturating_add(h.env.limits.poll)));
    h.fire(104);
    assert!(h.send().is_none());
    h.fire(105);
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 9 }));
}
#[test]
fn a_queued_merge_merged_at_its_head_is_made() {
    let mut h = Harness::new();
    let look = queued(&mut h);
    h.answer(look, Ok(Answer::Pull(merged([1; 32]))));
    assert_eq!(
        h.outcomes.as_slice(),
        &[(1, Outcome::Queued), (1, Outcome::Made { made: Made::Merged([4; 32]), found: false })]
    );
    assert!(h.store.is_empty());
}
#[test]
fn a_queued_merge_merged_at_another_head_is_raced() {
    let mut h = Harness::new();
    let look = queued(&mut h);
    h.answer(look, Ok(Answer::Pull(merged([2; 32]))));
    assert_eq!(
        h.outcomes.as_slice(),
        &[(1, Outcome::Queued), (1, Outcome::Raced { made: Made::Merged([4; 32]), why: Error::Stale })]
    );
}
#[test]
fn a_queued_merge_closed_unmerged_fails_closed() {
    let mut h = Harness::new();
    let look = queued(&mut h);
    h.answer(look, Ok(Answer::Pull(api::Pull { state: api::State::Closed, ..pull([1; 32]) })));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued), (1, Outcome::Failed(Error::Closed))]);
}
#[test]
fn a_queued_merge_ejected_fails_with_the_reason_it_shows() {
    for (label, head, mergeable, error) in [
        ("head moved before conflict", [2; 32], false, Error::Stale),
        ("head kept, conflicts", [1; 32], false, Error::Conflict),
        ("head kept, mergeable", [1; 32], true, Error::Refused),
    ] {
        let mut h = Harness::new();
        let look = queued(&mut h);
        let current = api::Pull { mergeable, dequeued: Some(Box::from(&b"CI_FAILURE"[..])), ..pull(head) };
        h.answer(look, Ok(Answer::Pull(current)));
        assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued), (1, Outcome::Failed(error))], "{label}");
        assert!(h.send().is_none(), "{label}");
        assert_eq!(h.writes, 1, "{label}");
    }
}
#[test]
fn a_queued_merge_still_queued_stays_kept_and_is_looked_at_each_poll() {
    let mut h = Harness::new();
    let mut look = queued(&mut h);
    let looks = h.env.limits.poll_max.as_nanos() / h.env.limits.poll.as_nanos() + 2;
    for _ in 0..looks {
        h.answer(look, Ok(still_queued_answer()));
        assert_eq!(h.domain.next_deadline(), Some(h.env.now.saturating_add(h.env.limits.poll)));
        assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued)]);
        assert_eq!(h.store.len(), 1);
        look = next_look(&mut h);
    }
}
#[test]
fn a_hint_naming_the_pull_brings_the_look_forward() {
    let mut h = Harness::new();
    let sent = submit(&mut h);
    h.answer(sent, Ok(Answer::Queued));
    let poll = h.domain.next_deadline();
    for change in [
        Change::Item(10),
        Change::Branch(Box::from(&b"topic"[..])),
        Change::Commit([1; 32]),
        Change::BranchMoved { branch: Box::from(&b"topic"[..]), head: [1; 32], actor: 1 },
    ] {
        hint(&mut h, change);
        assert_eq!(h.domain.next_deadline(), poll);
    }
    h.step(Event::Hint {
        hint: api::Hint { repository: api::Repository { repository: 6, ..REPO }, change: Change::Item(9), key: None },
    });
    assert_eq!(h.domain.next_deadline(), poll);
    hint(&mut h, Change::Item(9));
    let due = h.env.now.saturating_add(h.env.limits.hinted);
    assert_eq!(h.domain.next_deadline(), Some(due));
    h.env.now = h.env.now.saturating_add(Duration::from_millis(500));
    hint(&mut h, Change::Item(9));
    assert_eq!(h.domain.next_deadline(), Some(due));
    assert_eq!(next_look(&mut h).op, Op::Read(Read::Pull { number: 9 }));
}
#[test]
fn a_hint_during_a_look_in_flight_quickens_the_next_one() {
    let mut h = Harness::new();
    let look = queued(&mut h);
    hint(&mut h, Change::Item(9));
    h.answer(look, Ok(still_queued_answer()));
    assert_eq!(h.domain.next_deadline(), Some(h.env.now.saturating_add(h.env.limits.hinted)));
    let look = next_look(&mut h);
    h.answer(look, Ok(still_queued_answer()));
    assert_eq!(h.domain.next_deadline(), Some(h.env.now.saturating_add(h.env.limits.poll)));
}
#[test]
fn a_comment_on_a_queued_pull_waits_for_the_queue() {
    let mut h = Harness::new();
    let look = queued(&mut h);
    h.make(2, comment(), Condition::None);
    assert!(h.send().is_none());
    h.answer(look, Ok(Answer::Pull(merged([1; 32]))));
    h.clock();
    assert_eq!(h.one().op, Op::Write(comment()));
}
#[test]
fn a_queued_merge_cannot_be_withdrawn() {
    let mut h = Harness::new();
    let sent = submit(&mut h);
    h.answer(sent, Ok(Answer::Queued));
    let saved = h.store.get(&1).expect("queued entry").clone();
    let due = h.domain.next_deadline();
    h.step(Event::Withdraw { entry: 1 });
    assert_eq!(h.store.get(&1), Some(&saved));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued)]);
    assert_eq!(h.domain.next_deadline(), due);
    assert!(h.send().is_none());
}
#[test]
fn a_queued_merge_counts_against_entries() {
    let mut h = Harness::new();
    for entry in 1..=u64::from(h.env.limits.entries) {
        let number = entry + 8;
        merge(&mut h, entry, number);
        h.clock();
        let check = h.one();
        h.answer(check, Ok(Answer::Pull(api::Pull { number, ..pull([1; 32]) })));
        let sent = h.one();
        h.answer(sent, Ok(Answer::Queued));
    }
    let full = h.env.limits.entries;
    h.make(u64::from(full) + 1, comment(), Condition::None);
    assert_eq!(h.outcomes.as_slice().last(), Some(&(u64::from(full) + 1, Outcome::Failed(Error::Busy))));
    assert_eq!(h.store.len(), full);
    for (_, entry) in &h.store {
        assert!(entry.queued);
    }
}
#[test]
fn a_queued_merge_restored_is_read_at_once_and_never_sent() {
    let mut h = Harness::new();
    let look = restart(&mut h);
    h.answer(look, Ok(still_queued_answer()));
    assert_eq!(h.outbox_done, 1);
    let look = next_look(&mut h);
    h.answer(look, Ok(Answer::Pull(merged([1; 32]))));
    assert!(h.send().is_none());
    assert_eq!(h.writes, 1);
    assert_eq!(h.outbox_done, 1);
}
#[test]
fn a_queued_merge_whose_look_fails_backs_off_and_never_fails() {
    for error in [Error::Timeout, Error::InvalidAnswer, Error::TooLarge, Error::Unavailable, Error::Forbidden] {
        let mut h = Harness::new();
        let mut look = queued(&mut h);
        for _ in 0..=h.env.limits.write_attempts {
            h.answer(look, Err(error));
            assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued)], "{error:?}");
            assert!(h.store.get(&1).expect("queued merge retained").queued);
            look = next_look(&mut h);
        }
        assert_eq!(h.writes, 1);
    }
}
#[test]
fn only_a_merge_may_be_answered_queued() {
    assert!(crate::api::accepts(
        &Op::Write(Write::Merge { number: 9, head: [1; 32], how: api::MergeHow::Style }),
        &Answer::Queued
    ));
    assert!(!crate::api::accepts(&Op::Write(comment()), &Answer::Queued));
    assert!(!crate::api::accepts(&Op::Read(Read::Pull { number: 9 }), &Answer::Queued));
}
#[test]
fn a_hint_during_a_failure_backoff_brings_the_look_forward() {
    let mut h = Harness::new();
    h.env.limits.backoff = Duration::from_secs(4);
    let look = queued(&mut h);
    h.answer(look, Err(Error::Timeout));
    let retry = h.domain.next_deadline().expect("retry armed");
    assert!(retry >= h.env.now.saturating_add(Duration::from_secs(4)));
    hint(&mut h, Change::Item(10));
    assert_eq!(h.domain.next_deadline(), Some(retry));
    hint(&mut h, Change::Item(9));
    assert_eq!(h.domain.next_deadline(), Some(h.env.now.saturating_add(h.env.limits.hinted)));
    assert_eq!(next_look(&mut h).op, Op::Read(Read::Pull { number: 9 }));
}
#[test]
fn a_queued_merge_whose_pull_is_missing_fails_missing_and_frees_its_lane() {
    let mut h = Harness::new();
    let look = queued(&mut h);
    h.make(2, comment(), Condition::None);
    assert!(h.send().is_none());
    h.answer(look, Err(Error::Missing));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Queued), (1, Outcome::Failed(Error::Missing))]);
    assert!(h.store.get(&1).is_none());
    h.clock();
    let comment = h.one();
    assert_eq!(comment.op, Op::Write(self::comment()));
    h.answer(comment, Ok(Answer::Commented(7)));
    merge(&mut h, 3, 9);
    h.clock();
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 9 }));
    assert_eq!(h.outcomes.len(), 3);
}
#[test]
fn a_restored_queued_merge_whose_first_look_fails_still_lets_the_outbox_finish() {
    let mut h = Harness::new();
    let look = restart(&mut h);
    h.answer(look, Err(Error::Timeout));
    assert_eq!(h.outbox_done, 1);
    assert_eq!(next_look(&mut h).op, Op::Read(Read::Pull { number: 9 }));
    assert_eq!(h.writes, 1);
}
