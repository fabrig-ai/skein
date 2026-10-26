use super::{Limits, Settings, Visitor};
use core::net::{Ipv4Addr, SocketAddr};
use skein_lib::{Duration, Time, Wall};
use skein_shell::Host;
use skein_ui_web::{
    bridge::Duty,
    fetch::{Exchange, Report},
    transport::StreamDuty,
};
fn settings() -> Settings {
    Settings {
        listener: SocketAddr::from((Ipv4Addr::LOCALHOST, 7878)),
        host: b"application.example".as_slice().into(),
        origin: b"http://application.example".as_slice().into(),
        limits: Limits {
            io: skein_io::Limits {
                sockets: 4,
                names: 0,
                refusals: 2,
                intake: 512,
                receive: 512,
                output: 512,
                sends: 4,
                accepts: 2,
                backlog: 4,
                close_timeout: Duration::from_millis(10),
                retry: Duration::from_millis(1),
            },
            http: skein_http::client::Limits { request: 512, head: 512, headers: 8, read: 32, send: 32 },
            exchanges: 1,
            duties: 4,
            request: 512,
            answer: 32,
            piece: 8,
            cookies: 2,
            cookie: 32,
        },
    }
}
fn exchange() -> Duty {
    Duty::Exchange(Exchange {
        id: 1,
        path: b"/".as_slice().into(),
        method: "GET".into(),
        headers: vec![],
        body: Box::from([]),
        bound: 32,
    })
}
#[test]
#[should_panic(expected = "exchanges within the in-flight bound")]
fn an_exchange_past_the_bound_breaks_the_contract() {
    let mut visitor = Visitor::new(settings());
    visitor.duty(exchange());
    let mut second = exchange();
    if let Duty::Exchange(exchange) = &mut second {
        exchange.id = 2;
    }
    visitor.duty(second);
}
#[test]
#[should_panic(expected = "an exchange id is not in flight")]
fn a_duplicate_id_breaks_the_contract() {
    let mut visitor = Visitor::new(settings());
    visitor.duty(exchange());
    visitor.duty(exchange());
}
#[test]
#[should_panic(expected = "duties within the bound")]
fn a_duty_past_the_bound_breaks_the_contract() {
    let mut settings = settings();
    settings.limits.duties = 1;
    let mut visitor = Visitor::new(settings);
    visitor.duty(Duty::ReadOn);
    visitor.duty(Duty::ReadOn);
}
#[test]
#[should_panic(expected = "answer within its bound")]
fn an_answer_past_the_largest_bound_breaks_the_contract() {
    let mut visitor = Visitor::new(settings());
    let mut duty = exchange();
    if let Duty::Exchange(exchange) = &mut duty {
        exchange.bound = 33;
    }
    visitor.duty(duty);
}
#[test]
#[should_panic(expected = "request within its bound")]
fn a_request_past_its_bound_breaks_the_contract() {
    let mut visitor = Visitor::new(settings());
    let mut duty = exchange();
    if let Duty::Exchange(exchange) = &mut duty {
        exchange.body = vec![0; 513].into();
    }
    visitor.duty(duty);
}
#[test]
#[should_panic(expected = "stream piece within its bound")]
fn a_piece_past_its_bound_breaks_the_contract() {
    let mut visitor = Visitor::new(settings());
    visitor.duty(Duty::Stream(StreamDuty::Open { path: b"/".as_slice().into(), headers: vec![], bound: 9 }));
}
#[test]
fn bridge_races_without_an_active_request_are_ignored() {
    let mut visitor = Visitor::new(settings());
    visitor.duty(Duty::Abandon { exchange: 999 });
    visitor.duty(Duty::ReadOn);
    visitor.duty(Duty::Stream(StreamDuty::Close));
    visitor.iterate(Time::ZERO, Wall::EPOCH);
    assert_eq!(visitor.report(), None);
    visitor.close();
    visitor.iterate(Time::ZERO, Wall::EPOCH);
    assert!(visitor.is_empty());
}
#[test]
fn replacing_a_stream_and_explicit_close_each_make_one_terminal() {
    let mut visitor = Visitor::new(settings());
    for _ in 0..2 {
        visitor.duty(Duty::Stream(StreamDuty::Open { path: b"/".as_slice().into(), headers: vec![], bound: 8 }));
    }
    visitor.duty(Duty::Stream(StreamDuty::Close));
    visitor.iterate(Time::ZERO, Wall::EPOCH);
    assert_eq!(visitor.report(), Some(Report::StreamEnded));
    assert_eq!(visitor.report(), Some(Report::StreamEnded));
    assert_eq!(visitor.report(), None);
}
