use super::fixtures::outbox::{Harness, comment};
use super::{LIMITS, REPO, configured};
use crate::{Condition, Config, Domain, Event, Limits, Outcome, RecoveryClock, Request, Stored, Writer};
use alloc::boxed::Box;
use skein_forge::api::{Error, Op};
use skein_lib::{Env, List, Queue};

const WRITER: Writer = Writer { forge: REPO.forge, author: 1 };
const ANOTHER_AUTHOR: Writer = Writer { author: 2, ..WRITER };
const ZERO_AUTHOR: Writer = Writer { author: 0, ..WRITER };
const OTHER_FORGE: Writer = Writer { forge: 3, ..WRITER };
const EXCESS_FORGE: Writer = Writer { forge: 4, ..WRITER };
// A writer can be known from configuration or a runtime gift.
const WRITER_SOURCES: usize = 2;
// Conflicting configured or restored authors, zero, capacity and no configuration.
const FAILED_RESTORE_CASES: usize = 5;

fn writerless(l: &Limits) -> Domain {
    Domain::configured(l, 0, Config { namespace: Box::from(&b"kit"[..]), writers: Box::new([]) })
        .expect("namespace without writers is valid")
}

fn give(d: &mut Domain, env: &Env<Limits>, writer: Writer) -> (bool, Box<[Request]>) {
    let mut out = Queue::uncapped();
    let given = crate::writer(d, env, writer, &mut out);
    let mut requests = List::with_capacity(out.len());
    while let Some(request) = out.pop() {
        requests.push(request).expect("measured output fits");
    }
    (given, requests.into_boxed())
}

fn known_writers(env: &Env<Limits>) -> [(&'static str, Domain); WRITER_SOURCES] {
    let mut given = writerless(&env.limits);
    drop(give(&mut given, env, WRITER));
    [("configured", configured(&env.limits)), ("given", given)]
}

fn writerless_harness() -> Harness {
    let mut h = Harness::new();
    h.domain = writerless(&h.env.limits);
    h
}

#[test]
fn a_writer_given_at_runtime_is_saved_as_a_row() {
    let mut h = writerless_harness();
    let (given, requests) = give(&mut h.domain, &h.env, WRITER);
    assert!(given);
    assert_eq!(requests.as_ref(), &[Request::Save { record: Stored::Writer(WRITER) }]);
}

#[test]
fn the_same_writer_again_changes_nothing() {
    let h = writerless_harness();
    for (name, mut d) in known_writers(&h.env) {
        let (accepted, requests) = give(&mut d, &h.env, WRITER);
        assert!(accepted, "{name}");
        assert!(requests.is_empty(), "{name}");
    }
}

#[test]
fn another_author_for_a_forge_is_refused() {
    let h = writerless_harness();
    for (name, mut d) in known_writers(&h.env) {
        let (accepted, requests) = give(&mut d, &h.env, ANOTHER_AUTHOR);
        assert!(!accepted, "{name}");
        assert!(requests.is_empty(), "{name}");
        let (accepted, requests) = give(&mut d, &h.env, WRITER);
        assert!(accepted, "{name}");
        assert!(requests.is_empty(), "{name}");
    }
}

#[test]
fn a_writer_that_cannot_be_held_is_refused() {
    let h = writerless_harness();
    let mut full = writerless(&LIMITS);
    for writer in [WRITER, OTHER_FORGE] {
        drop(give(&mut full, &h.env, writer));
    }
    for (name, mut d, writer) in [
        ("author zero", writerless(&LIMITS), ZERO_AUTHOR),
        ("table full", full, EXCESS_FORGE),
        ("no configuration", Domain::new(&LIMITS), WRITER),
    ] {
        let (accepted, requests) = give(&mut d, &h.env, writer);
        assert!(!accepted, "{name}");
        assert!(requests.is_empty(), "{name}");
    }
}

#[test]
fn an_effect_is_forbidden_until_its_forges_writer_is_given() {
    let mut h = writerless_harness();
    h.step(Event::Restored { clock: RecoveryClock::Monotonic });
    drop(give(&mut h.domain, &h.env, OTHER_FORGE));
    h.make(1, comment(), Condition::None);
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Failed(Error::Forbidden))]);
    assert!(h.send().is_none());
    drop(give(&mut h.domain, &h.env, WRITER));
    h.make(2, comment(), Condition::None);
    h.clock();
    assert_eq!(h.one().op, Op::Write(comment()));
}

#[test]
fn a_restored_writer_row_admits_its_forges_effects() {
    let mut h = writerless_harness();
    h.step(Event::Restore { record: Stored::Writer(WRITER) });
    h.step(Event::Restored { clock: RecoveryClock::Monotonic });
    assert!(!h.domain.restoration_failed());
    h.make(1, comment(), Condition::None);
    h.clock();
    assert_eq!(h.one().op, Op::Write(comment()));
}

#[test]
fn a_writer_row_that_cannot_be_held_fails_restoration() {
    let cases: [(&str, Domain, Box<[Writer]>); FAILED_RESTORE_CASES] = [
        ("configured author differs", configured(&LIMITS), Box::new([ANOTHER_AUTHOR])),
        ("earlier author differs", writerless(&LIMITS), Box::new([WRITER, ANOTHER_AUTHOR])),
        ("author zero", writerless(&LIMITS), Box::new([ZERO_AUTHOR])),
        ("table full", writerless(&LIMITS), Box::new([WRITER, OTHER_FORGE, EXCESS_FORGE])),
        ("no configuration", Domain::new(&LIMITS), Box::new([WRITER])),
    ];
    for (name, domain, rows) in cases {
        let mut h = Harness::new();
        h.domain = domain;
        for writer in rows {
            h.step(Event::Restore { record: Stored::Writer(writer) });
        }
        h.step(Event::Restored { clock: RecoveryClock::Monotonic });
        assert!(h.domain.restoration_failed(), "{name}");
    }
}
