//! Connection machinery and each forge's start.
use super::fixtures;
use crate::{
    connection::Phase,
    grow::{Full, Grow, Tail},
};
use alloc::boxed::Box;
use skein_forge::forgejo::plans::{Failure, Heard, plan::Range};
use skein_forge::forgejo::request;
use skein_lib::{
    Duration, Time, Wall,
    stream::{Fault, Up},
};

#[test]
fn the_capped_buffer_doubles_and_refuses_a_write_past_its_cap() {
    let mut grow = Grow::new(4, 10);
    let rows: [(&str, &[u8], u32); 3] = [("first piece", b"abc", 4), ("doubled", b"def", 8), ("capped", b"ghij", 10)];
    for (name, piece, expected) in rows {
        assert_eq!(grow.write(piece), Ok(()), "{name}: piece fits");
        assert_eq!(grow.capacity(), expected, "{name}: capacity");
    }
    assert_eq!(grow.write(b"k"), Err(Full), "one past refused whole");
    assert_eq!(grow.finish().as_ref(), b"abcdefghij", "nothing changed by refusal");
}

#[test]
fn a_reset_before_the_first_byte_is_unseen_and_after_it_seen() {
    for (name, send, expected) in [("before room", false, Failure::Unseen), ("after send", true, Failure::Seen)] {
        let mut h = fixtures::ConnectionHarness::new();
        h.connection
            .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(request::Request::User, None))
            .expect("valid request");
        h.pump();
        if send {
            h.connection.stream(&h.env, Up::Room);
            h.pump();
        }
        h.connection.stream(&h.env, Up::Failed(Fault::Reset));
        h.pump();
        assert_eq!(h.connection.heard(), Some(crate::connection::Heard::Forgejo(Heard::Failed(expected))), "{name}");
        assert_eq!(h.connection.heard(), None, "{name}: one failure");
    }
}

#[test]
fn a_json_response_is_decoded_through_its_body_end() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(request::Request::User, None))
        .expect("valid request");
    let heard = h.answer(b"HTTP/1.1 200 OK\r\nContent-Length: 25\r\nDate: Wed, 01 Jan 2020 00:00:00 GMT\r\n\r\n{\"id\":2,\"login\":\"writer\"}");
    assert_eq!(heard.len(), 2, "head then document");
    assert_eq!(
        heard.get(0),
        Some(&Heard::Head {
            status: 200,
            fields: skein_forge::forgejo::plans::Fields {
                date: Some(Wall::from_nanos(1_577_836_800_000_000_000)),
                retry_after: None,
                has_more: None,
                total_count: None,
                content_range: None,
                close: false
            }
        }),
        "response fields"
    );
    assert_eq!(
        heard.get(1),
        Some(&Heard::Done(skein_forge::forgejo::plans::Body::Document(
            skein_forge::forgejo::document::Document::User(skein_forge::forgejo::document::User {
                id: 2,
                login: Box::from(&b"writer"[..]),
                is_admin: false,
                full_name: Box::from([])
            })
        ))),
        "decoded whole"
    );
    assert_eq!(h.connection.phase, Phase::Idle, "reusable");
}

#[test]
fn a_malformed_document_closes_without_reading_more() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(request::Request::User, None))
        .expect("valid request");
    let heard = h.answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
    let Some(Heard::Done(skein_forge::forgejo::plans::Body::Malformed(_))) = heard.last() else {
        unreachable!("malformed terminal");
    };
    assert_eq!(h.connection.phase, Phase::Closed, "malformed closes");
}

#[test]
fn the_start_reads_user_then_settings_on_one_connection() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.connected(fixtures::FORGE);
    h.answer(fixtures::FORGE, &fixtures::user_ok());
    h.answer(fixtures::FORGE, &fixtures::settings_ok());
    h.run(Duration::from_millis(10));
    assert_eq!(
        h.request_lines(fixtures::FORGE).as_slice(),
        [Box::from(&b"GET /api/v1/user HTTP/1.1"[..]), Box::from(&b"GET /api/v1/settings/api HTTP/1.1"[..])],
        "start request order"
    );
    assert_eq!(h.start(fixtures::FORGE), crate::start::Start::Ready { cap: 2 }, "page cap");
    assert_eq!(h.connections_opened(), 1, "one reused connection");
    assert_eq!(h.stopped(), None, "start accepted");
}

#[test]
fn a_refused_token_emits_stopped_once_naming_the_forge() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.connected(fixtures::FORGE);
    h.answer(fixtures::FORGE, &fixtures::status(401));
    h.run(Duration::ZERO);
    assert_eq!(
        h.stopped(),
        Some((1, crate::Stopped::Refused { read: crate::StartRead::User, status: 401 })),
        "refused forge"
    );
    h.run(Duration::from_secs(20));
    assert_eq!(h.connections_opened(), 1, "stopped never restarted");
}

#[test]
fn a_refused_connect_waits_its_backoff_and_connects_again() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.refuse(fixtures::FORGE);
    assert_eq!(
        h.start(fixtures::FORGE),
        crate::start::Start::Waiting { until: Time::from_nanos(1_000_000_000), next: Duration::from_secs(2) },
        "refusal backs off"
    );
    h.run(Duration::from_millis(999));
    assert_eq!(h.connections_opened(), 1, "no early retry");
    h.run(Duration::from_millis(1));
    h.connected(fixtures::FORGE);
    assert_eq!(h.connections_opened(), 2, "new connection after backoff");
    assert_eq!(
        h.request_lines(fixtures::FORGE).as_slice(),
        [Box::from(&b"GET /api/v1/user HTTP/1.1"[..])],
        "check starts again"
    );
}

#[test]
fn an_idle_connection_closes_at_idle() {
    let mut h = fixtures::Harness::ready();
    assert_eq!(h.component.next_deadline(), Some(Time::from_nanos(5_000_000_000)), "fixture idle");
    h.run(Duration::from_secs(4));
    assert_eq!(h.connections_opened(), 1, "idle before deadline");
    h.run(Duration::from_secs(1));
    assert_eq!(h.component.next_deadline(), None, "idle connection ended");
    h.send(crate::Request::Close);
    h.run(Duration::ZERO);
    assert!(h.closed, "component no longer holds the socket");
}

#[test]
fn a_start_read_unanswered_at_its_deadline_waits_and_asks_again() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.connected(fixtures::FORGE);
    assert_eq!(
        h.request_lines(fixtures::FORGE).as_slice(),
        [Box::from(&b"GET /api/v1/user HTTP/1.1"[..])],
        "user sent"
    );
    h.run(Duration::from_secs(10));
    assert_eq!(
        h.start(fixtures::FORGE),
        crate::start::Start::Waiting { until: Time::from_nanos(11_000_000_000), next: Duration::from_secs(2) },
        "read deadline waits"
    );
    h.run(Duration::from_millis(999));
    assert_eq!(h.connections_opened(), 1, "not before backoff");
    h.run(Duration::from_millis(1));
    h.connected(fixtures::FORGE);
    assert_eq!(h.connections_opened(), 2, "timed out connection replaced");
    assert_eq!(
        h.request_lines(fixtures::FORGE).as_slice(),
        [Box::from(&b"GET /api/v1/user HTTP/1.1"[..])],
        "new user read"
    );
}

#[test]
fn a_connection_close_answer_ends_its_connection_after_it() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(request::Request::User, None))
        .expect("valid request");
    let heard =
        h.answer(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 25\r\n\r\n{\"id\":2,\"login\":\"writer\"}");
    let Some(Heard::Done(skein_forge::forgejo::plans::Body::Document(_))) = heard.last() else {
        unreachable!("document ends first");
    };
    assert_eq!(h.connection.phase, Phase::Closed, "answer closed connection");
}

#[test]
fn each_start_read_keeps_its_deadline_through_connect_and_starts_afresh() {
    for (name, before_connect, answer_user, expected) in [
        ("connect uses holder", Duration::from_secs(9), false, Time::from_nanos(10_000_000_000)),
        ("settings gets fresh deadline", Duration::from_secs(5), true, Time::from_nanos(15_000_000_000)),
    ] {
        let mut h = fixtures::Harness::new(fixtures::settings());
        h.run(before_connect);
        h.connected(fixtures::FORGE);
        if answer_user {
            h.answer(fixtures::FORGE, &fixtures::user_ok());
        }
        assert_eq!(h.component.next_deadline(), Some(expected), "{name}");
    }
}

#[test]
fn raw_reads_keep_the_range_and_refuse_bytes_past_remaining_answer_room() {
    let rows: [RawRow; 5] = [
        ("short", b"ab", 3, 10, b"ab", false, Phase::Idle, b"Range: bytes=0-3\r\n"),
        ("exact max", b"abc", 3, 10, b"abc", false, Phase::Idle, b"Range: bytes=0-3\r\n"),
        ("one more", b"abcdef", 3, 10, b"abcd", true, Phase::Closed, b"Range: bytes=0-3\r\n"),
        ("remaining answer", b"abcdef", 5, 3, b"abc", true, Phase::Closed, b"Range: bytes=0-5\r\n"),
        ("empty", b"", 3, 10, b"", false, Phase::Idle, b"Range: bytes=0-3\r\n"),
    ];
    for (name, response, max, room, expected, more, phase, range) in rows {
        let mut h = fixtures::ConnectionHarness::new();
        h.connection.raw_room(room);
        h.connection
            .start(
                &h.env,
                &h.forge,
                fixtures::USER_AGENT,
                fixtures::forgejo_exchange(raw_request(), Some(Range::Head(max))),
            )
            .expect("valid raw request");
        let heard = h.answer(&fixtures::ok(response));
        assert_eq!(
            heard.last(),
            Some(&Heard::Done(skein_forge::forgejo::plans::Body::Raw { bytes: Box::from(expected), more })),
            "{name}: raw body"
        );
        assert_eq!(h.connection.phase, phase, "{name}: connection state");
        assert!(contains(&h.sent, range), "{name}: range header");
    }
}
type TailRow = (&'static str, &'static [u8], &'static [u8], bool, Phase);
type RawRow = (&'static str, &'static [u8], u32, u32, &'static [u8], bool, Phase, &'static [u8]);
fn contains(bytes: &[u8], expected: &[u8]) -> bool {
    for piece in bytes.windows(expected.len()) {
        if piece == expected {
            return true;
        }
    }
    false
}
fn raw_request() -> request::Request {
    request::Request::Raw {
        at: request::Repo { owner: Box::from(&b"fixture"[..]), name: Box::from(&b"specimen"[..]) },
        path: Box::from(&b"file"[..]),
        reference: skein_forge::forgejo::id::ObjectId {
            format: skein_forge::forgejo::id::ObjectFormat::Sha1,
            bytes: [0; 32],
        },
    }
}

#[test]
fn a_json_upload_sends_its_body_and_content_headers() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection
        .start(
            &h.env,
            &h.forge,
            fixtures::USER_AGENT,
            fixtures::forgejo_exchange(
                request::Request::RemoveReviewers {
                    at: request::Repo { owner: Box::from(&b"fixture"[..]), name: Box::from(&b"specimen"[..]) },
                    number: 7,
                    reviewers: Box::from([Box::from(&b"alice"[..])]),
                },
                None,
            ),
        )
        .expect("valid upload");
    let heard = h.answer(b"HTTP/1.1 204 No Content\r\n\r\n");
    assert_eq!(heard.last(), Some(&Heard::Done(skein_forge::forgejo::plans::Body::Empty)), "bodyless answer");
    assert_eq!(h.sent.as_ref(), b"DELETE /api/v1/repos/fixture/specimen/pulls/7/requested_reviewers HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nAuthorization: token 0123456789abcdef0123456789abcdef01234567\r\nUser-Agent: kit/0.0.0\r\nContent-Type: application/json\r\nContent-Length: 23\r\n\r\n{\"reviewers\":[\"alice\"]}", "encoded upload");
}

#[test]
fn a_reset_while_the_start_reads_waits_before_a_new_connection() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.connected(fixtures::FORGE);
    h.reset(fixtures::FORGE);
    assert_eq!(
        h.start(fixtures::FORGE),
        crate::start::Start::Waiting { until: Time::from_nanos(1_000_000_000), next: Duration::from_secs(2) },
        "reset backs off"
    );
    h.run(Duration::from_secs(1));
    assert_eq!(h.connections_opened(), 2, "new connect at backoff");
}

#[test]
fn a_chunked_raw_read_keeps_the_last_short_piece() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(raw_request(), Some(Range::Head(3))))
        .expect("valid raw request");
    let heard = h.answer(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n0\r\n\r\n");
    assert_eq!(
        heard.last(),
        Some(&Heard::Done(skein_forge::forgejo::plans::Body::Raw { bytes: Box::from(&b"ab"[..]), more: false })),
        "final bytes kept"
    );
    assert_eq!(h.connection.phase, Phase::Idle, "complete body reusable");
}

#[test]
fn the_tail_keeps_the_last_max_bytes_across_writes() {
    for (name, pieces, expected, dropped, capacities) in [
        ("short", [b"ab".as_slice(), b"", b""], b"ab".as_slice(), false, [2, 2, 2]),
        ("wrapped", [b"ab".as_slice(), b"cde", b"f"], b"cdef".as_slice(), true, [2, 4, 4]),
    ] {
        let mut tail = Tail::new(2, 4);
        for (piece, capacity) in pieces.into_iter().zip(capacities) {
            tail.write(piece);
            assert_eq!(tail.capacity(), capacity, "{name}: Capacity");
        }
        assert_eq!(tail.dropped(), dropped, "{name}: Earlier bytes");
        assert_eq!(tail.finish().as_ref(), expected, "{name}: Last bytes");
    }
}
#[test]
fn a_tail_read_keeps_a_200s_last_bytes_and_closes_when_some_were_dropped() {
    let rows: [TailRow; 3] = [
        ("short", b"ab", b"ab", false, Phase::Idle),
        ("max", b"abc", b"abc", false, Phase::Idle),
        ("longer", b"abcdef", b"def", true, Phase::Closed),
    ];
    for (name, response, expected, more, phase) in rows {
        let mut h = fixtures::ConnectionHarness::new();
        h.connection
            .start(
                &h.env,
                &h.forge,
                fixtures::USER_AGENT,
                fixtures::forgejo_exchange(raw_request(), Some(Range::Tail(3))),
            )
            .expect("Valid raw request");
        let heard = h.answer(&fixtures::ok(response));
        let kept = Heard::Done(skein_forge::forgejo::plans::Body::Raw { bytes: Box::from(expected), more });
        assert_eq!(heard.last(), Some(&kept), "{name}: The last bytes");
        assert_eq!(h.connection.phase, phase, "{name}: Connection state");
        assert!(contains(&h.sent, b"Range: bytes=-3\r\n"), "{name}: Suffix range");
    }
}
#[test]
fn a_tail_read_keeps_at_most_the_raw_room() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection.raw_room(8);
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(raw_request(), Some(Range::Tail(16))))
        .expect("Valid raw request");
    let heard = h.answer(&fixtures::ok(b"abcdefghijklmnopqrst"));
    assert_eq!(
        heard.last(),
        Some(&Heard::Done(skein_forge::forgejo::plans::Body::Raw { bytes: Box::from(&b"mnopqrst"[..]), more: true }))
    );
    assert_eq!(h.connection.phase, Phase::Closed, "Earlier bytes were dropped");
}
#[test]
fn a_chunked_tail_read_keeps_the_last_bytes_across_pieces() {
    let mut h = fixtures::ConnectionHarness::new();
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(raw_request(), Some(Range::Tail(3))))
        .expect("Valid raw request");
    let heard = h.answer(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n3\r\ncde\r\n0\r\n\r\n");
    assert_eq!(
        heard.last(),
        Some(&Heard::Done(skein_forge::forgejo::plans::Body::Raw { bytes: Box::from(&b"cde"[..]), more: true }))
    );
    assert_eq!(h.connection.phase, Phase::Closed, "Earlier bytes were dropped");
}
#[test]
fn a_tail_read_past_body_bytes_in_is_malformed_and_closes() {
    let mut h = fixtures::ConnectionHarness::new();
    h.env.limits.body_bytes_in = 8;
    h.connection
        .start(&h.env, &h.forge, fixtures::USER_AGENT, fixtures::forgejo_exchange(raw_request(), Some(Range::Tail(3))))
        .expect("Valid raw request");
    let heard = h.answer(&fixtures::ok(b"abcdefghi"));
    assert_eq!(
        heard.last(),
        Some(&Heard::Done(skein_forge::forgejo::plans::Body::Malformed(
            skein_forge::forgejo::decode::Malformed::Bound { field: b"log" }
        )))
    );
    assert_eq!(h.connection.phase, Phase::Closed, "Body limit closes");
}

#[test]
fn start_retry_after_wins_over_the_backoff_cap() {
    use crate::{Answered, start::Start};
    use skein_forge::api;
    let limits = fixtures::limits();
    let waiting = Start::Waiting { until: Time::from_nanos(21_000_000_000), next: Duration::from_secs(2) };
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.run(Duration::from_secs(1));
    h.connected(fixtures::FORGE);
    h.answer(fixtures::FORGE, b"HTTP/1.1 429 Refused\r\nContent-Length: 0\r\nRetry-After: 20\r\n\r\n");
    assert_eq!(h.start(fixtures::FORGE), waiting);
    h.call(1, fixtures::REPO, api::Op::Read(api::Read::Settings));
    h.run(Duration::from_nanos(9_999_999_999));
    assert_eq!((h.answered(1), h.connections_opened()), (None, 1));
    h.run(Duration::from_nanos(1));
    assert_eq!(
        h.answered(1),
        Some(&Answered {
            call: skein_lib::Token::new(1),
            cost: 0,
            free: 0,
            rate: None,
            result: Err(api::Error::Unavailable)
        })
    );
    assert_eq!(h.answers.len(), 1);
    h.run(Duration::from_nanos(9_999_999_999));
    assert_eq!(h.connections_opened(), 1);
    h.run(Duration::from_nanos(1));
    assert_eq!(h.connections_opened(), 2);
    retry_after_states(&limits, waiting);
}
fn retry_after_states(limits: &crate::Limits, waiting: crate::start::Start) {
    use crate::start::{self, Start};
    use skein_forge::forgejo::plans::start::Heard as StartHeard;
    for state in [Start::Checking { next: limits.start_backoff }, Start::Settings { next: limits.start_backoff }] {
        for (now, expected) in [
            (Time::from_nanos(1_000_000_000), waiting),
            (
                Time::from_nanos(u64::MAX - 1),
                Start::Waiting { until: Time::from_nanos(u64::MAX), next: Duration::from_secs(2) },
            ),
        ] {
            assert_eq!(
                start::heard(
                    state,
                    StartHeard::Status { status: 429, retry: Some(Duration::from_secs(20)) },
                    fixtures::WRITER,
                    now,
                    limits
                ),
                expected,
                "{state:?}/{now:?}"
            );
        }
    }
}

#[test]
fn an_unconfigured_forge_is_unavailable_at_zero_cost() {
    use skein_forge::api;
    let mut settings = fixtures::settings();
    settings.forges = skein_lib::List::with_capacity(0);
    let mut h = fixtures::Harness::new(settings);
    h.call(1, api::Repository { forge: 99, repository: 1 }, api::Op::Read(api::Read::Settings));
    h.run(Duration::ZERO);
    assert_eq!(
        h.answered(1),
        Some(&crate::Answered {
            call: skein_lib::Token::new(1),
            cost: 0,
            free: 0,
            rate: None,
            result: Err(api::Error::Unavailable)
        })
    );
    assert_eq!((h.answers.len(), h.connections_opened()), (1, 0));
}
#[test]
fn a_stopped_forge_does_not_stop_another() {
    use skein_forge::api;
    let mut h = fixtures::Harness::new(fixtures::two_forges());
    h.connected(fixtures::FORGE);
    h.answer(fixtures::FORGE, &fixtures::ok(b"{\"id\":1,\"login\":\"other\"}"));
    h.connected(fixtures::OTHER_FORGE);
    h.answer(fixtures::OTHER_FORGE, &fixtures::user_ok());
    h.answer(fixtures::OTHER_FORGE, &fixtures::settings_ok());
    h.call(1, fixtures::OTHER_REPO, api::Op::Read(api::Read::Branch { branch: Box::from(fixtures::BRANCH) }));
    fixtures::branch_answers(&mut h, fixtures::OTHER_FORGE);
    assert_eq!(h.stopped(), Some((fixtures::FORGE, crate::Stopped::OtherUser { user: 1 })));
    assert_eq!(h.start(fixtures::OTHER_FORGE), crate::start::Start::Ready { cap: 2 });
    assert_eq!(
        h.answered(1),
        Some(&crate::Answered {
            call: skein_lib::Token::new(1),
            cost: 2,
            free: 0,
            rate: None,
            result: Ok(api::Answer::Commit(fixtures::BRANCH_HEAD)),
        })
    );
    assert_eq!(h.answers.len(), 1);
    h.run(Duration::from_secs(20));
    assert_eq!(h.stopped(), Some((fixtures::FORGE, crate::Stopped::OtherUser { user: 1 })));
}
#[test]
fn an_idle_connection_is_reassigned_to_another_forge() {
    use skein_forge::api;
    let limits = crate::Limits { connections: fixtures::EVICTION_CONNECTIONS, ..fixtures::limits() };
    let shared = crate::Shared { calls: fixtures::EVICTION_CALLS, ..fixtures::shared() };
    let mut h = fixtures::ready_two(limits, shared);
    h.call(1, fixtures::REPO, api::Op::Read(api::Read::Branch { branch: Box::from(fixtures::BRANCH) }));
    h.run(Duration::ZERO);
    h.connected(fixtures::FORGE);
    fixtures::branch_answers(&mut h, fixtures::FORGE);
    let before = h.connections_opened();
    h.call(2, fixtures::OTHER_REPO, api::Op::Read(api::Read::Branch { branch: Box::from(fixtures::BRANCH) }));
    h.run(Duration::ZERO);
    h.connected(fixtures::OTHER_FORGE);
    fixtures::branch_answers(&mut h, fixtures::OTHER_FORGE);
    assert_eq!(h.connections_opened(), before.checked_add(fixtures::EVICTION_CONNECTIONS).expect("next connection"));
    assert_eq!(
        (h.connections_for(fixtures::FORGE), h.connections_for(fixtures::OTHER_FORGE)),
        (fixtures::EVICTION_NO_CONNECTIONS, fixtures::EVICTION_CONNECTIONS)
    );
    assert_eq!(
        h.answered(2),
        Some(&crate::Answered {
            call: skein_lib::Token::new(2),
            cost: 2,
            free: 0,
            rate: None,
            result: Ok(api::Answer::Commit(fixtures::BRANCH_HEAD)),
        })
    );
    assert_eq!(h.answers.len(), fixtures::EVICTION_ANSWERS);
}
#[test]
fn start_backoff_doubles_to_its_maximum() {
    use crate::start::Start;
    let mut h = fixtures::Harness::new(fixtures::settings());
    for (wait, until, next, before) in [(1, 1, 2, 1), (2, 3, 4, 2), (4, 7, 8, 3), (8, 15, 8, 4), (8, 23, 8, 5)] {
        h.refuse(fixtures::FORGE);
        assert_eq!(
            h.start(fixtures::FORGE),
            Start::Waiting {
                until: Time::ZERO.saturating_add(Duration::from_secs(until)),
                next: Duration::from_secs(next),
            }
        );
        h.run(Duration::from_nanos(wait * 1_000_000_000 - 1));
        assert_eq!(h.connections_opened(), before, "{wait}: before deadline");
        h.run(Duration::from_nanos(1));
        assert_eq!(h.connections_opened(), before + 1, "{wait}: at deadline");
    }
}
#[test]
fn terminal_start_states_ignore_every_start_answer() {
    use crate::start::{self, Start};
    use skein_forge::forgejo::plans::start::Heard as StartHeard;
    // The terminal matrix uses the 23-second backoff horizon and the same limits.
    let now = Time::ZERO.saturating_add(Duration::from_secs(23));
    let limits = fixtures::limits();
    for state in [Start::Unconnected, Start::Ready { cap: 2 }, Start::Stopped(crate::Stopped::OtherUser { user: 1 })] {
        for heard in [
            StartHeard::Unanswered,
            StartHeard::User { id: fixtures::WRITER },
            StartHeard::ApiSettings { max_response_items: 2 },
        ] {
            assert_eq!(start::heard(state, heard, fixtures::WRITER, now, &limits), state, "{state:?}/{heard:?}");
        }
    }
}
