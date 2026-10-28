pub(super) mod keep;
pub(super) mod outbox;
pub(super) mod recovery;
use super::REPO;
use crate::{Domain, Event, Fact, Limits, Priority, Request, calls, fire, resume, step};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read, Repository, Write};
use skein_lib::{Env, List, Queue, Time, Token, Wall};
pub(in crate::tests) fn at(seconds: u64) -> Time {
    Time::from_nanos(seconds.saturating_mul(1_000_000_000))
}
#[derive(Debug)]
pub(in crate::tests) struct Sent {
    pub(in crate::tests) call: Token,
    pub(in crate::tests) op: Op,
}
const CALL_ROOM: u32 = 1; // resume emits one call at a time
#[derive(Debug)]
pub(in crate::tests) struct Harness {
    pub(in crate::tests) domain: Domain,
    pub(in crate::tests) env: Env<Limits>,
}
impl Harness {
    pub(in crate::tests) fn new(limits: Limits) -> Harness {
        Harness { domain: Domain::new(&limits), env: Env { now: at(100), wall: Wall::EPOCH, limits } }
    }
    pub(in crate::tests) fn step(&mut self, event: Event) -> Box<[Request]> {
        let mut out = Queue::uncapped();
        step(&mut self.domain, &self.env, event, &mut out);
        self.take(&mut out)
    }
    fn take(&mut self, out: &mut Queue<Request>) -> Box<[Request]> {
        self.domain.reclaim();
        let mut got = List::with_capacity(out.len());
        for _ in 0..out.len() {
            got.push(out.pop().expect("request exists")).expect("room");
        }
        got.into_boxed()
    }
    pub(in crate::tests) fn read(&mut self, owner: u64) {
        assert!(
            self.step(Event::Read { owner: Token::new(owner), repository: REPO, read: Read::Pull { number: owner } })
                .is_empty()
        );
    }
    pub(in crate::tests) fn queued(&mut self, owner: u64, priority: Priority) {
        let op = if priority == Priority::Write {
            Op::Write(Write::Close { number: owner })
        } else {
            Op::Read(Read::Pull { number: owner })
        };
        assert!(crate::bounds::op(&op, &self.env.limits));
        calls::queue(&mut self.domain.calls, calls::Owner::Read(Token::new(owner)), REPO, op, priority);
    }
    pub(in crate::tests) fn send(&mut self) -> Option<Sent> {
        let mut out = Queue::with_capacity(CALL_ROOM);
        resume(&mut self.domain, &self.env, &mut out);
        match out.pop() {
            Some(Request::Call { call, repository, op }) => {
                assert_eq!(repository, REPO);
                Some(Sent { call, op })
            }
            Some(
                Request::Read { .. }
                | Request::Progress { .. }
                | Request::Save { .. }
                | Request::Erase { .. }
                | Request::Outcome { .. }
                | Request::Kept { .. }
                | Request::Changed { .. }
                | Request::Drift { .. }
                | Request::ReadAfreshDone
                | Request::Settled { .. }
                | Request::OutboxDone,
            ) => {
                panic!("read resume only sends a call")
            }
            None => None,
        }
    }
    pub(in crate::tests) fn sent(&mut self) -> (Sent, Repository) {
        let mut out = Queue::with_capacity(CALL_ROOM);
        resume(&mut self.domain, &self.env, &mut out);
        let Some(Request::Call { call, repository, op }) = out.pop() else {
            panic!("a locate sends its call");
        };
        (Sent { call, op }, repository)
    }
    pub(in crate::tests) fn one(&mut self) -> Sent {
        self.send().expect("call ready")
    }
    pub(in crate::tests) fn answer(&mut self, sent: &Sent, cost: u32, result: Result<Answer, Error>) -> Box<[Request]> {
        self.answer_free(sent, cost, 0, result)
    }
    pub(in crate::tests) fn answer_free(
        &mut self,
        sent: &Sent,
        cost: u32,
        free: u32,
        result: Result<Answer, Error>,
    ) -> Box<[Request]> {
        self.step(Event::Answered { call: sent.call, cost, free, rate: None, result })
    }
    pub(in crate::tests) fn answer_rate(
        &mut self,
        sent: &Sent,
        cost: u32,
        rate: api::Rate,
        result: Result<Answer, Error>,
    ) -> Box<[Request]> {
        self.step(Event::Answered { call: sent.call, cost, free: 0, rate: Some(rate), result })
    }
    pub(in crate::tests) fn rate(&self, remaining: u32, reset_secs: u64) -> api::Rate {
        let reset_at =
            Wall::from_nanos(self.env.wall.as_nanos().saturating_add(reset_secs.saturating_mul(1_000_000_000)));
        api::Rate { limit: 5000, remaining, reset_at }
    }
    pub(in crate::tests) fn fire(&mut self) {
        let mut out = Queue::with_capacity(CALL_ROOM);
        fire(&mut self.domain, &self.env, &mut out);
        assert!(out.is_empty());
    }
    pub(in crate::tests) fn facts(&mut self) -> Box<[Fact]> {
        let mut facts = List::with_capacity(self.env.limits.facts);
        for _ in 0..self.env.limits.facts {
            match self.domain.pop_fact() {
                Some(fact) => facts.push(fact).expect("room"),
                None => break,
            }
        }
        facts.into_boxed()
    }
}
pub(in crate::tests) fn terminal(requests: &[Request], owner: u64, error: Error) {
    assert_eq!(requests, &[Request::Read { owner: Token::new(owner), result: Err(error) }]);
}

pub(in crate::tests) fn configured(limits: &Limits) -> Domain {
    Domain::configured(
        limits,
        0,
        crate::Config {
            namespace: Box::from(&b"kit"[..]),
            writers: Box::new([crate::Writer { forge: REPO.forge, author: 1 }]),
        },
    )
    .expect("valid root identity")
}
pub(in crate::tests) fn listing_item(number: u64, updated: u64, kind: api::Kind) -> api::Summary {
    api::Summary {
        number,
        kind,
        state: api::State::Open,
        author: 2,
        key: None,
        title: Box::new([]),
        body: Box::new([]),
        labels: Box::new([]),
        updated: at(updated),
    }
}

pub(super) fn entry(write: Write) -> crate::Entry {
    crate::Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: crate::Effect { write, condition: crate::Condition::None },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    }
}
