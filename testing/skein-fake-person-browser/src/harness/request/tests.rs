use super::*;

const SOCKET: Token = Token::new(300);

fn attempt() -> (Exchange, super::super::Script) {
    use super::super::{Script, Step};
    let mut script = Script::new(Box::new([Step::Request {
        method: skein_http::Method::Get,
        path: b"/".as_slice().into(),
        host: b"other.test".as_slice().into(),
        headers: vec![],
        body: Box::new([]),
        status: 200,
        within: skein_lib::Duration::from_secs(1),
    }]));
    script.next(&mut crate::tests::face(), Time::ZERO);
    let asked = script.request().expect("request step");
    (Exchange::new(0, 1, asked, "127.0.0.1:9000", Time::ZERO), script)
}

fn connected(exchange: &mut Exchange) {
    exchange.event(io::Event::Connecting { owner: exchange.owner, socket: SOCKET });
    exchange.event(io::Event::Connected { owner: exchange.owner });
}

fn sent(exchange: &mut Exchange) {
    connected(exchange);
    exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::Room });
}

fn reported(mut exchange: Exchange, mut script: super::super::Script, failure: RequestFailure, words: &str) {
    assert_eq!(exchange.failure, Some(failure));
    assert_eq!(exchange.status, Some(0));
    exchange.answer(&mut script);
    let mut face = crate::tests::face();
    script.check_deadline(&mut face, Time::ZERO);
    assert_eq!(face.failure(), None, "the existing request deadline is unchanged");
    script.check_deadline(&mut face, Time::from_nanos(1_000_000_000));
    assert_eq!(
        face.failure().as_deref(),
        Some(format!("did not receive status 200, saw Some(0); request failed: {words} within its bound").as_str())
    );
}

#[test]
fn io_failures_reach_the_request_report_without_losing_their_kind() {
    for (error, words) in [
        (io::Error::Refused, "Io(Refused)"),
        (io::Error::Reset, "Io(Reset)"),
        (io::Error::TimedOut, "Io(TimedOut)"),
        (io::Error::Unreachable, "Io(Unreachable)"),
        (io::Error::Busy, "Io(Busy)"),
        (io::Error::Other, "Io(Other)"),
    ] {
        let (mut exchange, script) = attempt();
        exchange.event(io::Event::Failed { owner: exchange.owner, error });
        reported(exchange, script, RequestFailure::Io(error), words);
    }
}

#[test]
fn eof_reports_whether_the_request_was_sent_and_any_response_line_arrived() {
    for (send, head, error, words) in [
        (false, false, client::Error::Closed(None), "Http(Closed(None))"),
        (true, false, client::Error::Truncated { answered: false }, "Http(Truncated { answered: false })"),
        (true, true, client::Error::Truncated { answered: true }, "Http(Truncated { answered: true })"),
    ] {
        let (mut exchange, script) = attempt();
        if send {
            sent(&mut exchange);
        } else {
            connected(&mut exchange);
        }
        if head {
            exchange.event(io::Event::Stream {
                owner: exchange.owner,
                up: stream::Up::Bytes(b"HTTP/1.1 200 OK\r\n".as_slice().into()),
            });
        }
        exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::End });
        reported(exchange, script, RequestFailure::Http(error), words);
    }
}

#[test]
fn stream_faults_retain_their_kind_before_and_after_the_request_is_sent() {
    use stream::Fault;
    for (fault, closed, sending) in [
        (Fault::Reset, "Http(Closed(Some(Reset)))", "Http(Stream(Reset))"),
        (Fault::Invalid, "Http(Closed(Some(Invalid)))", "Http(Stream(Invalid))"),
        (Fault::Other, "Http(Closed(Some(Other)))", "Http(Stream(Other))"),
    ] {
        for send in [false, true] {
            let (mut exchange, script) = attempt();
            if send {
                sent(&mut exchange);
            } else {
                connected(&mut exchange);
            }
            exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::Failed(fault) });
            let error = if send { client::Error::Stream(fault) } else { client::Error::Closed(Some(fault)) };
            reported(exchange, script, RequestFailure::Http(error), if send { sending } else { closed });
        }
    }
}

#[test]
fn malformed_and_oversized_response_heads_name_the_http_error() {
    for (response, error, words) in [
        (b"INVALID\r\n".as_slice(), client::Error::Status, "Http(Status)"),
        (b"HTTP/2.0 200 OK\r\n".as_slice(), client::Error::Version, "Http(Version)"),
        (b"HTTP/1.1 200 OK\r\nInvalid\r\n".as_slice(), client::Error::Header, "Http(Header)"),
        (b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n".as_slice(), client::Error::Framing, "Http(Framing)"),
        (b"HTTP/1.1 200 OK\r\nX: a\r\nY: b\r\n".as_slice(), client::Error::TooManyHeaders, "Http(TooManyHeaders)"),
        (b"xxxxxxxxxxxxxxxx".as_slice(), client::Error::HeadTooLong, "Http(HeadTooLong)"),
        (b"HTTP/1.1 101 Switching\r\n\r\n".as_slice(), client::Error::Upgrade, "Http(Upgrade)"),
    ] {
        let (mut exchange, script) = attempt();
        exchange.env.limits.headers = 1;
        if error == client::Error::HeadTooLong {
            exchange.env.limits.head = u32::try_from(response.len()).unwrap();
        }
        sent(&mut exchange);
        for line in response.split_inclusive(|byte| *byte == b'\n') {
            exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::Bytes(line.into()) });
        }
        reported(exchange, script, RequestFailure::Http(error), words);
    }
}

#[test]
fn invalid_request_heads_name_the_clients_refusal() {
    use client::Refusal;
    for (refusal, words) in [
        (Refusal::Target, "Http(Refused(Target))"),
        (Refusal::Name, "Http(Refused(Name))"),
        (Refusal::Value, "Http(Refused(Value))"),
        (Refusal::Reserved, "Http(Refused(Reserved))"),
        (Refusal::Host, "Http(Refused(Host))"),
        (Refusal::TooLong, "Http(Refused(TooLong))"),
    ] {
        let (mut exchange, script) = attempt();
        match refusal {
            Refusal::Target => exchange.call.target = Box::new([]),
            Refusal::TooLong => exchange.env.limits.request = 1,
            Refusal::Name | Refusal::Value | Refusal::Reserved | Refusal::Host => {
                let (name, value) = match refusal {
                    Refusal::Name => (b"bad name".as_slice(), b"a".as_slice()),
                    Refusal::Value => (b"X".as_slice(), b"\n".as_slice()),
                    Refusal::Reserved => (b"Connection".as_slice(), b"close".as_slice()),
                    Refusal::Host => (b"Host".as_slice(), b"duplicate.test".as_slice()),
                    Refusal::Target | Refusal::TooLong => unreachable!("handled above"),
                };
                exchange.call.headers = Box::new([
                    skein_http::Header { name: b"Host".as_slice().into(), value: b"other.test".as_slice().into() },
                    skein_http::Header { name: name.into(), value: value.into() },
                ]);
            }
        }
        connected(&mut exchange);
        reported(exchange, script, RequestFailure::Http(client::Error::Refused(refusal)), words);
    }
}

#[test]
fn no_response_names_the_request_deadline_without_inventing_a_network_error() {
    let (mut exchange, mut script) = attempt();
    sent(&mut exchange);
    exchange.answer(&mut script);
    let mut face = crate::tests::face();
    script.check_deadline(&mut face, Time::from_nanos(999_999_999));
    assert_eq!(face.failure(), None);
    script.check_deadline(&mut face, Time::from_nanos(1_000_000_000));
    assert_eq!(
        face.failure().as_deref(),
        Some("did not receive status 200, saw None; no response before the request deadline within its bound")
    );
}

#[test]
fn a_status_only_play_keeps_its_failure_notification() {
    struct StatusOnly(Option<u16>);
    impl Play for StatusOnly {
        fn next(&mut self, _face: &mut crate::BrowserFace<'_>, _now: Time) -> skein_fake_person::Next<Box<[u8]>> {
            skein_fake_person::Next::Wait { until: None }
        }
        fn answered(&mut self, status: u16) {
            self.0 = Some(status);
        }
    }
    let (mut exchange, _) = attempt();
    exchange.event(io::Event::Failed { owner: exchange.owner, error: io::Error::Refused });
    let mut play = StatusOnly(None);
    exchange.answer(&mut play);
    assert_eq!(play.0, Some(0));
}

#[test]
fn teardown_after_a_status_closes_the_client_once_before_the_socket_settles() {
    let asked = Asked {
        method: skein_http::Method::Get,
        path: b"/".as_slice().into(),
        host: b"other.test".as_slice().into(),
        headers: vec![],
        body: Box::new([]),
    };
    let mut exchange = Exchange::new(0, 1, asked, "127.0.0.1:9000", Time::ZERO);
    exchange.requests.clear();
    exchange.event(io::Event::Connecting { owner: exchange.owner, socket: SOCKET });
    exchange.event(io::Event::Connected { owner: exchange.owner });
    exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::Room });
    exchange.requests.clear();
    for line in b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".split_inclusive(|byte| *byte == b'\n') {
        exchange.requests.clear();
        exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::Bytes(line.into()) });
    }
    assert_eq!(exchange.status, Some(200));
    exchange.event(io::Event::Stream { owner: exchange.owner, up: stream::Up::Failed(stream::Fault::Reset) });
    assert_eq!(exchange.status, Some(200), "a body failure after the observed head cannot replace its status");
    assert_eq!(exchange.failure, None);
    assert_eq!(exchange.client.waiting(), client::Waiting::Nothing);
    assert!(!exchange.closed);
    exchange.stop();
    assert_eq!(exchange.requests, VecDeque::from([Request::Abort { entity: SOCKET }]));
    assert!(exchange.above.is_empty());
    exchange.event(io::Event::Closed { owner: exchange.owner });
    assert!(exchange.closed);
}
