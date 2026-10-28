use super::super::{LIMITS, REPO, Sent, at};
use crate::{
    Condition, Domain, Effect, Entry, Event, Limits, Outcome, RecoveryClock, Request, fire, resume, step,
};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read, Write};
use skein_lib::{Env, List, Map, Queue, Wall};
// Observation room for twenty outcomes and eight queued calls in outbox stories.
const OUTCOME_ROOM: u32 = 20;
const CALL_ROOM: u32 = 8;
const REFUSED_ROOM: u32 = 1;
#[derive(Debug)]
pub(in crate::tests) struct Harness {
    pub(in crate::tests) domain: Domain,
    pub(in crate::tests) env: Env<Limits>,
    pub(in crate::tests) store: Map<u64, Entry>,
    pub(in crate::tests) outcomes: List<(u64, Outcome)>,
    pub(in crate::tests) calls: Queue<Sent>,
    pub(in crate::tests) writes: u32,
    pub(in crate::tests) outbox_done: u32,
    pub(in crate::tests) settled: Option<Box<[u64]>>,
}
impl Harness {
    pub(in crate::tests) fn new() -> Harness {
        let mut h = Harness {
            domain: super::super::configured(&LIMITS),
            env: Env {
                now: at(100),
                wall: Wall::from_nanos(at(100).as_nanos()),
                limits: Limits { rate: 100, reserve: 0, ..LIMITS },
            },
            store: Map::with_capacity(LIMITS.entries + REFUSED_ROOM),
            outcomes: List::with_capacity(OUTCOME_ROOM),
            calls: Queue::with_capacity(CALL_ROOM),
            writes: 0,
            outbox_done: 0,
            settled: None,
        };
        h.step(Event::Restored { clock: RecoveryClock::Monotonic });
        h
    }
    pub(in crate::tests) fn step(&mut self, event: Event) {
        let mut out = Queue::uncapped();
        step(&mut self.domain, &self.env, event, &mut out);
        self.take(&mut out);
        self.domain.reclaim();
    }
    pub(in crate::tests) fn take(&mut self, out: &mut Queue<Request>) {
        for _ in 0..out.len() {
            match out.pop().expect("output exists") {
                Request::Progress { entry } => {
                    self.store.insert(entry.number, entry).expect("store fits");
                }
                Request::Outcome { entry, task: _, outcome } => {
                    match outcome {
                        Outcome::Uncertain | Outcome::Queued => {}
                        Outcome::Made { .. }
                        | Outcome::Failed(_)
                        | Outcome::Raced { .. }
                        | Outcome::Held
                        | Outcome::Withdrawn => {
                            self.store.remove(&entry).expect("top removes settled entry");
                        }
                    }
                    self.outcomes.push((entry, outcome)).expect("outcome fits");
                }
                Request::Call { call, repository, op } => {
                    assert_eq!(repository, REPO);
                    match &op {
                        Op::Write(write) => {
                            let mut durable = false;
                            for (_, entry) in &self.store {
                                if entry.effect.write == *write {
                                    let attempt = entry.attempt.expect("attempt durable before call starts");
                                    assert_eq!(attempt.sent, self.env.now);
                                    assert!(entry.start.is_some());
                                    durable = true;
                                }
                            }
                            assert!(durable, "a durable entry authorizes this exact write");
                            self.writes = self.writes.saturating_add(1);
                        }
                        Op::Read(_) => {}
                    }
                    self.calls.push(Sent { call, op });
                }
                Request::Save { .. }
                | Request::Erase { .. }
                | Request::Kept { .. }
                | Request::Changed { .. }
                | Request::Drift { .. } => panic!("outbox tests keep no resources"),
                Request::Read { .. } => panic!("outbox tests make no parent read requests"),
                Request::Settled { left } => {
                    assert!(self.settled.replace(left).is_none(), "settled once");
                }
                Request::OutboxDone => self.outbox_done = self.outbox_done.saturating_add(1),
                Request::ReadAfreshDone => {
                    panic!("ordinary outbox test did not request restart phases")
                }
            }
        }
    }
    pub(in crate::tests) fn send(&mut self) -> Option<Sent> {
        let mut out = Queue::uncapped();
        resume(&mut self.domain, &self.env, &mut out);
        self.take(&mut out);
        self.domain.reclaim();
        self.calls.pop()
    }
    pub(in crate::tests) fn one(&mut self) -> Sent {
        self.send().expect("call ready")
    }
    pub(in crate::tests) fn answer(&mut self, sent: Sent, result: Result<Answer, Error>) {
        self.step(Event::Answered { call: sent.call, cost: 1, free: 0, rate: None, result });
    }
    pub(in crate::tests) fn clock(&mut self) {
        let sent = self.one();
        assert_eq!(sent.op, Op::Read(Read::Items { since: at(u64::MAX), page: 1, kind: None }));
        self.answer(sent, Ok(Answer::Items { items: Box::new([]), more: false, now: at(100) }));
    }
    pub(in crate::tests) fn make(&mut self, number: u64, write: Write, condition: Condition) {
        let entry = Entry {
            number,
            task: 7,
            repository: REPO,
            effect: Effect { write, condition },
            start: None,
            attempt: None,
            failures: 0,
            queued: false,
        };
        self.store.insert(number, entry.clone()).expect("top store fits");
        self.step(Event::Make { entry });
    }
    pub(in crate::tests) fn restart(&mut self, now: u64, wall: u64, clock: RecoveryClock) {
        self.domain = super::super::configured(&self.env.limits);
        self.env.now = at(now);
        self.env.wall = Wall::from_nanos(at(wall).as_nanos());
        let mut records = List::with_capacity(LIMITS.entries);
        for (_, entry) in &self.store {
            records.push(entry.clone()).expect("records fit");
        }
        self.step(Event::Restored { clock });
        for entry in records.into_boxed() {
            self.step(Event::Make { entry });
        }
    }
    pub(in crate::tests) fn fire(&mut self, now: u64) {
        self.env.now = at(now);
        let mut out = Queue::uncapped();
        fire(&mut self.domain, &self.env, &mut out);
        self.take(&mut out);
        self.domain.reclaim();
    }
}
pub(in crate::tests) fn comment() -> Write {
    Write::Post { number: 9, key: Box::from(&b"\x01\x00\x03kit:task:milestone"[..]), body: Box::from(&b"done"[..]) }
}
pub(in crate::tests) fn summary() -> api::Summary {
    api::Summary {
        number: 9,
        kind: api::Kind::Issue,
        state: api::State::Open,
        author: 1,
        key: None,
        labels: Box::new([]),
        title: Box::new([]),
        body: Box::new([]),
        updated: at(100),
    }
}
pub(in crate::tests) fn pull(head: [u8; 32]) -> api::Pull {
    api::Pull {
        queued: None,
        dequeued: None,
        number: 9,
        state: api::State::Open,
        head: Box::from(&b"topic"[..]),
        base: Box::from(&b"main"[..]),
        commit: head,
        base_commit: Some([2; 32]),
        merged: None,
        mergeable: true,
        ci: api::Ci::Passed,
        reviewers: Box::new([]),
    }
}
pub(in crate::tests) fn recovering_review() -> Harness {
    let mut h = Harness::new();
    h.make(
        1,
        Write::Review {
            number: 9,
            key: Box::from(&b"\x01\x00\x03kitreview-key"[..]),
            verdict: api::Verdict::Approve,
            body: Box::from(&b"looks good"[..]),
        },
        Condition::Review { head: [1; 32] },
    );
    h.clock();
    let check = h.one();
    h.answer(check, Ok(Answer::Pull(pull([1; 32]))));
    let write = h.one();
    h.answer(write, Err(Error::Timeout));
    // Restore the durable boundary a participating inbox captured before
    // submission. The outbox-only harness does not keep a live resource.
    h.store.get_mut(&1).expect("uncertain entry remains").start.as_mut().expect("durable start").review = 5;
    h.restart(105, 105, RecoveryClock::Monotonic);
    h
}
pub(in crate::tests) fn recovered_review(id: u64, provenance: api::Provenance, commit: [u8; 32]) -> api::Review {
    api::Review {
        provenance,
        id,
        revision: 1,
        author: 1,
        verdict: api::Verdict::Approve,
        commit,
        key: Some(Box::from(&b"\x01\x00\x03kitreview-key"[..])),
        body: Box::new([]),
        at: at(100),
        official: true,
    }
}
