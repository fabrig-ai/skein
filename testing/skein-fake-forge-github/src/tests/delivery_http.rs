use super::{fixtures as f, wall};
use crate::{server, service::Service};
use skein_lib::{
    Env, Intake, List, Queue, Token, bytes,
    stream::{Down, Read, Up},
};

struct Connection {
    server: server::Server,
    service: Service,
    env: Env<crate::Limits>,
    above: Queue<server::Event>,
    below: Queue<Down>,
    read: Read,
}
impl Connection {
    fn new() -> Self {
        let limits = f::limits();
        let mut connection = Self {
            server: server::Server::new(Token::new(1), &limits).unwrap(),
            service: Service::new(f::config(), &limits, 1).unwrap(),
            env: Env { now: skein_lib::Time::ZERO, wall: wall(1200), limits },
            above: Queue::with_capacity(server::MAX_UP),
            below: Queue::with_capacity(server::MAX_DOWN),
            read: Read::Nothing,
        };
        server::start(
            &mut connection.server,
            &mut connection.service,
            &connection.env,
            &mut connection.above,
            &mut connection.below,
        );
        connection
    }

    fn ask(&mut self, wire: &[u8]) -> Box<[u8]> {
        let mut input = Intake::with_capacity(self.env.limits.http.head);
        input.append(wire).unwrap();
        let mut output = List::with_capacity(self.env.limits.response);
        for _ in 0..512_u32 {
            drop(self.server.take_head());
            assert!(self.above.is_empty(), "App log reads keep the connection open and need no domain calls");
            let event = if let Some(bytes) = input.meet(self.read) {
                self.read = Read::Nothing;
                Some(Up::Bytes(bytes))
            } else if let Some(request) = self.below.pop() {
                match request {
                    Down::Demand { read, room } => {
                        self.read = read;
                        if room > 0 { Some(Up::Room) } else { None }
                    }
                    Down::Send(chunk) => {
                        for byte in chunk {
                            output.push(byte).unwrap();
                        }
                        None
                    }
                    Down::Finish => panic!("the HTTP connection persists"),
                }
            } else if input.is_empty() && self.server.is_settled() && !self.server.has_work() {
                return output.into_boxed();
            } else {
                server::resume(&mut self.server, &mut self.service, &self.env, &mut self.above, &mut self.below);
                None
            };
            if let Some(event) = event {
                server::up(&mut self.server, &mut self.service, &self.env, event, &mut self.above, &mut self.below);
            }
        }
        panic!("log read settles within the fixture bound")
    }
}

#[test]
fn a_matching_delivery_log_read_is_answered_304_without_a_body() {
    let mut connection = Connection::new();
    let jwt = f::authorization(super::app().key, 1000, 1600);
    let head = f::parts(&[
        b"GET /app/hook/deliveries?per_page=1 HTTP/1.1\r\nHost: fake.test\r\nUser-Agent: example-app\r\nX-GitHub-Api-Version: ",
        skein_forge::github::pin::API_VERSION.as_bytes(),
        b"\r\nAuthorization: ",
        jwt.expose(),
        b"\r\n",
    ]);
    let first = connection.ask(&f::parts(&[&head, b"\r\n"]));
    assert!(first.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(first.ends_with(b"\r\n\r\n[]"));
    let marker = b"\r\nETag: ";
    let start = bytes::find(&first, marker).expect("log validator").checked_add(marker.len()).unwrap();
    let tail = first.get(start..).unwrap();
    let end = bytes::find(tail, b"\r\n").unwrap();
    let etag = tail.get(..end).unwrap();

    let same = connection.ask(&f::parts(&[&head, b"If-None-Match: ", etag, b"\r\n\r\n"]));
    assert!(same.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));
    let body_start = bytes::find(&same, b"\r\n\r\n").unwrap().checked_add(4).unwrap();
    assert_eq!(body_start, same.len(), "304 ends with its head and sends no body bytes");
    assert!(bytes::find(&same, &f::parts(&[marker, etag, b"\r\n"])).is_some());
    assert!(bytes::find(&same, b"\r\nContent-Length:").is_none());
    assert!(bytes::find(&same, b"\r\nTransfer-Encoding:").is_none());

    let third = connection.ask(&f::parts(&[&head, b"\r\n"]));
    assert_eq!(third, first, "the same connection serves the next unconditional log read normally");
}

#[test]
fn browser_favicon_without_api_headers_is_bodyless_and_keeps_the_connection() {
    let mut connection = Connection::new();
    // Chromium asks for this resource while following the App's browser pages.
    let request = b"GET /favicon.ico HTTP/1.1\r\nHost: fake.test\r\nUser-Agent: Chromium\r\n\r\n";
    let first = connection.ask(request);
    assert!(first.starts_with(b"HTTP/1.1 204 No Content\r\n"), "favicon: {first:?}");
    let body_start = bytes::find(&first, b"\r\n\r\n").unwrap().checked_add(4).unwrap();
    assert_eq!(body_start, first.len(), "the optional icon has no body");
    assert!(bytes::find(&first, b"\r\nContent-Length:").is_none());
    assert!(bytes::find(&first, b"\r\nTransfer-Encoding:").is_none());

    let api = connection.ask(b"GET /app HTTP/1.1\r\nHost: fake.test\r\nUser-Agent: Chromium\r\n\r\n");
    assert!(api.starts_with(b"HTTP/1.1 400 Bad Request\r\n"), "API reads still require their version header");
    assert_eq!(connection.ask(request), first, "the same connection serves another browser resource");
}
