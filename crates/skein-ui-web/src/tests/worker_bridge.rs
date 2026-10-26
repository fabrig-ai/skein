use crate::{
    bridge::{Duty, Line, Malformed, Output, Said, Stamp, Start, ToPage, ToWorld, decode_to_world, encode_to_page},
    fetch::{Exchange, Report},
    platform::worker::{Bridge, Clock, Figures, Heard, Limits, Speed},
    store::Record,
    transport::StreamDuty,
};
use skein_lib::{Duration, Time, Wall};

const BROWSER_START: u64 = 1_000;
const WALL_START: u64 = 10_000;
const BUILD: &[u8] = b"bridge-test-build";
const LOG_BYTES: u32 = 16 * 1024; // Room for the finite bridge fixtures.
const APPLICATION_ID: u32 = 73;

fn at(nanos: u64) -> Time {
    Time::from_nanos(nanos)
}

fn start() -> Start {
    Start { story: b"story-a".as_slice().into(), seed: 42, start: Wall::from_nanos(WALL_START) }
}

fn outgoing(bridge: &mut Bridge) -> ToWorld {
    decode_to_world(&bridge.outgoing().expect("a message for the world")).expect("valid bridge message")
}

fn bridge(exchanges: u32, speed: u32) -> Bridge {
    let mut bridge = Bridge::new(start(), BUILD, Limits { exchanges, log: LOG_BYTES }, Speed(speed), at(BROWSER_START));
    assert_eq!(outgoing(&mut bridge), ToWorld::Start(start()));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(speed)));
    assert_eq!(bridge.outgoing(), None);
    bridge
}

fn clock() -> Clock {
    Clock {
        step: 7,
        now: at(100),
        wall: Wall::from_nanos(WALL_START + 100),
        idle: false,
        due: None,
        memory: 0,
        slice: Duration::ZERO,
    }
}

fn incoming(bridge: &mut Bridge, said: Said) -> Heard {
    bridge.incoming(&encode_to_page(&ToPage { clock: clock(), said }), at(BROWSER_START)).expect("valid message")
}

#[test]
fn the_page_keeps_the_figures_high_water() {
    let mut bridge = bridge(1, 1);
    assert_eq!(bridge.figures(), Figures { memory: 0, slice: Duration::ZERO });
    let file = bridge.log();
    for (memory, nanos, most_memory, longest) in
        [(65_536, 3, 65_536, 3), (32_768, 9, 65_536, 9), (131_072, 5, 131_072, 9), (0, 0, 131_072, 9)]
    {
        let clock = Clock { memory, slice: Duration::from_nanos(nanos), ..clock() };
        let bytes = encode_to_page(&ToPage { clock, said: Said::Slice });
        assert_eq!(bridge.incoming(&bytes, at(BROWSER_START)), Ok(Heard::Nothing));
        assert_eq!(bridge.figures(), Figures { memory: most_memory, slice: Duration::from_nanos(longest) });
    }
    let before = bridge.figures();
    let clock = Clock { memory: u64::MAX, slice: Duration::from_nanos(u64::MAX), ..clock() };
    let mut bytes = encode_to_page(&ToPage { clock, said: Said::Slice }).into_vec();
    bytes.push(255);
    assert_eq!(bridge.incoming(&bytes, at(BROWSER_START)), Err(Malformed));
    assert_eq!(bridge.figures(), before);
    assert_eq!(bridge.log(), file, "clocks never enter the session log");
}

fn exchange() -> Exchange {
    Exchange {
        id: APPLICATION_ID,
        path: b"/visit?x=1".as_slice().into(),
        method: "POST".into(),
        headers: vec![("content-type".into(), "text/plain".into())],
        body: b"request".as_slice().into(),
        bound: 3,
    }
}

fn send(bridge: &mut Bridge, exchange: &Exchange) -> u32 {
    assert_eq!(bridge.send(exchange), None);
    let ToWorld::Duty(Duty::Exchange(sent)) = outgoing(bridge) else {
        panic!("the exchange goes to the world");
    };
    assert_eq!(bridge.outgoing(), None);
    let ticket = sent.id;
    let mut expected = exchange.clone();
    expected.id = ticket;
    assert_eq!(sent, expected);
    ticket
}

fn answer(ticket: u32, body: &[u8]) -> Said {
    Said::Report(Report::Answered { exchange: ticket, status: 201, body: body.into() })
}

fn open(bound: u32) -> StreamDuty {
    StreamDuty::Open {
        path: b"/events".as_slice().into(),
        headers: vec![(b"last-event-id".as_slice().into(), b"42".as_slice().into())],
        bound,
    }
}

fn open_stream(bridge: &mut Bridge, bound: u32) {
    assert_eq!(bridge.stream(open(bound)), None);
    assert_eq!(outgoing(bridge), ToWorld::Duty(Duty::Stream(open(bound))));
    assert_eq!(bridge.outgoing(), None);
}

#[test]
fn the_start_is_the_first_message() {
    let mut bridge = Bridge::new(start(), BUILD, Limits { exchanges: 1, log: LOG_BYTES }, Speed(10), at(BROWSER_START));
    assert_eq!(bridge.now(at(BROWSER_START + 100)), (Time::ZERO, Wall::from_nanos(WALL_START)));
    assert_eq!(bridge.timer(at(1)), None);
    assert_eq!(outgoing(&mut bridge), ToWorld::Start(start()));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(10)));
    assert_eq!(bridge.outgoing(), None);
}

#[test]
fn an_exchange_goes_out_under_its_ticket_and_comes_back_under_its_id() {
    let mut bridge = bridge(1, 1);
    let exchange = exchange();
    let ticket = send(&mut bridge, &exchange);
    assert_ne!(ticket, exchange.id);
    assert_eq!(exchange.id, APPLICATION_ID);
    assert_eq!(
        incoming(&mut bridge, answer(ticket, b"yes")),
        Heard::Report(Report::Answered { exchange: APPLICATION_ID, status: 201, body: b"yes".as_slice().into() })
    );
    assert_eq!(incoming(&mut bridge, answer(ticket, b"yes")), Heard::Nothing);
    let next = send(&mut bridge, &exchange);
    assert_ne!(next, ticket);
    assert_eq!(
        incoming(&mut bridge, Said::Report(Report::Failed { exchange: next })),
        Heard::Report(Report::Failed { exchange: APPLICATION_ID })
    );
    assert_eq!(incoming(&mut bridge, Said::Report(Report::Failed { exchange: next })), Heard::Nothing);
}

#[test]
fn an_exchange_past_the_bound_fails_at_once_and_sends_nothing() {
    let mut bridge = bridge(1, 1);
    let ticket = send(&mut bridge, &exchange());
    let second = Exchange { id: APPLICATION_ID + 1, ..exchange() };
    assert_eq!(bridge.send(&second), Some(Report::Failed { exchange: second.id }));
    assert_eq!(bridge.outgoing(), None);
    assert_eq!(
        incoming(&mut bridge, answer(ticket, b"yes")),
        Heard::Report(Report::Answered { exchange: APPLICATION_ID, status: 201, body: b"yes".as_slice().into() })
    );
    send(&mut bridge, &second);
}

#[test]
fn an_answer_past_its_bound_fails() {
    let mut bridge = bridge(1, 1);
    let ticket = send(&mut bridge, &exchange());
    assert_eq!(
        incoming(&mut bridge, answer(ticket, b"four")),
        Heard::Report(Report::Failed { exchange: APPLICATION_ID })
    );
    assert_eq!(incoming(&mut bridge, answer(ticket, b"yes")), Heard::Nothing);
    send(&mut bridge, &exchange());
}

#[test]
fn an_answer_crossing_its_abandon_is_nothing() {
    let mut bridge = bridge(1, 1);
    let ticket = send(&mut bridge, &exchange());
    bridge.abandon(APPLICATION_ID);
    assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::Abandon { exchange: ticket }));
    assert_eq!(bridge.outgoing(), None);
    assert_eq!(incoming(&mut bridge, answer(ticket, b"yes")), Heard::Nothing);
    assert_eq!(incoming(&mut bridge, Said::Report(Report::Failed { exchange: ticket })), Heard::Nothing);
    bridge.abandon(APPLICATION_ID);
    assert_eq!(bridge.outgoing(), None);
}

#[test]
fn an_id_reused_after_an_abandon_hears_only_its_own_answer() {
    let mut bridge = bridge(1, 1);
    let old = send(&mut bridge, &exchange());
    bridge.abandon(APPLICATION_ID);
    assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::Abandon { exchange: old }));
    let current = send(&mut bridge, &exchange());
    assert_ne!(current, old);
    assert_eq!(incoming(&mut bridge, answer(old, b"old")), Heard::Nothing);
    assert_eq!(
        incoming(&mut bridge, answer(current, b"new")),
        Heard::Report(Report::Answered { exchange: APPLICATION_ID, status: 201, body: b"new".as_slice().into() })
    );
}

#[test]
fn a_closed_streams_late_pieces_are_nothing() {
    let mut bridge = bridge(1, 1);
    open_stream(&mut bridge, 2);
    assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamOpened)), Heard::Report(Report::StreamOpened));
    assert_eq!(bridge.stream(StreamDuty::Close), Some(Report::StreamEnded));
    assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::Stream(StreamDuty::Close)));
    assert_eq!(bridge.outgoing(), None);
    assert_eq!(
        incoming(&mut bridge, Said::Report(Report::StreamPiece { bytes: b"old".as_slice().into() })),
        Heard::Nothing
    );
    assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamEnded)), Heard::Nothing);
    assert_eq!(bridge.read_on(), None);
    assert_eq!(bridge.outgoing(), None);
    assert_eq!(bridge.stream(StreamDuty::Close), None);
    assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::Stream(StreamDuty::Close)));
}

#[test]
fn a_stream_reopened_hears_only_the_new_streams_reports() {
    for terminal in [Report::StreamEnded, Report::StreamRefused { status: 403 }] {
        let mut bridge = bridge(1, 1);
        open_stream(&mut bridge, 2);
        assert_eq!(bridge.stream(open(3)), Some(Report::StreamEnded));
        assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::Stream(open(3))));
        assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamOpened)), Heard::Nothing);
        assert_eq!(
            incoming(&mut bridge, Said::Report(Report::StreamPiece { bytes: b"old".as_slice().into() })),
            Heard::Nothing
        );
        assert_eq!(incoming(&mut bridge, Said::Report(terminal)), Heard::Nothing);
        assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamOpened)), Heard::Report(Report::StreamOpened));
        assert_eq!(
            incoming(&mut bridge, Said::Report(Report::StreamPiece { bytes: b"new".as_slice().into() })),
            Heard::Report(Report::StreamPiece { bytes: b"new".as_slice().into() })
        );
        assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamEnded)), Heard::Report(Report::StreamEnded));
        assert_eq!(bridge.read_on(), None);
        assert_eq!(bridge.outgoing(), None);
    }
}

#[test]
fn a_read_on_goes_out_after_a_piece() {
    let mut bridge = bridge(1, 1);
    open_stream(&mut bridge, 2);
    assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamOpened)), Heard::Report(Report::StreamOpened));
    assert_eq!(
        incoming(&mut bridge, Said::Report(Report::StreamPiece { bytes: b"abcde".as_slice().into() })),
        Heard::Report(Report::StreamPiece { bytes: b"ab".as_slice().into() })
    );
    for bytes in [b"cd".as_slice(), b"e".as_slice()] {
        assert_eq!(bridge.read_on(), Some(Report::StreamPiece { bytes: bytes.into() }));
        assert_eq!(bridge.outgoing(), None);
    }
    assert_eq!(bridge.read_on(), None);
    assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::ReadOn));
    assert_eq!(bridge.outgoing(), None);
}

#[test]
fn the_clock_follows_the_worlds_messages() {
    let mut bridge = bridge(1, 10);
    let clock = Clock { idle: true, due: Some(at(150)), ..clock() };
    assert_eq!(
        bridge.incoming(&encode_to_page(&ToPage { clock, said: Said::Slice }), at(BROWSER_START)),
        Ok(Heard::Nothing)
    );
    assert_eq!(bridge.now(at(BROWSER_START + 1)), (at(110), Wall::from_nanos(WALL_START + 110)));
    assert_eq!(bridge.timer(at(150)), Some(at(BROWSER_START + 5)));
    assert_eq!(bridge.timer(at(151)), None);
    assert_eq!(bridge.now(at(BROWSER_START + 9)), (at(150), Wall::from_nanos(WALL_START + 150)));

    let lagging = Clock {
        step: 8,
        now: at(140),
        wall: Wall::from_nanos(WALL_START + 140),
        idle: false,
        due: None,
        memory: 0,
        slice: Duration::ZERO,
    };
    let message = ToPage { clock: lagging, said: Said::Report(Report::Failed { exchange: 999 }) };
    assert_eq!(bridge.incoming(&encode_to_page(&message), at(BROWSER_START + 10)), Ok(Heard::Nothing));
    assert_eq!(bridge.now(at(BROWSER_START + 11)), (at(150), Wall::from_nanos(WALL_START + 150)));
    assert_eq!(bridge.timer(at(151)), None);

    let clock =
        Clock { now: at(200), wall: Wall::from_nanos(WALL_START + 200), idle: true, due: Some(at(260)), ..lagging };
    send(&mut bridge, &exchange());
    let message = ToPage { clock, said: Said::Stamp(Stamp { step: 8, instant: at(200) }) };
    assert_eq!(
        bridge.incoming(&encode_to_page(&message), at(BROWSER_START + 20)),
        Ok(Heard::Stamp(Stamp { step: 8, instant: at(200) }))
    );
    assert_eq!(bridge.now(at(BROWSER_START + 22)), (at(220), Wall::from_nanos(WALL_START + 220)));
}

#[test]
fn a_hidden_tab_pauses_the_world_and_shown_resumes_its_pace() {
    let mut bridge = bridge(1, 10);
    open_stream(&mut bridge, 2);
    assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamOpened)), Heard::Report(Report::StreamOpened));
    let clock = Clock { idle: true, ..clock() };
    assert_eq!(
        bridge.incoming(&encode_to_page(&ToPage { clock, said: Said::Slice }), at(BROWSER_START)),
        Ok(Heard::Nothing)
    );
    bridge.visibility(true, at(BROWSER_START + 2));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    assert_eq!(bridge.outgoing(), None);
    assert_eq!(bridge.now(at(BROWSER_START + 50)), (at(120), Wall::from_nanos(WALL_START + 120)));
    assert_eq!(bridge.timer(at(121)), None);
    bridge.visibility(true, at(BROWSER_START + 50));
    assert_eq!(bridge.outgoing(), None);
    bridge.visibility(false, at(BROWSER_START + 60));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(10)));
    assert_eq!(bridge.now(at(BROWSER_START + 61)), (at(130), Wall::from_nanos(WALL_START + 130)));
    assert_eq!(bridge.timer(at(140)), Some(at(BROWSER_START + 62)));
    assert_eq!(
        incoming(&mut bridge, Said::Report(Report::StreamPiece { bytes: b"ok".as_slice().into() })),
        Heard::Report(Report::StreamPiece { bytes: b"ok".as_slice().into() })
    );
}

#[test]
fn a_pace_selected_while_hidden_waits_for_showing() {
    let mut bridge = bridge(1, 10);
    bridge.visibility(true, at(BROWSER_START));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    bridge.pace(Speed(3), at(BROWSER_START + 1));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    bridge.visibility(false, at(BROWSER_START + 2));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(3)));
    bridge.pace(Speed(0), at(BROWSER_START + 3));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    bridge.visibility(true, at(BROWSER_START + 4));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    bridge.visibility(false, at(BROWSER_START + 5));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    assert_eq!(bridge.outgoing(), None);
}

#[test]
fn lines_stamps_stop_and_failure_are_heard() {
    let mut bridge = bridge(1, 1);
    send(&mut bridge, &exchange());
    let line =
        || Line { stream: Output::Error, process: b"server".as_slice().into(), line: b"words".as_slice().into() };
    let stamp = Stamp { step: 19, instant: at(31) };
    let rows = [
        (Said::Line(line()), Heard::Line(line())),
        (Said::Stamp(stamp), Heard::Stamp(stamp)),
        (Said::Stopped, Heard::Stopped),
        (
            Said::Failed { words: b"panic words".as_slice().into() },
            Heard::Failed { words: b"panic words".as_slice().into() },
        ),
    ];
    for (said, expected) in rows {
        assert_eq!(incoming(&mut bridge, said), expected);
        assert_eq!(bridge.now(at(BROWSER_START + 1)), (clock().now, clock().wall));
    }
}

#[test]
fn malformed_bytes_are_refused() {
    let mut bridge = bridge(1, 10);
    let ticket = send(&mut bridge, &exchange());
    let valid = encode_to_page(&ToPage { clock: clock(), said: answer(ticket, b"yes") });
    let mut trailing = valid.to_vec();
    trailing.push(0);
    for bytes in [&[][..], &[255][..], &valid[..valid.len() - 1], &trailing] {
        assert_eq!(bridge.incoming(bytes, at(BROWSER_START)), Err(Malformed));
        assert_eq!(bridge.now(at(BROWSER_START + 1)), (Time::ZERO, Wall::from_nanos(WALL_START)));
        assert_eq!(bridge.outgoing(), None);
    }
    assert_eq!(
        bridge.incoming(&valid, at(BROWSER_START)),
        Ok(Heard::Report(Report::Answered { exchange: APPLICATION_ID, status: 201, body: b"yes".as_slice().into() }))
    );
}

#[test]
fn a_stream_refusal_is_terminal_on_the_page() {
    let mut bridge = bridge(1, 1);
    open_stream(&mut bridge, 2);
    assert_eq!(
        incoming(&mut bridge, Said::Report(Report::StreamRefused { status: 401 })),
        Heard::Report(Report::StreamRefused { status: 401 })
    );
    assert_eq!(bridge.read_on(), None);
    assert_eq!(bridge.outgoing(), None);
    assert_eq!(bridge.stream(StreamDuty::Close), None);
    assert_eq!(outgoing(&mut bridge), ToWorld::Duty(Duty::Stream(StreamDuty::Close)));
    open_stream(&mut bridge, 2);
    assert_eq!(incoming(&mut bridge, Said::Report(Report::StreamOpened)), Heard::Report(Report::StreamOpened));
}

#[test]
fn the_store_belongs_to_the_visit() {
    let mut first = bridge(1, 1);
    let mut second = bridge(1, 1);
    assert!(first.store().write(b"key", b"value"));
    assert_eq!(
        first.store().read_all(),
        vec![Record { key: b"key".as_slice().into(), value: b"value".as_slice().into() }]
    );
    assert!(second.store().read_all().is_empty());
    assert_eq!(first.outgoing(), None);
    first.store().remove(b"key");
    assert!(first.store().read_all().is_empty());
}

#[test]
fn a_pace_change_preserves_the_instant_and_moves_the_timer() {
    let mut bridge = bridge(1, 10);
    let clock = Clock { idle: true, due: Some(at(150)), ..clock() };
    assert_eq!(
        bridge.incoming(&encode_to_page(&ToPage { clock, said: Said::Slice }), at(BROWSER_START)),
        Ok(Heard::Nothing)
    );
    let changed_at = at(BROWSER_START + 2);
    let before = bridge.now(changed_at);
    assert_eq!(before, (at(120), Wall::from_nanos(WALL_START + 120)));
    bridge.pace(Speed(2), changed_at);
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(2)));
    assert_eq!(bridge.now(changed_at), before);
    assert_eq!(bridge.timer(at(150)), Some(at(BROWSER_START + 17)));
    assert_eq!(bridge.now(at(BROWSER_START + 3)), (at(122), Wall::from_nanos(WALL_START + 122)));
    bridge.pace(Speed(0), at(BROWSER_START + 3));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    assert_eq!(bridge.now(at(BROWSER_START + 100)), (at(122), Wall::from_nanos(WALL_START + 122)));
    assert_eq!(bridge.timer(at(123)), None);
    assert_eq!(bridge.outgoing(), None);
}

#[test]
fn the_log_records_duties_as_sent_and_the_world_as_it_said() {
    use crate::bridge::log::{End, Fold, read};
    let mut bridge = bridge(1, 1);
    let mut duties = Vec::new();
    bridge.send(&exchange());
    duties.push(outgoing(&mut bridge));
    // Refusal at the page's entrance sends no duty.
    bridge.send(&Exchange { id: APPLICATION_ID + 1, ..exchange() });
    bridge.abandon(APPLICATION_ID);
    duties.push(outgoing(&mut bridge));
    bridge.stream(open(3));
    duties.push(outgoing(&mut bridge));
    bridge.read_on();
    duties.push(outgoing(&mut bridge));
    bridge.stream(StreamDuty::Close);
    duties.push(outgoing(&mut bridge));
    bridge.pace(Speed(0), at(BROWSER_START));
    assert_eq!(outgoing(&mut bridge), ToWorld::Pace(Speed(0)));
    let stamp = Stamp { step: 7, instant: at(100) };
    let line = Said::Line(Line {
        process: b"server".as_slice().into(),
        stream: Output::Standard,
        line: b"hello".as_slice().into(),
    });
    let mut fold = Fold::default();
    fold.said(&line);
    let first_digest = fold.take();
    let empty_digest = fold.take();
    incoming(&mut bridge, line);
    for _ in &duties {
        incoming(&mut bridge, Said::Stamp(stamp));
    }
    let terminal = Said::Stopped;
    fold.said(&terminal);
    let end_digest = fold.take();
    incoming(&mut bridge, terminal);
    let log = read(&bridge.log()).unwrap();
    assert_eq!(log.build.as_ref(), BUILD);
    assert_eq!(log.start, start());
    assert_eq!(log.inputs.len(), duties.len());
    for (at, (input, duty)) in log.inputs.iter().zip(duties).enumerate() {
        assert_eq!(ToWorld::Duty(input.duty.clone()), duty);
        assert_eq!(input.stamp, stamp);
        assert_eq!(input.digest, if at == 0 { first_digest } else { empty_digest });
    }
    assert_eq!(log.end, End::Said(end_digest));
}

#[test]
fn a_dropped_stale_report_is_still_folded() {
    use crate::bridge::log::{Fold, read};
    let mut bridge = bridge(1, 1);
    let ticket = send(&mut bridge, &exchange());
    incoming(&mut bridge, Said::Stamp(Stamp { step: 0, instant: Time::ZERO }));
    bridge.abandon(APPLICATION_ID);
    outgoing(&mut bridge);
    incoming(&mut bridge, Said::Stamp(Stamp { step: 1, instant: Time::ZERO }));
    let stale = answer(ticket, b"old");
    let mut fold = Fold::default();
    fold.said(&stale);
    assert_eq!(incoming(&mut bridge, stale), Heard::Nothing);
    send(&mut bridge, &exchange());
    incoming(&mut bridge, Said::Stamp(Stamp { step: 2, instant: Time::ZERO }));
    let log = read(&bridge.log()).unwrap();
    assert_eq!(log.inputs[2].digest, fold.take());
}
