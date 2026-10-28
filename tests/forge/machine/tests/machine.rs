use skein_forge::api::{Answer as ResultAnswer, Error, Op, Read};
use skein_forge_connection::{StartRead, Stopped};
use skein_forge_machine_world::{
    fixtures,
    machine::{self, Answer, Connect, Machine, Response},
};
use skein_lib::Duration;
const SECOND: Duration = Duration::from_secs(1);
#[test]
fn the_start_reaches_ready_from_the_recorded_answers() {
    let mut world = Machine::ready(7, vec![]);
    world.run_for(SECOND);
    assert!(world.script_done());
    assert_eq!(world.requests().len(), 2);
    assert_eq!(world.stopped(), None);
    assert!(world.close());
}
#[test]
fn another_user_stops_the_start_once_naming_the_forge() {
    let mut world = Machine::new(
        7,
        vec![Answer {
            request: "GET /api/v1/user",
            response: Response::Bytes(fixtures::response(200, br#"{"id":8,"login":"other"}"#)),
        }],
    );
    world.run_for(Duration::from_secs(3));
    assert_eq!(world.stopped(), Some((fixtures::FORGE, Stopped::OtherUser { user: 8 })));
    assert_eq!(world.requests().len(), 1);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn settings_answered_404_stops_the_start_with_its_status() {
    let mut world = Machine::new(
        7,
        vec![
            Answer::case("GET /api/v1/user", "user"),
            Answer { request: "GET /api/v1/settings/api", response: Response::Bytes(fixtures::response(404, b"")) },
        ],
    );
    world.run_for(SECOND);
    assert_eq!(
        world.stopped(),
        Some((fixtures::FORGE, Stopped::Refused { read: StartRead::ApiSettings, status: 404 }))
    );
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn an_unreachable_forge_is_checked_again_after_its_backoff() {
    let mut world = Machine::ready(7, vec![]);
    world.connects(&[Connect::Refused]);
    world.run_for(Duration::from_millis(999));
    assert_eq!(world.connections_opened(), 1);
    world.run_for(SECOND);
    assert_eq!(world.connections_opened(), 3);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn a_call_waiting_past_its_deadline_is_unavailable_at_no_cost() {
    let mut world = Machine::new(7, vec![Answer { request: "GET /api/v1/user", response: Response::Silent }]);
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(Duration::from_secs(10));
    let answered = world.answered(1).expect("waiting call ended");
    assert_eq!((answered.cost, &answered.result), (0, &Err(Error::Unavailable)));
    assert!(world.close());
}
#[test]
fn a_call_silent_past_its_deadline_is_a_timeout_and_aborts() {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer { request: "GET /api/v1/repos/fixture/specimen/branches/main", response: Response::Silent },
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(Duration::from_secs(10));
    let answered = world.answered(1).expect("written call ended");
    assert_eq!((answered.cost, &answered.result), (2, &Err(Error::Timeout)));
    assert!(world.aborts() > 0);
    assert!(world.close());
}
#[test]
fn a_repository_level_404_is_resolved_again_by_id() {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer::case("GET /api/v1/repos/fixture/specimen/issues/999", "missing-issue"),
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
        ],
    );
    world.call(1, fixtures::REPO, Op::Read(Read::Item { number: 999, after: 0 }));
    world.run_for(SECOND);
    world.call(2, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    let first = world.answered(1).expect("missing answered");
    assert_eq!((first.cost, &first.result), (2, &Err(Error::Missing)));
    let second = world.answered(2).expect("branch answered");
    assert_eq!(second.cost, 2);
    assert!(matches!(&second.result, Ok(ResultAnswer::Commit(_))), "{:?}", second.result);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn a_redirect_drops_the_path_and_answers_unavailable() {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer {
                request: "GET /api/v1/repos/fixture/specimen/branches/main",
                response: Response::Bytes(fixtures::response(301, b"")),
            },
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    assert_eq!(world.answered(1).expect("redirect answered").result, Err(Error::Unavailable));
    world.call(2, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    assert_eq!(world.answered(2).expect("branch answered").cost, 2);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn a_body_past_body_bytes_is_carried_empty_without_its_key() {
    let body = format!(
        "{{\"number\":1,\"state\":\"open\",\"user\":{{\"id\":2,\"login\":\"writer\"}},\"labels\":[],\"updated_at\":\"2020-01-01T00:00:00Z\",\"title\":\"title\",\"body\":\"<!-- kit:key ab -->\\n{}\"}}",
        "x".repeat(1046)
    );
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer {
                request: "GET /api/v1/repos/fixture/specimen/issues?state=all&since=1970-01-01T00%3A00%3A00Z&sort=leastupdate&page=1&limit=2",
                response: Response::Bytes(fixtures::dated_response(200, format!("[{body}]").as_bytes())),
            },
        ],
    );
    world.call(1, fixtures::REPO, Op::Read(Read::Items { since: skein_lib::Time::ZERO, page: 1, kind: None }));
    world.run_for(SECOND);
    let result = &world.answered(1).expect("items answered").result;
    let Ok(ResultAnswer::Items { items, more, .. }) = result else {
        panic!("items: {result:?}");
    };
    assert_eq!(items.len(), 1);
    assert!(!more);
    assert_eq!(items[0].key, None);
    assert!(items[0].body.is_empty());
    assert!(world.close());
}
#[test]
fn an_idle_connection_closes_and_the_next_call_connects() {
    let mut branch = machine::case_bytes("branch");
    let at = branch
        .windows(b"Connection: close\r\n".len())
        .position(|bytes| bytes == b"Connection: close\r\n")
        .expect("close header");
    branch.drain(at..at + b"Connection: close\r\n".len());
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer { request: "GET /api/v1/repos/fixture/specimen/branches/main", response: Response::Bytes(branch) },
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    let opened = world.connections_opened();
    world.run_for(Duration::from_secs(5));
    world.call(2, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    assert_eq!(world.connections_opened(), opened + 1);
    assert_eq!(world.answered(2).expect("after idle").cost, 1);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn a_connection_marked_close_finishes_its_call_and_the_next_connects() {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    let opened = world.connections_opened();
    world.call(2, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    assert_eq!(world.connections_opened(), opened + 1);
    assert_eq!(world.answered(2).expect("after close").cost, 1);
    assert!(world.close());
}
#[test]
fn a_well_framed_malformed_document_is_invalid_answer() {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer {
                request: "GET /api/v1/repos/fixture/specimen/branches/main",
                response: Response::Bytes(fixtures::response(200, b"{")),
            },
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    let answered = world.answered(1).expect("malformed answered");
    assert_eq!((answered.cost, &answered.result), (2, &Err(Error::InvalidAnswer)));
    assert!(world.close());
}
#[test]
fn a_reset_after_a_written_request_is_timeout() {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer { request: "GET /api/v1/repos/fixture/specimen/branches/main", response: Response::Reset },
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    let answered = world.answered(1).expect("reset answered");
    assert_eq!((answered.cost, &answered.result), (2, &Err(Error::Timeout)));
    assert!(world.close());
}
#[test]
fn a_write_body_is_read_before_its_scripted_response() {
    let mut world = Machine::ready(7, vec![Answer::case("GET /api/v1/repositories/1", "repository-by-id"), Answer { request: "POST /api/v1/repos/fixture/specimen/issues", response: Response::Bytes(fixtures::response(201, br#"{"number":7,"state":"open","user":{"id":2,"login":"writer"},"labels":[],"updated_at":"2020-01-01T00:00:00Z","title":"title","body":"body"}"#)) }]);
    world.call(
        1,
        fixtures::REPO,
        Op::Write(skein_forge::api::Write::CreateIssue {
            key: Box::from([0xab]),
            title: Box::from(*b"title"),
            body: Box::from(*b"body"),
        }),
    );
    world.run_for(SECOND);
    let answered = world.answered(1).expect("creation answered");
    assert_eq!((answered.cost, &answered.result), (2, &Ok(ResultAnswer::Created(7))));
    assert!(
        world
            .requests()
            .last()
            .expect("creation request")
            .ends_with(br#"{"title":"title","body":"<!-- kit:key ab -->\nbody"}"#)
    );
    assert!(world.close());
}

// The deadline is ten seconds in the admitted fixture; inspect its two sides.
const DEADLINE: skein_lib::Time = skein_lib::Time::from_nanos(10_000_000_000);
const BEFORE_DEADLINE: skein_lib::Time = skein_lib::Time::from_nanos(9_999_999_999);
// The fixture's idle timeout is five seconds; SECOND allows observing startup first.
// This is an observation allowance, not a bound on the duration of startup.
const CALL_ADMISSION: Duration = Duration::from_secs(5).saturating_add(SECOND);
// The fixture's ten-second deadline plus its one-second initial start backoff.
const START_RETRY: skein_lib::Time = DEADLINE.saturating_add(SECOND);
#[derive(Debug)]
struct DeadlineRow {
    name: &'static str,
    request: usize,
    pause: machine::Pause,
}
#[test]
fn start_deadlines_retry_and_end_waiting_calls() {
    for row in [
        DeadlineRow { name: "checking and waiting", request: 0, pause: machine::Pause::Reading(0) },
        DeadlineRow { name: "settings and waiting", request: 1, pause: machine::Pause::Reading(0) },
    ] {
        for secure in [false, true] {
            let mut world = transport_machine(secure, fixtures::locate_script());
            assert_eq!(drive_deadline(&mut world, &row, Duration::ZERO), (0, Error::Unavailable, 1, 1), "{}", row.name);
            let opened = world.connections_opened();
            world.connects(&[Connect::Refused]);
            world.advance_to(START_RETRY);
            assert_eq!(world.connections_opened(), opened + 1, "{} start retries", row.name);
            assert!(world.close());
        }
    }
}
#[test]
fn unseen_call_deadlines_are_unavailable_at_no_cost() {
    for secure in [false, true] {
        let mut rows = vec![
            DeadlineRow { name: "connecting", request: 2, pause: machine::Pause::Connecting },
            DeadlineRow { name: "sending before first byte", request: 2, pause: machine::Pause::Writing(0) },
        ];
        if secure {
            rows.push(DeadlineRow { name: "handshaking", request: 2, pause: machine::Pause::Handshaking });
        }
        for row in rows {
            let mut world = ready_for_deadline(secure);
            assert_eq!(drive_deadline(&mut world, &row, CALL_ADMISSION), (0, Error::Unavailable, 1, 1), "{}", row.name);
            assert!(world.close());
        }
    }
}
#[test]
fn deadlines_end_each_phase() {
    for secure in [false, true] {
        for row in deadline_rows() {
            let mut world = ready_for_deadline(secure);
            assert_eq!(drive_deadline(&mut world, &row, CALL_ADMISSION), (1, Error::Timeout, 1, 1), "{}", row.name);
            assert!(world.close());
        }
    }
}
fn deadline_rows() -> [DeadlineRow; 3] {
    use machine::Pause::{Reading, Writing};
    let response = match fixtures::locate_script().pop().expect("locate response").response {
        Response::Bytes(bytes) => bytes,
        Response::Case(_) | Response::Served | Response::Reset | Response::Cut { .. } | Response::Silent => {
            unreachable!("generated bytes")
        }
    };
    let body = response.windows(4).position(|part| part == b"\r\n\r\n").expect("head") + 4;
    [
        DeadlineRow { name: "sending after first byte", request: 2, pause: Writing(1) },
        DeadlineRow { name: "reading head", request: 2, pause: Reading(1) },
        DeadlineRow { name: "reading body", request: 2, pause: Reading(body + 1) },
    ]
}
fn ready_for_deadline(secure: bool) -> Machine {
    let mut world = transport_machine(secure, fixtures::locate_script());
    // The generated start leaves an idle socket; let it expire before the call.
    world.run_for(CALL_ADMISSION);
    world
}
fn drive_deadline(world: &mut Machine, row: &DeadlineRow, admission: Duration) -> (u32, Error, usize, u32) {
    world.pause_at(row.request, row.pause);
    world.call(1, fixtures::REPO, fixtures::locate());
    world.advance_to(BEFORE_DEADLINE.saturating_add(admission));
    assert_eq!((world.deliveries().len(), world.aborts()), (0, 0), "{} before", row.name);
    world.advance_to(DEADLINE.saturating_add(admission));
    let answer = world.answered(1).expect("one deadline terminal");
    (answer.cost, answer.result.clone().expect_err("deadline"), world.deliveries().len(), world.aborts())
}
#[test]
fn a_start_doubles_its_backoff_to_the_cap() {
    let mut script = fixtures::locate_script();
    script.pop();
    let mut world = Machine::new(7, script);
    world.connects(&[Connect::Refused; 5]);
    let mut due = skein_lib::Time::ZERO;
    world.advance_to(due);
    for (opened, seconds) in [1, 2, 4, 8, 8].into_iter().enumerate() {
        due = due.saturating_add(Duration::from_secs(seconds));
        world.advance_to(skein_lib::Time::from_nanos(due.as_nanos() - 1));
        assert_eq!(world.connections_opened(), u32::try_from(opened + 1).expect("small count"));
        world.advance_to(due);
        assert_eq!(world.connections_opened(), u32::try_from(opened + 2).expect("small count"));
    }
    assert_eq!(world.stopped(), None);
    assert!(world.script_done());
    assert!(world.close());
}
fn answered_branch() -> Machine {
    let mut world = Machine::ready(
        7,
        vec![
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    world
}
#[test]
fn a_held_answer_is_offered_again() {
    let mut world = answered_branch();
    let answer = world.answered(1).expect("answer").clone();
    world.action(skein_forge_connection::Request::Refused(answer));
    let first = world.deliveries().len();
    world.action(skein_forge_connection::Request::Offer);
    let second = world.deliveries().len();
    world.action(skein_forge_connection::Request::Offer);
    assert_eq!([first, second, world.deliveries().len()], [1, 2, 2]);
    assert_eq!(world.deliveries().iter().map(|answer| answer.cost).collect::<Vec<_>>(), [2, 2]);
    assert!(world.close());
}
#[test]
fn a_returned_answer_is_never_offered_again() {
    let mut world = answered_branch();
    let answer = world.answered(1).expect("answer").clone();
    world.action(skein_forge_connection::Request::Returned(answer));
    let first = world.deliveries().len();
    world.action(skein_forge_connection::Request::Offer);
    let second = world.deliveries().len();
    world.action(skein_forge_connection::Request::Offer);
    assert_eq!([first, second, world.deliveries().len()], [1, 1, 1]);
    assert!(world.close());
    assert_eq!(world.closed_events(), 1);
}

const OTHER: u16 = 2;
fn two_idle_forge_one_connections() -> Machine {
    use skein_forge_connection::settings::{Forge, Kind, Transport};
    // Forge 2 has its own admitted loopback endpoint; both starts complete first.
    let mut settings = fixtures::settings();
    settings
        .forges
        .push(Forge {
            number: OTHER,
            kind: Kind::Forgejo,
            log_origins: skein_lib::List::with_capacity(0),
            address: skein_io::Addr::Ip(([127, 0, 0, 1], 3001).into()),
            transport: Transport::Plaintext { name: None },
            access: Some(fixtures::access(fixtures::TOKEN, fixtures::WRITER, None)),
        })
        .expect("second admitted forge");
    let mut script = fixtures::locate_script();
    script.pop();
    let mut second = fixtures::locate_script();
    second.pop();
    script.extend(second);
    // Clear the second start's socket ownership without caching the later repository.
    script.push(Answer { request: fixtures::LOCATE, response: Response::Bytes(fixtures::response(200,
        br#"{"id":2,"owner":{"id":1,"login":"fixture"},"name":"other","object_format_name":"sha1","default_branch":"main"}"#)) });
    for _ in 0..2 {
        script.push(Answer {
            request: "GET /api/v1/repositories/1",
            response: Response::Bytes(keepalive("repository-by-id")),
        });
    }
    for _ in 0..2 {
        script.push(Answer {
            request: "GET /api/v1/repos/fixture/specimen/branches/main",
            response: Response::Bytes(keepalive("branch")),
        });
    }
    script.extend([
        Answer { request: "GET /api/v1/repositories/1", response: Response::Bytes(keepalive("repository-by-id")) },
        Answer {
            request: "GET /api/v1/repos/fixture/specimen/branches/main",
            response: Response::Bytes(keepalive("branch")),
        },
    ]);
    let mut world = Machine::with(7, fixtures::limits(), settings, script);
    world.connects(&[Connect::Made, Connect::Refused]);
    world.run_for(Duration::from_secs(2));
    let other = skein_forge::api::Repository { forge: OTHER, repository: 1 };
    world.call(1, other, fixtures::locate());
    world.advance_to(skein_lib::Time::from_nanos(2_000_000_000));
    world.pause(machine::Pause::Reading(0));
    world.call(2, fixtures::REPO, fixtures::branch());
    world.advance_to(skein_lib::Time::from_nanos(2_000_000_000));
    world.call(3, fixtures::REPO, fixtures::branch());
    world.advance_to(skein_lib::Time::from_nanos(2_000_000_000));
    world.release_response();
    world.advance_to(skein_lib::Time::from_nanos(2_000_000_000));
    world
}
#[test]
fn an_idle_connection_is_reassigned_to_another_forge() {
    let mut world = two_idle_forge_one_connections();
    let addresses = world.addresses();
    let other = skein_forge::api::Repository { forge: OTHER, repository: 1 };
    let before = (world.closes(), world.connections_opened(), world.deliveries().len());
    world.call(4, other, fixtures::branch());
    world.advance_to(skein_lib::Time::from_nanos(2_000_000_000));
    let answer = world.answered(4).expect("replacement answers");
    assert_eq!(
        (world.closes() - before.0, world.connections_opened() - before.1, world.deliveries().len() - before.2),
        (1, 1, 1)
    );
    assert_eq!(addresses, [skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()); 2]);
    assert_eq!(answer.cost, 2);
    assert!(answer.result.is_ok());
    assert!(world.script_done());
    assert!(world.close());
}
fn keepalive(case: &str) -> Vec<u8> {
    let response = machine::case_bytes(case);
    let head_end = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").expect("head") + 4;
    let mut result = Vec::new();
    let mut removed = 0;
    for line in response[..head_end].split_inclusive(|byte| *byte == b'\n') {
        if line.eq_ignore_ascii_case(b"Connection: close\r\n") {
            removed += 1;
        } else {
            result.extend_from_slice(line);
        }
    }
    assert_eq!(removed, 1, "one close header record");
    result.extend_from_slice(&response[head_end..]);
    result
}

#[derive(Clone, Copy, Debug)]
enum ClosePhase {
    Checking,
    Settings,
    Waiting,
    Connecting,
    Handshaking,
    SendingBefore,
    Sending,
    Reading,
    ReadingBody,
    Resolving,
    Idle,
    Closed,
    Stopped,
}
#[derive(Clone, Copy, Debug)]
enum Delivery {
    Before,
    Accepted,
    Refused,
    Returned,
}
#[derive(Debug)]
struct CloseRow {
    phase: ClosePhase,
    delivery: Delivery,
    expected: [usize; 6],
}

fn close_rows() -> [CloseRow; 35] {
    use ClosePhase::{
        Checking, Closed, Connecting, Idle, Reading, ReadingBody, Resolving, Sending, SendingBefore, Settings, Stopped,
        Waiting,
    };
    use Delivery::{Accepted, Before, Refused, Returned};
    [
        CloseRow { phase: Checking, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Checking, delivery: Accepted, expected: [1, 0, 1, 1, 0, 0] },
        CloseRow { phase: Settings, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Settings, delivery: Accepted, expected: [1, 0, 1, 1, 0, 0] },
        CloseRow { phase: Connecting, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Connecting, delivery: Accepted, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Connecting, delivery: Refused, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Connecting, delivery: Returned, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: SendingBefore, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: SendingBefore, delivery: Accepted, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: SendingBefore, delivery: Refused, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: SendingBefore, delivery: Returned, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Sending, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Sending, delivery: Accepted, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Sending, delivery: Refused, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Sending, delivery: Returned, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Reading, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Reading, delivery: Accepted, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Reading, delivery: Refused, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Reading, delivery: Returned, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: ReadingBody, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: ReadingBody, delivery: Accepted, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: ReadingBody, delivery: Refused, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: ReadingBody, delivery: Returned, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Resolving, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Resolving, delivery: Accepted, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Resolving, delivery: Refused, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Resolving, delivery: Returned, expected: [1, 0, 0, 1, 0, 0] },
        CloseRow { phase: Waiting, delivery: Before, expected: [0, 0, 1, 1, 0, 0] },
        CloseRow { phase: Waiting, delivery: Accepted, expected: [1, 0, 1, 1, 0, 0] },
        CloseRow { phase: Waiting, delivery: Refused, expected: [1, 0, 1, 1, 0, 0] },
        CloseRow { phase: Waiting, delivery: Returned, expected: [1, 0, 1, 1, 0, 0] },
        CloseRow { phase: Idle, delivery: Before, expected: [0, 0, 0, 1, 0, 0] },
        CloseRow { phase: Closed, delivery: Before, expected: [0, 0, 1, 1, 0, 0] },
        CloseRow { phase: Stopped, delivery: Before, expected: [0, 0, 1, 1, 0, 0] },
    ]
}

#[derive(Clone, Copy, Debug)]
enum ListenerClosePhase {
    Head,
    Body,
    Answering,
    Closing,
    Hint,
}
#[derive(Debug)]
struct ListenerCloseRow {
    phase: ListenerClosePhase,
    expected: [usize; 6],
}

#[test]
fn owner_close_wins_or_follows_each_terminal() {
    use ListenerClosePhase::{Answering, Body, Closing, Head, Hint};
    for secure in [false, true] {
        for row in close_rows() {
            assert_eq!(drive_close(&row, secure), row.expected, "{row:?}, TLS={secure}");
        }
    }
    let handshaking =
        CloseRow { phase: ClosePhase::Handshaking, delivery: Delivery::Before, expected: [0, 0, 0, 1, 0, 0] };
    assert_eq!(drive_close(&handshaking, true), handshaking.expected, "{handshaking:?}");
    for row in [
        ListenerCloseRow { phase: Head, expected: [0, 0, 0, 1, 0, 0] },
        ListenerCloseRow { phase: Body, expected: [0, 0, 0, 1, 0, 0] },
        ListenerCloseRow { phase: Answering, expected: [1, 0, 0, 1, 0, 0] },
        ListenerCloseRow { phase: Closing, expected: [0, 0, 0, 1, 0, 0] },
        ListenerCloseRow { phase: Hint, expected: [1, 0, 0, 1, 0, 0] },
    ] {
        assert_eq!(drive_listener_close(row.phase), row.expected, "{row:?}");
    }
}

// Refusal-prefix room keeps the waiting fixture from the queue's default Made fallback.
const WAITING_CONNECT_REFUSALS: usize = 32;

fn close_machine(row: &CloseRow, secure: bool) -> Machine {
    use ClosePhase::{
        Checking, Closed, Connecting, Handshaking, Idle, Reading, ReadingBody, Resolving, Sending, SendingBefore,
        Settings, Stopped, Waiting,
    };
    let mut script = fixtures::locate_script();
    if matches!(row.phase, Checking | Stopped) && !matches!(row.delivery, Delivery::Before)
        || matches!(row.phase, Stopped)
    {
        script[0].response = Response::Bytes(fixtures::response(200, br#"{"id":8,"login":"other"}"#));
        script.truncate(1);
    } else if matches!(row.phase, Settings) && !matches!(row.delivery, Delivery::Before) {
        script[1].response = Response::Bytes(fixtures::response(404, b""));
        script.truncate(2);
    }
    if matches!(row.phase, Resolving) {
        script.truncate(2);
        script.extend([
            Answer { request: "GET /api/v1/repositories/1", response: Response::Bytes(keepalive("repository-by-id")) },
            Answer {
                request: "GET /api/v1/repos/fixture/specimen/branches/main",
                response: Response::Bytes(keepalive("branch")),
            },
        ]);
    }
    let mut world = transport_machine(secure, script);
    if matches!(row.phase, Waiting) {
        world.connects(&[Connect::Refused; WAITING_CONNECT_REFUSALS]);
    }
    if matches!(row.delivery, Delivery::Before) {
        let pause = match row.phase {
            Checking | Settings | Reading | Resolving => Some(machine::Pause::Reading(0)),
            Connecting => Some(machine::Pause::Connecting),
            Handshaking => Some(machine::Pause::Handshaking),
            SendingBefore => Some(machine::Pause::Writing(0)),
            Sending => Some(machine::Pause::Writing(1)),
            ReadingBody => Some(deadline_rows()[2].pause),
            Waiting | Idle | Closed | Stopped => None,
        };
        if let Some(pause) = pause {
            let at = match row.phase {
                Checking => 0,
                Settings => 1,
                Waiting | Connecting | Handshaking | SendingBefore | Sending | Reading | ReadingBody | Resolving
                | Idle | Closed | Stopped => 2,
            };
            if matches!(row.phase, Connecting | Handshaking) {
                world.advance_to(skein_lib::Time::ZERO.saturating_add(CALL_ADMISSION));
            }
            world.pause_at(at, pause);
        }
    }
    match row.phase {
        Checking | Settings if !matches!(row.delivery, Delivery::Before) => {}
        Checking | Settings | Waiting | Connecting | Handshaking | SendingBefore | Sending | Reading | ReadingBody => {
            world.call(1, fixtures::REPO, fixtures::locate());
        }
        Resolving => world.call(1, fixtures::REPO, fixtures::branch()),
        Idle | Closed | Stopped => {}
    }
    world.advance_to(if matches!(row.phase, Closed) {
        skein_lib::Time::ZERO.saturating_add(CALL_ADMISSION)
    } else if matches!(row.phase, Waiting) && !matches!(row.delivery, Delivery::Before) {
        DEADLINE
    } else if matches!(row.phase, Connecting | Handshaking) && matches!(row.delivery, Delivery::Before) {
        skein_lib::Time::ZERO.saturating_add(CALL_ADMISSION)
    } else {
        skein_lib::Time::ZERO
    });
    world
}

fn drive_close(row: &CloseRow, secure: bool) -> [usize; 6] {
    use skein_forge_connection::Request;
    let mut world = close_machine(row, secure);
    let baseline = if matches!(row.phase, ClosePhase::Stopped) { world.event_order().len() } else { 0 };
    if matches!(row.delivery, Delivery::Refused | Delivery::Returned) {
        let answer = world.answered(1).expect("delivered answer").clone();
        world.action(if matches!(row.delivery, Delivery::Refused) {
            Request::Refused(answer)
        } else {
            Request::Returned(answer)
        });
    }
    let terminals = world.event_order().len() - baseline;
    world.hold_settlement();
    world.action(Request::Close);
    let before = world.closed_events();
    world.action(Request::Offer);
    let reoffers = world.event_order().iter().filter(|name| **name != "closed").count() - baseline - terminals;
    world.release_settlement();
    world.action(Request::Offer);
    let order = world.event_order();
    let after_closed = order.len() - order.iter().position(|name| *name == "closed").expect("Closed") - 1;
    [
        terminals,
        reoffers,
        usize::try_from(before).expect("small"),
        usize::try_from(world.closed_events()).expect("small"),
        world.addresses().len(),
        after_closed,
    ]
}

fn drive_listener_close(phase: ListenerClosePhase) -> [usize; 6] {
    use ListenerClosePhase::{Answering, Body, Closing, Head, Hint};
    use skein_forge_connection::Request;
    use skein_forge_machine_world::hooks::{self, Listener};
    let mut world = Listener::new(7, fixtures::hook_limits(), fixtures::hook_settings());
    let socket = world.accept();
    let post = hooks::push(b"main", 7);
    world.hold_settlement();
    match phase {
        Head => world.write(socket, &post.head[..post.head.len() - 1]),
        Body => {
            world.write(socket, &post.head);
            world.write(socket, &post.body[..post.body.len() - 1]);
        }
        Answering => {
            world.send(socket, &post);
            assert_eq!((world.counts()[0], world.hints().len()), (1, 1));
            assert!(world.hinted_after_answer());
            world.refuse();
            assert_eq!(world.hints().len(), 0);
        }
        Closing => world.fault(socket, true),
        Hint => world.send(socket, &post),
    }
    let terminals = world.counts()[1];
    world.request(Request::Close);
    let before = world.counts()[4];
    world.request(Request::Offer);
    world.release_settlement();
    world.request(Request::Offer);
    let counts = world.counts();
    let after_closed =
        world.event_order().len() - world.event_order().iter().position(|name| *name == "closed").expect("Closed") - 1;
    [
        usize::try_from(terminals).expect("small"),
        usize::try_from(counts[1] - terminals).expect("small"),
        usize::try_from(before).expect("small"),
        usize::try_from(counts[4]).expect("small"),
        usize::try_from(counts[3]).expect("small"),
        after_closed,
    ]
}

#[test]
fn a_tls_forge_starts_and_answers_a_call() {
    let mut world = Machine::with(
        7,
        fixtures::limits(),
        skein_forge_machine_world::tls::settings(),
        vec![
            Answer::case("GET /api/v1/user", "user"),
            Answer::case("GET /api/v1/settings/api", "settings-api"),
            Answer::case("GET /api/v1/repositories/1", "repository-by-id"),
            Answer::case("GET /api/v1/repos/fixture/specimen/branches/main", "branch"),
        ],
    );
    world.call(1, fixtures::REPO, fixtures::branch());
    world.run_for(SECOND);
    let answer = world.answered(1).expect("one terminal");
    assert_eq!(answer.cost, 2);
    assert!(answer.result.is_ok());
    assert!(world.script_done());
    assert!(world.close());
}

fn transport_machine(secure: bool, script: Vec<Answer>) -> Machine {
    let settings = if secure { skein_forge_machine_world::tls::settings() } else { fixtures::settings() };
    Machine::with(7, fixtures::limits(), settings, script)
}

#[test]
fn a_connection_takes_no_further_answer_until_its_heard_one_is_taken() {
    use machine::Pause;
    use skein_forge_connection::Request;
    const SEED: u64 = 7;
    const FIRST_REQUEST: &str = "GET /api/v1/user";
    const NEXT_REQUEST: &str = "GET /api/v1/settings/api";
    let first = fixtures::response(200, br#"{"id":2,"login":"writer"}"#);
    let head = first.windows(4).position(|bytes| bytes == b"\r\n\r\n").expect("response head") + 4;
    let second = fixtures::response(200, br#"{"max_response_items":2,"default_paging_num":30}"#);
    let mut world = Machine::new(
        SEED,
        vec![
            Answer { request: FIRST_REQUEST, response: Response::Bytes(first) },
            Answer { request: NEXT_REQUEST, response: Response::Bytes(second) },
        ],
    );
    world.pause_at(0, Pause::Reading(head));
    world.run_for(SECOND);
    world.action(Request::Offer);
    world.action(Request::Offer);
    assert_eq!(world.requests().len(), 1, "the next exchange cannot demand its body past the held first reply");
    assert!(world.connected_forges().is_empty());
    world.release_response();
    world.run_for(SECOND);
    assert_eq!(world.requests().len(), 2, "each buffered reply advances the start plan once");
    assert_eq!(world.connected_forges(), &[fixtures::FORGE]);
    assert!(world.script_done());
    assert!(world.close());
}
