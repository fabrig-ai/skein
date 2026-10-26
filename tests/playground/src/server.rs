//! A scripted HTTP server over the same io and HTTP machines as an application.
use super::{QUEUE, io_limits};
use core::net::{Ipv4Addr, SocketAddr};
use skein_http::{Header, Method, server as http};
use skein_io::{self as io, kernel};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use skein_world::Host;

pub const ADDRESS: SocketAddr = SocketAddr::new(core::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 7878);
const LISTENER: Token = Token::new(0);
const CONNECTIONS: u32 = 8;
const BYTES: u32 = 4096;
const ROOM: u32 = 8;
fn limits() -> http::Limits {
    http::Limits { head: BYTES, headers: 16, body: u64::from(BYTES), read: 256, response: 512, send: 256 }
}
struct Connection {
    owner: Token,
    socket: Token,
    machine: http::Server,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    below: Queue<Down>,
    call: Option<http::Call>,
    received: Vec<u8>,
    answer: Vec<u8>,
    offset: usize,
    write: usize,
    ready: Option<Time>,
    responded: bool,
    stream: bool,
    truncated: bool,
}
impl Connection {
    fn new(owner: Token, socket: Token) -> Self {
        let mut requests = Queue::with_capacity(ROOM);
        requests.push(http::Request::Next);
        Self {
            owner,
            socket,
            machine: http::Server::new(&limits()),
            events: Queue::with_capacity(ROOM),
            requests,
            below: Queue::with_capacity(ROOM),
            call: None,
            received: Vec::with_capacity(usize::try_from(BYTES).expect("body fits")),
            answer: Vec::new(),
            offset: 0,
            write: 256,
            ready: None,
            responded: false,
            stream: false,
            truncated: false,
        }
    }
    fn takes(&self) -> bool {
        self.events.room() >= http::UP_MAX_OUT.above && self.below.room() >= http::UP_MAX_OUT.below
    }
    fn read(&mut self) {
        self.requests.push(http::Request::Body(Down::Demand { read: Read::Arrived { max: limits().read }, room: 0 }));
    }
    fn respond(&mut self) {
        let call = self.call.as_ref().expect("request head");
        let target = std::str::from_utf8(&call.target).expect("HTTP target ASCII");
        let mut status = 200;
        let mut headers = Vec::new();
        if target.starts_with("/echo") || target == "/other" || target == "/echoes" {
            self.answer.extend_from_slice(call.method.as_bytes());
            self.answer.push(b'\n');
            self.answer.extend_from_slice(&call.target);
            self.answer.push(b'\n');
            for name in [b"host".as_slice(), b"origin", b"cookie"] {
                if let Some(value) = call.header(name) {
                    self.answer.extend_from_slice(name);
                    self.answer.extend_from_slice(b": ");
                    self.answer.extend_from_slice(value);
                } else {
                    self.answer.extend_from_slice(name);
                    self.answer.extend_from_slice(b": <absent>");
                }
                self.answer.push(b'\n');
            }
            self.answer.extend_from_slice(&self.received);
        } else if let Some(value) = target.strip_prefix("/set?") {
            headers.push(Header { name: b"Set-Cookie".as_slice().into(), value: decode(value).into() });
        } else if let Some(value) = target.strip_prefix("/set-fail?") {
            headers.push(Header { name: b"Set-Cookie".as_slice().into(), value: decode(value).into() });
            self.answer.resize(33, b'x');
        } else if let Some(value) = target.strip_prefix("/set-truncated?") {
            headers.push(Header { name: b"Set-Cookie".as_slice().into(), value: decode(value).into() });
            self.answer.resize(1, b'x');
            self.truncated = true;
        } else if let Some(size) = target.strip_prefix("/bytes/") {
            let size = size.parse::<u32>().expect("scripted size");
            assert!(size <= BYTES, "scripted answer within server bound");
            self.answer.resize(usize::try_from(size).expect("size fits"), b'x');
        } else if let Some(code) = target.strip_prefix("/status/") {
            status = code.parse().expect("scripted status");
        } else if let Some(shape) = target.strip_prefix("/stream/") {
            let (count, size) = shape.split_once('x').expect("stream shape");
            let count = count.parse::<usize>().expect("stream writes");
            self.write = size.parse().expect("stream write size");
            assert!(self.write > 0 && self.write <= usize::try_from(limits().send).expect("send fits"));
            let length = count.checked_mul(self.write).expect("stream length");
            assert!(length <= usize::try_from(BYTES).expect("bound fits"));
            self.answer.resize(length, b's');
            self.stream = true;
        } else if target != "/slow" {
            status = 404;
        }
        let head_only = call.method == Method::Head || status == 204 || status == 304;
        let response = http::Response {
            status,
            headers: headers.into_boxed_slice(),
            body: if head_only || (self.answer.is_empty() && !self.stream) {
                http::Body::None
            } else if self.stream {
                http::Body::Chunked
            } else {
                http::Body::Length(
                    u64::try_from(self.answer.len() + usize::from(self.truncated)).expect("answer length"),
                )
            },
            close: true,
        };
        self.requests.push(http::Request::Respond(response));
        if !head_only && (!self.answer.is_empty() || self.stream) {
            self.room();
        }
        self.responded = true;
        self.ready = None;
    }
    fn room(&mut self) {
        self.requests.push(http::Request::Reply(Down::Demand {
            read: Read::Nothing,
            room: u32::try_from(self.write.min(self.answer.len().saturating_sub(self.offset)).max(1))
                .expect("write fits"),
        }));
    }
    fn progress(&mut self, now: Time, wall: Wall, slow: Duration, io_requests: &mut Queue<io::Request>) -> bool {
        if self.truncated
            && self.responded
            && self.offset == self.answer.len()
            && self.requests.is_empty()
            && self.below.is_empty()
        {
            return true;
        }
        if let Some(event) = self.events.pop() {
            match event {
                http::Event::Call(call) => {
                    if call.target.as_ref() == b"/reset" {
                        io_requests.push(io::Request::Abort { entity: self.socket });
                        return true;
                    }
                    self.call = Some(call);
                    self.read();
                }
                http::Event::Body(Up::Bytes(bytes)) => {
                    self.received.extend_from_slice(&bytes);
                    self.read();
                }
                http::Event::Body(Up::End) => {
                    self.ready = Some(if self.call.as_ref().expect("head").target.as_ref() == b"/slow" {
                        now.saturating_add(slow)
                    } else {
                        now
                    });
                }
                http::Event::Reply(Up::Room) => {
                    let end = self.offset.saturating_add(self.write).min(self.answer.len());
                    self.requests.push(http::Request::Reply(Down::Send(self.answer[self.offset..end].into())));
                    self.offset = end;
                    if end == self.answer.len() {
                        if !self.truncated {
                            self.requests.push(http::Request::Reply(Down::Finish));
                        }
                    } else {
                        self.room();
                    }
                }
                http::Event::Done(_)
                | http::Event::Ended
                | http::Event::Failed(_)
                | http::Event::Body(Up::Failed(_))
                | http::Event::Reply(Up::Failed(_)) => return true,
                http::Event::Closed => {}
                http::Event::Refused(refusal) => panic!("scripted response refused: {refusal:?}"),
                http::Event::Body(Up::Room) | http::Event::Reply(Up::Bytes(_) | Up::End) => {
                    unreachable!("HTTP server directions")
                }
            }
        }
        if !self.responded && self.ready.is_some_and(|at| at <= now) {
            self.respond();
        }
        if self.events.room() >= http::DOWN_MAX_OUT.above
            && self.below.room() >= http::DOWN_MAX_OUT.below
            && let Some(request) = self.requests.pop()
        {
            http::down(
                &mut self.machine,
                &Env { now, wall, limits: limits() },
                request,
                &mut self.events,
                &mut self.below,
            );
        }
        while let Some(down) = self.below.pop() {
            io_requests.push(io::Request::Stream { stream: self.socket, down });
        }
        false
    }
    fn has_work(&self, now: Time) -> bool {
        !self.requests.is_empty()
            || !self.events.is_empty()
            || !self.below.is_empty()
            || self.ready.is_some_and(|at| at <= now)
    }
}
fn decode(text: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next().expect("percent digit")).to_digit(16).expect("hex digit");
            let low = char::from(bytes.next().expect("percent digit")).to_digit(16).expect("hex digit");
            result.push(u8::try_from(high * 16 + low).expect("byte"));
        } else {
            result.push(byte);
        }
    }
    result
}

pub struct Server {
    io: io::Io,
    events: Queue<io::Event>,
    requests: Queue<io::Request>,
    completions: Queue<kernel::Complete>,
    submissions: Queue<kernel::Submit>,
    listener: Option<Token>,
    connections: Vec<Connection>,
    serial: u64,
    pub ready: bool,
    pub answers: usize,
    pub accepted: usize,
    pub closing: bool,
    slow: Duration,
}
impl Server {
    #[must_use]
    pub fn new(slow: Duration) -> Self {
        let mut requests = Queue::with_capacity(QUEUE);
        requests.push(io::Request::Listen { owner: LISTENER, addr: io::Addr::Ip(ADDRESS) });
        Self {
            io: io::Io::new(&io_limits()),
            events: Queue::with_capacity(QUEUE),
            requests,
            completions: Queue::with_capacity(QUEUE),
            submissions: Queue::with_capacity(QUEUE),
            listener: None,
            connections: Vec::with_capacity(usize::try_from(CONNECTIONS).expect("count fits")),
            serial: 0,
            ready: false,
            answers: 0,
            accepted: 0,
            closing: false,
            slow,
        }
    }
    fn event(&mut self, event: io::Event, now: Time, wall: Wall) {
        match event {
            io::Event::Listening { listener, .. } => {
                self.listener = Some(listener);
                self.ready = true;
            }
            io::Event::Accepted { socket, .. } => {
                if self.closing || self.connections.len() == usize::try_from(CONNECTIONS).expect("count fits") {
                    self.requests.push(io::Request::Reject { socket });
                } else {
                    self.accepted += 1;
                    self.serial += 1;
                    let owner = Token::new(self.serial);
                    self.requests.push(io::Request::Bind { socket, owner });
                    self.connections.push(Connection::new(owner, socket));
                }
            }
            io::Event::Stream { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    http::up(
                        &mut connection.machine,
                        &Env { now, wall, limits: limits() },
                        up,
                        &mut connection.events,
                        &mut connection.below,
                    );
                }
            }
            io::Event::Closed { owner } | io::Event::Failed { owner, .. } => {
                if let Some(at) = self.connections.iter().position(|connection| connection.owner == owner) {
                    let connection = self.connections.remove(at);
                    self.requests.push(io::Request::Close { entity: connection.socket });
                }
            }
            io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Output { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => unreachable!("scripted server only listens"),
        }
    }
}
impl Host for Server {
    fn iterate(&mut self, now: Time, wall: Wall) {
        let env = Env { now, wall, limits: io_limits() };
        if self.closing {
            if let Some(entity) = self.listener.take() {
                self.requests.push(io::Request::Close { entity });
            }
            for connection in self.connections.drain(..) {
                self.requests.push(io::Request::Close { entity: connection.socket });
            }
        }
        if self.events.room() >= io::MAX_OUT_UP.events && self.submissions.room() >= io::MAX_OUT_UP.submissions {
            if self.io.is_ready() {
                io::resume(&mut self.io, &env, &mut self.events, &mut self.submissions);
            } else if let Some(complete) = self.completions.pop() {
                io::up(&mut self.io, &env, complete, &mut self.events, &mut self.submissions);
            } else if self.io.is_due(now) {
                io::fire(&mut self.io, &env, &mut self.events, &mut self.submissions);
            }
        }
        if self.requests.room() >= 4
            && self.connections.iter().all(Connection::takes)
            && let Some(event) = self.events.pop()
        {
            self.event(event, now, wall);
        }
        let mut at = 0;
        while at < self.connections.len() && self.requests.room() > ROOM {
            if self.connections[at].progress(now, wall, self.slow, &mut self.requests) {
                let connection = self.connections.remove(at);
                self.answers += usize::from(connection.responded);
                self.requests.push(io::Request::Close { entity: connection.socket });
            } else {
                at += 1;
            }
        }
        while self.io.takes() && self.submissions.room() >= io::MAX_OUT_DOWN.submissions {
            let Some(request) = self.requests.pop() else { break };
            io::down(&mut self.io, &env, request, &mut self.submissions);
        }
        self.io.reclaim();
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        &mut self.completions
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        &mut self.submissions
    }
    fn work_pending(&self, now: Time) -> bool {
        self.io.is_ready()
            || self.io.is_due(now)
            || !self.events.is_empty()
            || !self.requests.is_empty()
            || !self.completions.is_empty()
            || self.connections.iter().any(|connection| connection.has_work(now))
            || (self.closing && self.listener.is_some())
    }
    fn next_deadline(&self) -> Option<Time> {
        self.connections.iter().filter_map(|connection| connection.ready).chain(self.io.next_deadline()).min()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        self.connections.iter().filter_map(|connection| connection.ready).min()
    }
    fn is_empty(&self) -> bool {
        self.closing
            && self.io.is_empty()
            && self.listener.is_none()
            && self.connections.is_empty()
            && self.events.is_empty()
            && self.requests.is_empty()
            && self.completions.is_empty()
            && self.submissions.is_empty()
    }
    fn worst_case(&self) -> u64 {
        // The peer's fixed fixture bounds: eight clients, 4096-byte heads and
        // bodies, and bounded protocol and io queues with their payloads.
        io::worst_case(&io_limits()).expect("server io bound")
            + u64::from(QUEUE)
                * (u64::try_from(
                    size_of::<io::Event>()
                        + size_of::<io::Request>()
                        + size_of::<kernel::Complete>()
                        + size_of::<kernel::Submit>(),
                )
                .expect("queue item sizes")
                    + u64::from(io_limits().intake.max(io_limits().output)))
            + u64::from(CONNECTIONS)
                * (u64::try_from(size_of::<Connection>()).expect("connection size")
                    + http::worst_case(&limits()).expect("HTTP bound")
                    + u64::from(ROOM)
                        * (u64::try_from(size_of::<http::Event>() + size_of::<http::Request>() + size_of::<Down>())
                            .expect("protocol queue item sizes")
                            + 4 * u64::from(BYTES))
                    + 8 * u64::from(BYTES))
    }
    fn operations(&self) -> u32 {
        io::operations(&io_limits()).expect("server operations fit")
    }
}
