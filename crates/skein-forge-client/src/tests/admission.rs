use super::{Harness, LIMITS, REPO, at, listing_item, terminal};
use crate::{Event, Limits, Request, Resource, Watch, What};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read};
use skein_lib::Token;

const ATTEMPT: api::JobAttempt = api::JobAttempt { head: [1; 32], run: 2, job: 4, attempt: 1 };

fn job(owner: u64) -> Event {
    Event::Read { owner: Token::new(owner), repository: REPO, read: Read::Job { attempt: ATTEMPT, max_bytes: 64 } }
}

fn log() -> Answer {
    Answer::Job {
        attempt: ATTEMPT,
        step: Some(api::Step { number: 1, name: Box::from(b"changes".as_slice()) }),
        log: Box::from(b"Missing changes/2.md\n".as_slice()),
        truncated: false,
    }
}

#[test]
fn a_job_read_waits_for_a_call_slot_and_completes_once() {
    let mut h = Harness::new(Limits { pending: 1, calls: 1, ..LIMITS });
    h.read(1);
    let held = h.one();
    let deadline = h.domain.next_deadline();
    assert!(h.step(job(2)).is_empty(), "local admission retains the demand without answering Busy");
    assert!(!h.domain.is_ready(), "a full client waits for its outstanding call");
    assert!(h.send().is_none());
    assert_eq!(h.domain.next_deadline(), deadline, "admission arms no timer");
    terminal(&h.answer(&held, 1, Err(Error::Forbidden)), 1, Error::Forbidden);
    assert!(h.domain.is_ready(), "releasing the slot wakes the retained read");
    let sent = h.one();
    assert_eq!(sent.op, Op::Read(Read::Job { attempt: ATTEMPT, max_bytes: 64 }));
    assert_ne!(sent.call, held.call);
    assert_eq!(h.answer(&sent, 1, Ok(log())).as_ref(), &[Request::Read { owner: Token::new(2), result: Ok(log()) }]);
    assert!(!h.domain.is_ready());
    assert!(h.send().is_none(), "the demand is issued and answered once");
    assert_eq!(h.env.now, at(100), "a slot release needs no clock advance");
}

#[test]
fn a_job_read_waits_for_the_reserved_background_share() {
    let mut h = Harness::new(Limits { pending: 1, calls: 1, rate: 4, reserve: 3, ..LIMITS });
    h.step(Event::Restored { clock: crate::RecoveryClock::Monotonic });
    h.read(1);
    let sent = h.one();
    terminal(&h.answer(&sent, 1, Err(Error::Forbidden)), 1, Error::Forbidden);
    h.step(Event::Keep {
        owner: Token::new(9),
        watches: Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Issue(9) },
            participating: false,
        }]),
    });
    assert!(h.step(job(2)).is_empty(), "owing a background read retains the demand");
    let listing = h.one();
    assert_eq!(listing.op, Op::Read(Read::Items { since: at(u64::MAX), page: 1, kind: None }));
    h.answer(&listing, 1, Ok(Answer::Items { items: Box::new([]), more: false, now: h.env.now }));
    let item = h.one();
    assert_eq!(item.op, Op::Read(Read::Item { number: 9, after: u64::MAX }));
    h.answer(
        &item,
        1,
        Ok(Answer::Item { item: listing_item(9, 100, api::Kind::Issue), comments: Box::new([]), more: false }),
    );
    assert!(h.domain.is_ready(), "the completed background reads release admission");
    let sent = h.one();
    assert_eq!(sent.op, Op::Read(Read::Job { attempt: ATTEMPT, max_bytes: 64 }));
    assert_eq!(h.answer(&sent, 1, Ok(log())).as_ref(), &[Request::Read { owner: Token::new(2), result: Ok(log()) }]);
    assert!(h.send().is_none());
    assert_eq!(h.env.now, at(100));
}
