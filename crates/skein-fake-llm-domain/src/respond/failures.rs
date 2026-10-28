use super::respond;
use crate::api::{Answer, Error, Failure, Finish, Line, Part, Piece, Query, Report, Script, Scripts, Turn};
use crate::tests::{CONFIG, played, query, says, script, user};
use crate::{Domain, Event, Request, fire, step};
use alloc::boxed::Box;
use skein_lib::bytes::copy_of;
use skein_lib::{Duration, Env, Queue, ReplyTo, Rng, Time, Token, Wall};

const FIRST: u32 = 0;
const OTHER: u32 = 1;
const CALLS: u32 = 2; // The failed call and its retry.

fn report() -> Report {
    Report { code: copy_of(b"spent"), request: copy_of(b"req_42"), field: copy_of(b"top_p") }
}
fn fail(error: Error, credentials: Box<[u32]>) -> Line {
    Line::Fail { error, report: report(), credentials }
}
fn setup(lines: Box<[Line]>, credential: Option<u32>) -> (Query, [Script; 1]) {
    let mut query = query(Box::new([user(Box::new([Part::Text { text: copy_of(b"hi") }]))]));
    query.system = copy_of(b"@main");
    query.credential = credential;
    let turn = Turn { lines, ..says(b"") };
    (query, [script(b"@main", Box::new([turn]))])
}
fn reply(query: &Query, scripts: &[Script]) -> Result<(Answer, Option<crate::pieces::Place>), Failure> {
    match respond(
        &mut Rng::new(1),
        &mut 0,
        &CONFIG,
        &mut played(scripts),
        &crate::api::Menu { arguments: Box::new([]), invalid: Box::new([]) },
        query,
        Time::ZERO,
    ) {
        Ok(answer) => Ok(answer),
        Err((failure, _)) => Err(failure),
    }
}

#[test]
fn a_failing_line_answers_its_class_and_report() {
    let rows = [
        Error::Exhausted { retry_after: Some(Duration::from_secs(2)) },
        Error::Exhausted { retry_after: None },
        Error::Refused,
        Error::Unsupported,
        Error::Unauthorized,
    ];
    for error in rows {
        let (query, scripts) = setup(Box::new([fail(error, Box::new([]))]), None);
        assert_eq!(reply(&query, &scripts), Err(Failure { error, report: report(), parts: Box::new([]) }), "{error:?}");
    }
}

#[test]
fn a_failure_leaves_the_turn_for_the_call_made_again() {
    let (query, scripts) = setup(Box::new([fail(Error::Refused, Box::new([]))]), None);
    let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
    for _ in 0..CALLS {
        assert_eq!(
            call_at(&mut domain, &query, Time::ZERO),
            Err(Failure { error: Error::Refused, report: report(), parts: Box::new([]) })
        );
    }
}

fn call_at(domain: &mut Domain, query: &Query, now: Time) -> Result<Answer, Failure> {
    let env = Env { now, wall: Wall::EPOCH, limits: CONFIG };
    let mut out = Queue::uncapped();
    let call = Token::new(7);
    let reply_to = ReplyTo::new(call);
    step(domain, &env, Event::Call { reply_to, call, query: query.clone() }, &mut out);
    fire(domain, &env, &mut out);
    let Some(Request::Reply { to, result }) = out.pop() else { panic!("one terminal reply") };
    assert_eq!(to, ReplyTo::new(call));
    assert!(out.is_empty());
    domain.reclaim();
    result
}

fn assert_call(domain: &mut Domain, query: &Query, nanos: u64, failure: Option<Error>) {
    let got = call_at(domain, query, Time::from_nanos(nanos));
    if let Some(error) = failure {
        assert_eq!(got, Err(Failure { error, report: report(), parts: Box::new([]) }), "at {nanos}");
    } else {
        let answer = got.expect("the call passes over the expired line");
        assert_eq!(answer.parts.as_ref(), &[Part::Text { text: copy_of(b"through") }], "at {nanos}");
    }
}

const START: u64 = 5_000_000_000;
const BEFORE: u64 = 6_999_999_999;
const RESET: u64 = 7_000_000_000;
const AFTER: u64 = 8_000_000_000;
const LIMITED: Error = Error::Exhausted { retry_after: Some(Duration::from_secs(2)) };

#[test]
fn a_reset_fails_again_just_before_and_passes_at_and_after_its_boundary() {
    let (mut query, scripts) =
        setup(Box::new([fail(LIMITED, Box::new([FIRST])), Line::Text { text: copy_of(b"through") }]), Some(OTHER));
    let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
    // Calls the line does not name neither fail nor start its reset.
    assert_call(&mut domain, &query, 0, None);
    query.credential = None;
    assert_call(&mut domain, &query, 1, None);
    query.credential = Some(FIRST);
    for (now, error) in [(START, Some(LIMITED)), (BEFORE, Some(LIMITED)), (RESET, None), (AFTER, None)] {
        assert_call(&mut domain, &query, now, error);
    }
}

#[test]
fn failures_without_an_exhausted_reset_keep_failing_after_any_time() {
    for error in [
        Error::Exhausted { retry_after: None },
        Error::RateLimited { retry_after: Duration::from_secs(2) },
        Error::Refused,
        Error::Unsupported,
        Error::Unauthorized,
        Error::Overloaded,
        Error::Unavailable,
        Error::ContextTooLong,
        Error::InvalidRequest,
    ] {
        let (query, scripts) = setup(Box::new([fail(error, Box::new([]))]), Some(FIRST));
        let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
        for now in [START, BEFORE, RESET, u64::MAX] {
            assert_call(&mut domain, &query, now, Some(error));
        }
    }
}

#[test]
fn named_credentials_have_independent_resets() {
    let (mut query, scripts) = setup(
        Box::new([fail(LIMITED, Box::new([FIRST, OTHER])), Line::Text { text: copy_of(b"through") }]),
        Some(FIRST),
    );
    let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
    for (credential, now, failure) in [
        (FIRST, START, Some(LIMITED)),
        (OTHER, 6_000_000_000, Some(LIMITED)),
        (FIRST, RESET, None),
        (OTHER, 7_999_999_999, Some(LIMITED)),
        (OTHER, AFTER, None),
    ] {
        query.credential = Some(credential);
        assert_call(&mut domain, &query, now, failure);
    }
}

#[test]
fn a_line_naming_no_credentials_shares_its_reset_across_calls() {
    let (mut query, scripts) = setup(Box::new([fail(LIMITED, Box::new([]))]), None);
    let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
    assert_call(&mut domain, &query, START, Some(LIMITED));
    query.credential = Some(FIRST);
    assert_call(&mut domain, &query, BEFORE, Some(LIMITED));
    for now in [RESET, AFTER] {
        assert!(call_at(&mut domain, &query, Time::from_nanos(now)).expect("expired line").parts.is_empty());
    }
}

#[test]
fn each_line_starts_its_reset_only_when_it_fails_a_call() {
    let (query, scripts) = setup(
        Box::new([
            fail(LIMITED, Box::new([FIRST])),
            fail(LIMITED, Box::new([FIRST])),
            Line::Text { text: copy_of(b"through") },
        ]),
        Some(FIRST),
    );
    let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
    for (now, error) in
        [(START, Some(LIMITED)), (RESET, Some(LIMITED)), (8_999_999_999, Some(LIMITED)), (9_000_000_000, None)]
    {
        assert_call(&mut domain, &query, now, error);
    }
}

#[test]
fn a_zero_reset_fails_once_and_an_overflowing_reset_does_not_expire_early() {
    for (delay, first, retry, fails) in
        [(Duration::ZERO, START, START, false), (Duration::from_nanos(2), u64::MAX - 1, u64::MAX, true)]
    {
        let error = Error::Exhausted { retry_after: Some(delay) };
        let (query, scripts) = setup(Box::new([fail(error, Box::new([]))]), None);
        let mut domain = Domain::scripted(&CONFIG, 1, scripts.into());
        assert_call(&mut domain, &query, first, Some(error));
        assert_eq!(call_at(&mut domain, &query, Time::from_nanos(retry)).is_err(), fails);
    }
}

#[test]
fn every_script_group_retains_its_reset() {
    let (query, scripts) =
        setup(Box::new([fail(LIMITED, Box::new([FIRST])), Line::Text { text: copy_of(b"through") }]), Some(FIRST));
    let [script] = scripts;
    for scripts in [
        Scripts { cued: Box::new([script.clone()]), asked: Box::new([]), otherwise: Box::new([]) },
        Scripts {
            cued: Box::new([]),
            asked: Box::new([Script { cue: copy_of(b"hi"), ..script.clone() }]),
            otherwise: Box::new([]),
        },
        Scripts { cued: Box::new([]), asked: Box::new([]), otherwise: script.turns },
    ] {
        let mut domain = Domain::played(&CONFIG, 1, scripts, crate::tests::empty_menu()).expect("admitted scripts");
        for (now, error) in [(START, Some(LIMITED)), (BEFORE, Some(LIMITED)), (RESET, None)] {
            assert_call(&mut domain, &query, now, error);
        }
    }
}

#[test]
fn reset_state_fits_the_script_byte_cap_at_admission() {
    for credentials in [Box::<[u32]>::from([]), Box::from([FIRST, OTHER])] {
        let reset_slots = credentials.len().max(1);
        let cap = size_of::<Script>()
            + b"@main".len()
            + size_of::<Turn>()
            + size_of::<Line>()
            + b"spent".len()
            + b"req_42".len()
            + b"top_p".len()
            + credentials.len() * size_of::<u32>()
            + reset_slots * size_of::<super::Reset>();
        for (bytes, expected) in [(cap - 1, Some(Error::ContextTooLong)), (cap, None)] {
            let (_, scripts) = setup(Box::new([fail(LIMITED, credentials.clone())]), Some(FIRST));
            let config = crate::Config { script_bytes: u32::try_from(bytes).expect("fixture cap"), ..CONFIG };
            assert_eq!(
                Domain::try_scripted(&config, 1, scripts.into()).err(),
                expected,
                "{reset_slots} resets, {bytes} bytes"
            );
        }
    }
}

#[test]
fn a_failing_line_naming_credentials_fails_only_their_calls() {
    let rows: [(Box<[u32]>, _, _); 5] = [
        (Box::new([OTHER]), Some(OTHER), true),
        (Box::new([OTHER]), Some(FIRST), false),
        (Box::new([OTHER]), None, false),
        (Box::new([]), Some(FIRST), true),
        (Box::new([]), None, true),
    ];
    for (credentials, credential, fails) in rows {
        let mut lines = skein_lib::List::with_capacity(2);
        lines.push(fail(Error::Refused, credentials.clone())).expect("room for failing line");
        if !credentials.is_empty() {
            lines.push(Line::Text { text: copy_of(b"through") }).expect("room for text");
        }
        let (query, scripts) = setup(lines.into_boxed(), credential);
        let expected: Result<(Box<[Part]>, Finish), Failure> = if fails {
            Err(Failure { error: Error::Refused, report: report(), parts: Box::new([]) })
        } else {
            Ok((Box::new([Part::Text { text: copy_of(b"through") }]), Finish::Stop))
        };
        let got = match reply(&query, &scripts) {
            Ok((answer, _)) => Ok((answer.parts, answer.finish)),
            Err(failure) => Err(failure),
        };
        assert_eq!(got, expected, "{credentials:?} {credential:?}");
    }
}

#[test]
fn a_failing_line_after_text_keeps_the_text_before_it() {
    let rows = [
        Line::Text { text: copy_of(b"before") },
        Line::Pieces {
            pieces: Box::new([
                Piece { text: copy_of(b"be"), hold: None },
                Piece { text: copy_of(b"fore"), hold: None },
            ]),
        },
    ];
    for line in rows {
        let (query, scripts) = setup(Box::new([line, fail(Error::Refused, Box::new([]))]), None);
        assert_eq!(
            reply(&query, &scripts),
            Err(Failure {
                error: Error::Refused,
                report: report(),
                parts: Box::new([Part::Text { text: copy_of(b"before") }])
            })
        );
    }
}
