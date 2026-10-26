use skein_lib::Time;
use skein_playground_world::{self as world, Step, Story, When};
use skein_ui_web::{
    bridge::Duty,
    fetch::{Exchange, Report},
    transport::StreamDuty,
};
#[global_allocator]
static HEAP: skein_heap::Counting = skein_heap::Counting;
const SEED: u64 = 17;
fn exchange(id: u32, path: &str, bound: u32) -> Duty {
    Duty::Exchange(Exchange {
        id,
        path: path.as_bytes().into(),
        method: "GET".into(),
        headers: vec![],
        body: Box::from([]),
        bound,
    })
}
fn open(path: &str, bound: u32) -> Duty {
    Duty::Stream(StreamDuty::Open { path: path.as_bytes().into(), headers: vec![], bound })
}
fn after(reports: usize, duty: Duty) -> Step {
    Step { when: When::After(reports), duty }
}
fn at(ms: u64, duty: Duty) -> Step {
    Step { when: When::At(Time::from_nanos(ms * 1_000_000)), duty }
}
fn answer(reports: &[Report], id: u32) -> (u16, &[u8]) {
    reports
        .iter()
        .find_map(|report| match report {
            Report::Answered { exchange, status, body } if *exchange == id => Some((*status, body.as_ref())),
            Report::Answered { .. }
            | Report::Failed { .. }
            | Report::StreamOpened
            | Report::StreamRefused { .. }
            | Report::StreamPiece { .. }
            | Report::StreamEnded => None,
        })
        .expect("answered exchange")
}
#[test]
fn an_exchange_is_answered_with_the_host_and_no_origin() {
    let (outcome, reports) = world::run(SEED, Story::new(vec![at(0, exchange(1, "/echo", 256))], 1));
    assert_eq!(
        answer(&reports, 1),
        (200, b"GET\n/echo\nhost: application.example\norigin: <absent>\ncookie: <absent>\n".as_slice())
    );
    assert!(outcome.heap.as_ref().expect("checked heap").iter().all(|(peak, bound)| peak <= bound));
}
#[test]
fn a_post_sends_its_origin_and_its_body() {
    let mut duty = exchange(1, "/echo?query", 256);
    if let Duty::Exchange(exchange) = &mut duty {
        exchange.method = "POST".into();
        exchange.body = vec![b'p'; 128].into();
        exchange.headers.push(("x-client".into(), "visitor".into()));
    }
    let (_outcome, reports) = world::run(SEED, Story::new(vec![at(0, duty)], 1));
    let (status, body) = answer(&reports, 1);
    assert_eq!(status, 200);
    let mut expected =
        b"POST\n/echo?query\nhost: application.example\norigin: http://application.example\ncookie: <absent>\n"
            .to_vec();
    expected.extend_from_slice(&[b'p'; 128]);
    assert_eq!(body, expected);
}
#[test]
fn an_answer_at_its_bound_is_answered() {
    let (_outcome, reports) = world::run(SEED, Story::new(vec![at(0, exchange(1, "/bytes/16", 16))], 1));
    assert_eq!(answer(&reports, 1), (200, [b'x'; 16].as_slice()));
}
#[test]
fn an_answer_past_its_bound_fails() {
    let (_outcome, reports) = world::run(SEED, Story::new(vec![at(0, exchange(1, "/bytes/17", 16))], 1));
    assert_eq!(reports, vec![Report::Failed { exchange: 1 }]);
}
#[test]
fn exchanges_at_the_bound_are_all_answered() {
    let (outcome, reports) =
        world::run(SEED, Story::new(vec![at(0, exchange(1, "/slow", 1)), at(0, exchange(2, "/slow", 1))], 2));
    assert_eq!(answer(&reports, 1), (200, b"".as_slice()));
    assert_eq!(answer(&reports, 2), (200, b"".as_slice()));
    assert!(
        world::visitor(&outcome.procs).stamps.iter().all(|stamp| *stamp >= Time::from_nanos(world::SLOW.as_nanos()))
    );
}
#[test]
fn a_refused_connection_fails_the_exchange_and_ends_the_stream() {
    let mut story = Story::new(vec![at(0, exchange(1, "/echo", 256)), at(0, open("/stream/1x4", 4))], 2);
    story.listener.set_port(9999);
    let (_outcome, reports) = world::run(SEED, story);
    assert!(reports.contains(&Report::Failed { exchange: 1 }));
    assert!(reports.contains(&Report::StreamEnded));
}
#[test]
fn an_abandoned_exchange_reports_nothing() {
    let mut story = Story::new(
        vec![at(0, exchange(1, "/slow", 1)), at(50, Duty::Abandon { exchange: 1 }), at(50, exchange(1, "/echo", 256))],
        1,
    );
    story.until = Time::from_nanos(200_000_000);
    let (_outcome, reports) = world::run(SEED, story);
    assert_eq!(reports.len(), 1);
    assert_eq!(answer(&reports, 1).0, 200);
    assert!(answer(&reports, 1).1.starts_with(b"GET\n/echo\n"));
}
#[test]
fn an_answer_crossing_its_abandon_is_never_reported() {
    let mut story = Story::new(
        vec![
            at(0, exchange(1, "/bytes/1", 1)),
            Step { when: When::ServerAnswered(1), duty: Duty::Abandon { exchange: 1 } },
        ],
        0,
    );
    story.until = Time::from_nanos(20_000_000);
    let (outcome, reports) = world::run(SEED, story);
    assert!(reports.is_empty());
    assert_eq!(world::visitor(&outcome.procs).inputs_at, vec![Time::ZERO, Time::ZERO]);
    assert!(outcome.submissions.iter().all(|count| *count > 0));
}
#[test]
fn the_stream_reads_one_piece_per_read_on() {
    let steps = vec![
        at(0, open("/stream/1x10", 4)),
        Step { when: When::AfterAt { reports: 1, at: Time::from_nanos(10_000_000) }, duty: Duty::ReadOn },
        Step { when: When::AfterAt { reports: 2, at: Time::from_nanos(20_000_000) }, duty: Duty::ReadOn },
        Step { when: When::AfterAt { reports: 3, at: Time::from_nanos(30_000_000) }, duty: Duty::ReadOn },
        after(4, Duty::ReadOn),
    ];
    let (outcome, reports) = world::run(SEED, Story::new(steps, 5));
    assert_eq!(
        reports,
        vec![
            Report::StreamOpened,
            Report::StreamPiece { bytes: b"ssss".as_slice().into() },
            Report::StreamPiece { bytes: b"ssss".as_slice().into() },
            Report::StreamPiece { bytes: b"ss".as_slice().into() },
            Report::StreamEnded
        ]
    );
    assert_eq!(
        &world::visitor(&outcome.procs).stamps[1..4],
        &[Time::from_nanos(10_000_000), Time::from_nanos(20_000_000), Time::from_nanos(30_000_000)]
    );
}
#[test]
fn a_stream_ends_with_its_body() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(vec![at(0, open("/stream/1x4", 4)), after(1, Duty::ReadOn), after(2, Duty::ReadOn)], 3),
    );
    assert_eq!(
        reports,
        vec![Report::StreamOpened, Report::StreamPiece { bytes: b"ssss".as_slice().into() }, Report::StreamEnded]
    );
}
#[test]
fn a_refused_stream_names_its_status() {
    let (_outcome, reports) = world::run(SEED, Story::new(vec![at(0, open("/status/403", 4))], 1));
    assert_eq!(reports, vec![Report::StreamRefused { status: 403 }]);
}
#[test]
fn every_open_has_one_terminal() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(
            vec![
                at(0, open("/stream/1x4", 4)),
                after(1, open("/stream/1x4", 4)),
                after(3, Duty::Stream(StreamDuty::Close)),
            ],
            4,
        ),
    );
    assert_eq!(reports, vec![Report::StreamOpened, Report::StreamEnded, Report::StreamOpened, Report::StreamEnded]);
}
fn set(id: u32, cookie: &str) -> Duty {
    exchange(id, &format!("/set?{}", cookie.replace(' ', "%20").replace(';', "%3B")), 0)
}
#[test]
fn a_cookie_is_sent_under_its_path_alone() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(
            vec![
                at(0, set(1, "n=v; Path=/echo; HttpOnly")),
                after(1, exchange(2, "/echo?x", 256)),
                after(2, exchange(3, "/echo/child", 256)),
                after(3, exchange(4, "/echoes", 256)),
                after(4, exchange(5, "/other", 256)),
            ],
            5,
        ),
    );
    for id in [2, 3] {
        assert!(answer(&reports, id).1.ends_with(b"cookie: n=v\n"));
    }
    for id in [4, 5] {
        assert!(answer(&reports, id).1.ends_with(b"cookie: <absent>\n"));
    }
}
#[test]
fn a_cleared_cookie_is_not_sent() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(
            vec![at(0, set(1, "n=v")), after(1, set(2, "n=; Max-Age=0")), after(2, exchange(3, "/echo", 256))],
            3,
        ),
    );
    assert!(answer(&reports, 3).1.ends_with(b"cookie: <absent>\n"));
}
#[test]
fn an_expired_cookie_is_not_sent() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(
            vec![
                at(0, set(1, "n=v; Max-Age=1")),
                after(1, exchange(2, "/echo", 256)),
                at(1100, exchange(3, "/echo", 256)),
            ],
            3,
        ),
    );
    assert!(answer(&reports, 2).1.ends_with(b"cookie: n=v\n"));
    assert!(answer(&reports, 3).1.ends_with(b"cookie: <absent>\n"));
}
#[test]
fn the_jar_at_its_bound_refuses_one_more() {
    let mut story = Story::new(
        vec![
            at(0, set(1, "a=123")),
            after(1, set(2, "b=456")),
            after(2, set(3, "c=789")),
            after(3, set(4, "a=1234")),
            after(4, exchange(5, "/echo", 256)),
        ],
        5,
    );
    story.limits.cookie = 4;
    let (_outcome, reports) = world::run(SEED, story);
    assert!(answer(&reports, 5).1.ends_with(b"cookie: a=123; b=456\n"));
}
#[test]
fn a_cookie_from_an_answer_past_its_bound_is_kept() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(vec![at(0, exchange(1, "/set-fail?n=v", 1)), after(1, exchange(2, "/echo", 256))], 2),
    );
    assert_eq!(reports[0], Report::Failed { exchange: 1 });
    assert!(answer(&reports, 2).1.ends_with(b"cookie: n=v\n"));
}
#[test]
fn a_head_refused_by_the_http_client_has_one_terminal() {
    let mut story = Story::new(vec![at(0, exchange(1, "/echo", 256)), at(0, open("/stream/1x4", 4))], 2);
    story.limits.http.request = 16;
    let (_outcome, reports) = world::run(SEED, story);
    assert!(reports.contains(&Report::Failed { exchange: 1 }));
    assert!(reports.contains(&Report::StreamEnded));
}
#[test]
fn a_visitor_at_its_bounds_holds_its_worst_case() {
    let mut full = exchange(5, "/bytes/256", 256);
    // Method and path hold 14 bytes; the body fills the 512-byte duty envelope.
    if let Duty::Exchange(exchange) = &mut full {
        exchange.method = "POST".into();
        exchange.body = vec![b'p'; 498].into();
    }
    let steps = vec![
        at(0, set(1, "a=123456789012345")),
        after(1, set(2, "b=123456789012345")),
        after(2, exchange(3, "/slow", 1)),
        after(2, exchange(4, "/slow", 1)),
        after(2, open("/stream/1x8", 8)),
        after(2, Duty::Abandon { exchange: 4 }),
        after(4, full),
        after(5, Duty::ReadOn),
        after(6, Duty::ReadOn),
    ];
    // set answers (2), slow (1), opened (1), full answer (1), piece (1), end (1).
    let (outcome, reports) = world::run(SEED, Story::new(steps, 7));
    assert_eq!(answer(&reports, 5).1.len(), 256);
    assert!(outcome.heap.as_ref().expect("heap checked").iter().all(|(peak, bound)| peak <= bound));
}
#[test]
fn a_seed_replays() {
    let story = || {
        Story::new(
            vec![
                at(0, set(1, "n=v")),
                after(1, exchange(2, "/echo", 256)),
                after(2, open("/stream/1x8", 4)),
                after(3, Duty::ReadOn),
                after(4, Duty::ReadOn),
                after(5, Duty::ReadOn),
            ],
            6,
        )
    };
    let (first, reports) = world::run(SEED, story());
    let (replay, replayed) = world::run(SEED, story());
    assert_eq!(first.trace, replay.trace);
    assert_eq!(first.heap, replay.heap);
    assert_eq!(first.iterations, replay.iterations);
    assert_eq!(reports, replayed);
}
#[test]
fn a_reset_before_the_head_fails_the_exchange_and_ends_the_stream() {
    let (_outcome, reports) =
        world::run(SEED, Story::new(vec![at(0, exchange(1, "/reset", 1)), at(0, open("/reset", 4))], 2));
    assert!(reports.contains(&Report::Failed { exchange: 1 }));
    assert!(reports.contains(&Report::StreamEnded));
}
#[test]
fn a_cookie_from_a_truncated_answer_is_kept() {
    let (_outcome, reports) = world::run(
        SEED,
        Story::new(vec![at(0, exchange(1, "/set-truncated?n=v", 4)), after(1, exchange(2, "/echo", 256))], 2),
    );
    assert_eq!(reports[0], Report::Failed { exchange: 1 });
    assert!(answer(&reports, 2).1.ends_with(b"cookie: n=v\n"));
}
#[test]
fn the_stream_sends_the_jar_cookies_and_no_origin() {
    let mut story = Story::new(
        vec![at(0, set(1, "n=v")), after(1, open("/echo", 256)), after(2, Duty::ReadOn), after(3, Duty::ReadOn)],
        4,
    );
    story.limits.piece = 256;
    story.limits.http.read = 256;
    let (_outcome, reports) = world::run(SEED, story);
    assert_eq!(reports[1], Report::StreamOpened);
    assert_eq!(
        reports[2],
        Report::StreamPiece {
            bytes: b"GET\n/echo\nhost: application.example\norigin: <absent>\ncookie: n=v\n".as_slice().into()
        }
    );
    assert_eq!(reports[3], Report::StreamEnded);
}
