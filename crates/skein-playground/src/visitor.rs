//! The visitor carries fetch's duties over simulated HTTP connections (playground.md, 4).
use core::net::SocketAddr;
use skein_http::{Header, Method, client as http};
use skein_io::{self as io, kernel};
use skein_lib::{Env, Queue, Time, Token, Wall};
use skein_shell::Host;
use skein_ui_web::{
    bridge::Duty,
    fetch::{Exchange, Report},
    transport::StreamDuty,
};
mod connection;
mod cookie;
use connection::{Connection, Kind};

/// What the application names for its visitor.
pub struct Settings {
    /// The application's clients' listener, in the world.
    pub listener: SocketAddr,
    /// The deployment's address for that listener, sent as Host.
    pub host: Box<[u8]>,
    /// The whole same-origin address, sent except on GET and HEAD.
    pub origin: Box<[u8]>,
    pub limits: Limits,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub io: io::Limits,
    pub http: http::Limits,
    /// Exchanges in flight at once, the page's fetch bound.
    pub exchanges: u32,
    /// Duties waiting for the visitor's next turn.
    pub duties: u32,
    /// The most heap bytes a duty holds, including header containers and spare capacity.
    pub request: u32,
    /// The largest bound an exchange's answer may have.
    pub answer: u32,
    /// The largest bound a stream's piece may have.
    pub piece: u32,
    /// Cookies the jar holds; one past it is refused.
    pub cookies: u32,
    /// The longest cookie, its name and value.
    pub cookie: u32,
}

/// A process of the world, with bounded inputs, reports and cookie storage.
pub struct Visitor {
    settings: Settings,
    io: io::Io,
    events: Queue<io::Event>,
    requests: Queue<io::Request>,
    completions: Queue<kernel::Complete>,
    submissions: Queue<kernel::Submit>,
    duties: Queue<Duty>,
    reports: Queue<Report>,
    connections: Vec<Connection>,
    flights: Vec<u32>,
    jar: cookie::Jar,
    serial: u64,
    closing: bool,
}
impl Visitor {
    #[must_use]
    pub fn new(settings: Settings) -> Self {
        let limits = settings.limits;
        assert!(
            settings.host.len() <= usize::try_from(limits.request).expect("request fits"),
            "Host within request bound"
        );
        assert!(
            settings.origin.len() <= usize::try_from(limits.request).expect("request fits"),
            "Origin within request bound"
        );
        assert!(limits.io.is_usable(), "usable io limits");
        assert!(http::largest_read(&limits.http) <= limits.io.largest_read(), "HTTP read fits io");
        assert!(http::largest_room(&limits.http) <= limits.io.largest_room(), "HTTP write fits io");
        let _bound = worst_case(&settings);
        let queue = queue_capacity(&limits);
        Self {
            settings,
            io: io::Io::new(&limits.io),
            events: Queue::with_capacity(queue),
            requests: Queue::with_capacity(queue),
            completions: Queue::with_capacity(queue),
            submissions: Queue::with_capacity(queue),
            duties: Queue::with_capacity(limits.duties),
            reports: Queue::with_capacity(report_capacity(&limits)),
            connections: Vec::with_capacity(
                usize::try_from(limits.exchanges).expect("count fits").checked_add(1).expect("stream slot"),
            ),
            flights: Vec::with_capacity(usize::try_from(limits.exchanges).expect("count fits")),
            jar: cookie::Jar::new(limits.cookies, limits.cookie),
            serial: 0,
            closing: false,
        }
    }
    /// Queues a duty between turns. Held bytes include owned container capacities.
    pub fn duty(&mut self, duty: Duty) {
        assert!(!self.closing, "no duties after the world's end");
        assert!(self.duties.room() > 0, "duties within the bound between turns");
        let limits = &self.settings.limits;
        match &duty {
            Duty::Exchange(exchange) => {
                assert!(exchange.bound <= limits.answer, "answer within its bound");
                assert!(
                    exchange_bytes(exchange) <= usize::try_from(limits.request).expect("request fits"),
                    "request within its bound"
                );
                assert!(!self.flights.contains(&exchange.id), "an exchange id is not in flight");
                assert!(
                    self.flights.len() < usize::try_from(limits.exchanges).expect("count fits"),
                    "exchanges within the in-flight bound"
                );
                self.flights.push(exchange.id);
            }
            Duty::Abandon { exchange } => self.forget(*exchange),
            Duty::Stream(StreamDuty::Open { path, headers, bound }) => {
                assert!(*bound > 0 && *bound <= limits.piece, "stream piece within its bound");
                let held = path
                    .len()
                    .checked_add(
                        headers
                            .capacity()
                            .checked_mul(size_of::<skein_ui_web::transport::Header>())
                            .expect("headers fit"),
                    )
                    .and_then(|held| {
                        headers.iter().try_fold(held, |held, (name, value)| {
                            held.checked_add(name.len())?.checked_add(value.len())
                        })
                    })
                    .expect("held bytes fit");
                assert!(
                    held <= usize::try_from(limits.request).expect("request fits"),
                    "stream request within its bound"
                );
            }
            Duty::Stream(StreamDuty::Close) | Duty::ReadOn => {}
        }
        self.duties.push(duty);
    }
    /// The next report, in the order made; the loop drains them after a slice.
    pub fn report(&mut self) -> Option<Report> {
        self.reports.pop()
    }
    /// Ends the world: closes all connections and suppresses all further reports.
    pub fn close(&mut self) {
        self.closing = true;
    }
    fn forget(&mut self, id: u32) {
        if let Some(at) = self.flights.iter().position(|held| *held == id) {
            self.flights.remove(at);
        }
    }
    fn remove(&mut self, at: usize) {
        let connection = self.connections.remove(at);
        if let Kind::Exchange { id, .. } = connection.kind {
            self.forget(id);
        }
        if let Some(entity) = connection.socket {
            self.requests.push(io::Request::Close { entity });
        }
    }
    fn stream(&self) -> Option<usize> {
        self.connections.iter().position(|connection| matches!(connection.kind, Kind::Stream { .. }))
    }
    fn end_stream(&mut self) {
        if let Some(at) = self.stream() {
            self.reports.push(Report::StreamEnded);
            self.remove(at);
        }
    }
    fn apply(&mut self, duty: Duty, wall: Wall) {
        match duty {
            Duty::Exchange(exchange) => {
                let Exchange { id, path, method, headers, body, bound } = exchange;
                let kind = Kind::Exchange {
                    id,
                    bound,
                    received: Vec::with_capacity(usize::try_from(bound).expect("answer fits")),
                };
                self.open(
                    kind,
                    path,
                    &method,
                    headers
                        .into_iter()
                        .map(|(name, value)| Header {
                            name: name.into_bytes().into(),
                            value: value.into_bytes().into(),
                        })
                        .collect(),
                    body,
                    wall,
                );
            }
            Duty::Abandon { exchange } => {
                if let Some(at) = self
                    .connections
                    .iter()
                    .position(|connection| matches!(connection.kind, Kind::Exchange { id, .. } if id == exchange))
                {
                    // The id may already have a new reservation in a later queued duty.
                    let connection = self.connections.remove(at);
                    if let Some(entity) = connection.socket {
                        self.requests.push(io::Request::Close { entity });
                    }
                }
            }
            Duty::Stream(StreamDuty::Open { path, headers, bound }) => {
                self.end_stream();
                self.open(
                    Kind::Stream { bound, opened: false, reading: false },
                    path,
                    "GET",
                    headers.into_iter().map(|(name, value)| Header { name, value }).collect(),
                    Box::from([]),
                    wall,
                );
            }
            Duty::Stream(StreamDuty::Close) => self.end_stream(),
            Duty::ReadOn => {
                if let Some(at) = self.stream() {
                    self.connections[at].read_on(&self.settings.limits);
                }
            }
        }
    }
    #[expect(clippy::too_many_arguments, reason = "the owned request parts cross the connection admission together")]
    fn open(
        &mut self,
        kind: Kind,
        path: Box<[u8]>,
        method: &str,
        mut headers: Vec<Header>,
        body: Box<[u8]>,
        wall: Wall,
    ) {
        let method = Method::from_bytes(method.as_bytes());
        // Browser fetch constrains its target to the page's origin.
        if method.is_none() || !path.starts_with(b"/") || path.starts_with(b"//") || path.contains(&b'\\') {
            match kind {
                Kind::Exchange { id, .. } => {
                    self.forget(id);
                    self.reports.push(Report::Failed { exchange: id });
                }
                Kind::Stream { .. } => self.reports.push(Report::StreamEnded),
            }
            return;
        }
        let method = method.expect("supported method");
        headers.push(Header { name: Box::from(b"Host".as_slice()), value: self.settings.host.clone() });
        if method != Method::Get && method != Method::Head {
            headers.push(Header { name: Box::from(b"Origin".as_slice()), value: self.settings.origin.clone() });
        }
        if let Some(value) = self.jar.header(&path, wall) {
            headers.push(Header { name: Box::from(b"Cookie".as_slice()), value });
        }
        let call = http::Call {
            method,
            target: path,
            headers: headers.into_boxed_slice(),
            body: if body.is_empty() {
                http::Body::None
            } else {
                http::Body::Length(u64::try_from(body.len()).expect("body length"))
            },
            close: true,
        };
        self.serial = self.serial.checked_add(1).expect("connection tickets fit");
        let owner = Token::new(self.serial);
        self.connections.push(Connection::new(owner, kind, call, body, &self.settings.limits));
        self.requests.push(io::Request::Connect { owner, addr: io::Addr::Ip(self.settings.listener) });
    }
    fn event(&mut self, event: io::Event, now: Time, wall: Wall) {
        match event {
            io::Event::Connecting { owner, socket } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.socket = Some(socket);
                } else {
                    self.requests.push(io::Request::Close { entity: socket });
                }
            }
            io::Event::Connected { owner } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.connected = true;
                }
            }
            io::Event::Stream { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.up(up, now, wall, &self.settings.limits);
                }
            }
            io::Event::Failed { owner, .. } | io::Event::Closed { owner } => {
                if let Some(at) = self.connections.iter().position(|connection| connection.owner == owner) {
                    self.reports.push(self.connections[at].failed());
                    self.remove(at);
                }
            }
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Output { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => unreachable!("the visitor only connects and carries byte streams"),
        }
    }
}
impl Host for Visitor {
    fn iterate(&mut self, now: Time, wall: Wall) {
        let env = Env { now, wall, limits: self.settings.limits.io };
        if self.closing {
            while self.duties.pop().is_some() {}
            while self.reports.pop().is_some() {}
            self.flights.clear();
            self.jar.clear();
            while !self.connections.is_empty() {
                self.remove(0);
            }
        } else {
            // Inputs precede completions: an abandon wins over an answer in this turn.
            for _ in 0..self.settings.limits.duties {
                if self.requests.room() < 4 {
                    break;
                }
                let Some(duty) = self.duties.pop() else { break };
                self.apply(duty, wall);
            }
        }
        if self.events.room() >= io::MAX_OUT_RESUME.events && self.submissions.room() >= io::MAX_OUT_UP.submissions {
            if self.io.is_ready() {
                io::resume(&mut self.io, &env, &mut self.events, &mut self.submissions);
            } else if let Some(complete) = self.completions.pop() {
                io::up(&mut self.io, &env, complete, &mut self.events, &mut self.submissions);
            } else if self.io.is_due(now) {
                io::fire(&mut self.io, &env, &mut self.events, &mut self.submissions);
            }
        }
        // Reserve the HTTP entrance before taking a stream event from io.
        if self.requests.room() >= 4
            && self.connections.iter().all(Connection::takes)
            && let Some(event) = self.events.pop()
        {
            self.event(event, now, wall);
        }
        let mut at = 0;
        while at < self.connections.len() && self.requests.room() >= connection::ROOM + 1 {
            if self.connections[at].progress(
                now,
                wall,
                &self.settings.limits,
                &mut self.jar,
                &mut self.reports,
                &mut self.requests,
            ) {
                self.remove(at);
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
            || !self.duties.is_empty()
            || self.connections.iter().any(Connection::has_work)
            || (self.closing && (!self.connections.is_empty() || !self.reports.is_empty()))
    }
    fn next_deadline(&self) -> Option<Time> {
        self.io.next_deadline()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        None
    }
    fn is_empty(&self) -> bool {
        self.closing
            && self.io.is_empty()
            && self.connections.is_empty()
            && self.duties.is_empty()
            && self.reports.is_empty()
            && self.events.is_empty()
            && self.requests.is_empty()
            && self.completions.is_empty()
            && self.submissions.is_empty()
    }
    fn worst_case(&self) -> u64 {
        worst_case(&self.settings)
    }
    fn operations(&self) -> u32 {
        io::operations(&self.settings.limits.io).expect("io operations fit")
    }
}

fn exchange_bytes(exchange: &Exchange) -> usize {
    exchange
        .method
        .capacity()
        .checked_add(exchange.path.len())
        .and_then(|bytes| bytes.checked_add(exchange.body.len()))
        .and_then(|bytes| bytes.checked_add(exchange.headers.capacity().checked_mul(size_of::<(String, String)>())?))
        .and_then(|bytes| {
            exchange.headers.iter().try_fold(bytes, |bytes, (name, value)| {
                bytes.checked_add(name.capacity())?.checked_add(value.capacity())
            })
        })
        .expect("held request bytes fit")
}
fn queue_capacity(limits: &Limits) -> u32 {
    // Enough for a full io pass and each active client's bounded outputs.
    limits
        .io
        .sockets
        .checked_add(limits.io.refusals)
        .and_then(|count| count.checked_add(limits.exchanges))
        .and_then(|count| count.checked_add(limits.duties))
        .and_then(|count| count.checked_add(2))
        .and_then(|count| count.checked_mul(16))
        .expect("visitor queues fit")
}
fn report_capacity(limits: &Limits) -> u32 {
    limits
        .exchanges
        .checked_add(limits.duties.checked_mul(2).expect("duty reports fit"))
        .and_then(|count| count.checked_add(2))
        .expect("report count fits")
}

/// The most heap held by the visitor, including inputs and outputs awaiting the loop.
///
/// Reports are drained after each slice, whose inputs enter before stepping:
/// `exchanges + 2 * duties + 2` slots cover the old exchanges, up to two
/// reports per entered duty, and a pre-existing stream's piece and terminal.
/// The report queue asserts that contract. Each slot may own at most
/// `max(answer, piece)` bytes. Every client and inter-layer queue is counted,
/// including one extra body byte to detect an answer past its bound, request
/// copies during header construction, and the jar's Path bytes from HTTP heads.
#[must_use]
pub fn worst_case(settings: &Settings) -> u64 {
    let limits = &settings.limits;
    let queue = queue_capacity(limits);
    let clients = u64::from(limits.exchanges) + 1;
    let cookie_header =
        u64::from(limits.cookies).checked_mul(u64::from(limits.cookie) + 3).expect("cookie header fits");
    let client = http::worst_case(&limits.http)
        .expect("usable HTTP limits")
        .checked_add(u64::try_from(size_of::<Connection>()).expect("size fits"))
        .and_then(|bytes| bytes.checked_add(Queue::<http::Event>::worst_case(connection::ROOM)?))
        .and_then(|bytes| bytes.checked_add(Queue::<http::Request>::worst_case(connection::ROOM)?))
        .and_then(|bytes| bytes.checked_add(Queue::<skein_lib::stream::Down>::worst_case(connection::ROOM)?))
        .and_then(|bytes| bytes.checked_add(u64::from(limits.request).checked_mul(8)?))
        .and_then(|bytes| bytes.checked_add(cookie_header.checked_mul(4)?))
        .and_then(|bytes| bytes.checked_add(u64::from(limits.answer).checked_mul(2)?))
        // Queue payloads include heads and their header descriptors, upload pieces,
        // received pieces and encoded request heads. HTTP owns its own intake.
        .and_then(|bytes| {
            bytes.checked_add(
                u64::from(connection::ROOM).checked_mul(
                    u64::from(limits.http.head)
                        .checked_mul(4)?
                        .checked_add(
                            u64::from(limits.http.headers).checked_mul(u64::try_from(size_of::<Header>()).ok()?)?,
                        )?
                        .checked_add(u64::from(limits.http.read))?
                        .checked_add(u64::from(limits.http.send))?
                        .checked_add(u64::from(limits.http.request))?,
                )?,
            )
        })
        .expect("client heap fits");
    io::worst_case(&limits.io)
        .expect("io heap fits")
        .checked_add(Queue::<io::Event>::worst_case(queue).expect("queue fits"))
        .and_then(|bytes| bytes.checked_add(Queue::<io::Request>::worst_case(queue)?))
        .and_then(|bytes| bytes.checked_add(Queue::<kernel::Complete>::worst_case(queue)?))
        .and_then(|bytes| bytes.checked_add(Queue::<kernel::Submit>::worst_case(queue)?))
        .and_then(|bytes| {
            bytes.checked_add(
                u64::from(queue)
                    .checked_mul(u64::from(limits.io.receive.max(limits.io.intake).max(limits.io.output)))?,
            )
        })
        .and_then(|bytes| bytes.checked_add(Queue::<Duty>::worst_case(limits.duties)?))
        .and_then(|bytes| bytes.checked_add(u64::from(limits.duties).checked_mul(u64::from(limits.request))?))
        .and_then(|bytes| bytes.checked_add(Queue::<Report>::worst_case(report_capacity(limits))?))
        .and_then(|bytes| {
            bytes.checked_add(
                u64::from(report_capacity(limits)).checked_mul(u64::from(limits.answer.max(limits.piece)))?,
            )
        })
        .and_then(|bytes| bytes.checked_add(clients.checked_mul(client)?))
        .and_then(|bytes| bytes.checked_add(u64::from(limits.exchanges).checked_mul(4)?))
        .and_then(|bytes| bytes.checked_add(cookie::worst_case(limits.cookies, limits.cookie, limits.http.head)))
        .and_then(|bytes| bytes.checked_add(u64::try_from(settings.host.len()).ok()?))
        .and_then(|bytes| bytes.checked_add(u64::try_from(settings.origin.len()).ok()?))
        .expect("visitor heap fits")
}

#[cfg(test)]
mod tests;
