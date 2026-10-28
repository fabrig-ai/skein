//! Startup and a single drive loop on the HTTP and io machines (examples.md, 9.3).
use skein_http::server::{self, Server};
use skein_http::sse::writer as sse;
use skein_io::kernel::{Complete, Exit, Submit};
use skein_io::{self as io, Addr, Io, Request};
use skein_lib::stream::{self, Read};
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use skein_shell::{Backend, Clock, Config, Host, Kernel, drive};
use skein_ui_example_server::{State, bundle::Bundle, routes};
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
const LISTENER: Token = Token::new(1);
const CONNECTIONS: usize = 32;
const QUEUE: u32 = 1024;
const STREAMS: usize = 8;
const EVENT_BYTES: u32 = 8192;
fn sse_limits() -> sse::Limits {
    sse::Limits { event: EVENT_BYTES, chunk: 4096 }
}
fn io_limits() -> io::Limits {
    io::Limits {
        sockets: 40,
        refusals: 1,
        intake: 8192,
        receive: 4096,
        output: 32768,
        sends: 8,
        accepts: 4,
        backlog: 32,
        close_timeout: Duration::from_secs(2),
        names: 0,
        retry: Duration::from_millis(50),
    }
}
fn http_limits() -> server::Limits {
    server::Limits { head: 4096, headers: 32, body: 8192, read: 4096, response: 2048, send: 16384 }
}
fn main() -> ExitCode {
    skein_fail_stop::install();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("skein-ui-example: {why}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let address: SocketAddr = args
        .next()
        .ok_or("usage: skein-ui-example ADDRESS --bundle DIR")?
        .parse()
        .map_err(|why| format!("invalid address: {why}"))?;
    let mut directory = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bundle" => directory = Some(PathBuf::from(args.next().ok_or("--bundle needs a directory")?)),
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    let directory = directory.ok_or("--bundle needs a directory")?;
    let bundle = Bundle::read(&directory)?;
    let signals = skein_shell::open_termination_signals().map_err(|why| format!("cannot open signals: {why:?}"))?;
    let operations = io::operations(&io_limits()).ok_or("ring bound overflow")?;
    let mut kernel = Kernel::open(Config { operations }).map_err(|why| why.to_string())?;
    let chosen = kernel.chosen();
    if chosen.backend != Backend::Ring {
        eprintln!("skein-ui-example: kernel: {chosen}");
    }
    let clock = Clock::new();
    let skein_shell::Now { now, wall } = clock.now();
    let mut host = Web::new(address, bundle, now, wall);
    host.signals = Some(host.io.adopt_signals(signals).map_err(|_| "no signal slot")?);
    let exit = drive(&mut kernel, &clock, &mut host);
    match exit {
        Exit::Code(0) => Ok(()),
        other @ (Exit::Code(_) | Exit::Signal(_)) => Err(format!("server ended: {other:?}")),
    }
}
#[derive(Debug)]
struct Connection {
    socket: Token,
    server: Server,
    reply: Box<[u8]>,
    call: Option<server::Call>,
    body: Vec<u8>,
    writer: Option<sse::Writer>,
    events: VecDeque<Box<[u8]>>,
    sent: usize,
    deadline: Time,
}
#[derive(Debug)]
struct Web {
    io: Io,
    io_env: Env<io::Limits>,
    env: Env<server::Limits>,
    completions: Queue<Complete>,
    submissions: Queue<Submit>,
    events: Queue<io::Event>,
    requests: Queue<Request>,
    above: Queue<server::Event>,
    below: Queue<stream::Down>,
    connections: BTreeMap<Token, Connection>,
    next_owner: u64,
    state: State,
    bundle: Bundle,
    listener: Option<Token>,
    signals: Option<Token>,
    address: Option<SocketAddr>,
    told: bool,
    stopping: bool,
    failed: bool,
}
impl Web {
    fn new(address: SocketAddr, bundle: Bundle, now: Time, wall: Wall) -> Self {
        let mut requests = Queue::with_capacity(QUEUE);
        requests.push(Request::Listen { owner: LISTENER, addr: Addr::Ip(address) });
        Self {
            io: Io::new(&io_limits()),
            io_env: Env { now, wall, limits: io_limits() },
            env: Env { now, wall, limits: http_limits() },
            completions: Queue::with_capacity(QUEUE),
            submissions: Queue::with_capacity(QUEUE),
            events: Queue::with_capacity(QUEUE),
            requests,
            above: Queue::with_capacity(32),
            below: Queue::with_capacity(32),
            connections: BTreeMap::new(),
            next_owner: 2,
            state: State::new(),
            bundle,
            listener: None,
            signals: None,
            address: None,
            told: false,
            stopping: false,
            failed: false,
        }
    }
    fn stop(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        if let Some(entity) = self.listener {
            self.requests.push(Request::Close { entity });
        }
        if let Some(entity) = self.signals.take() {
            self.requests.push(Request::Close { entity });
        }
        for conn in self.connections.values() {
            self.requests.push(Request::Abort { entity: conn.socket });
        }
    }
    fn event(&mut self, event: io::Event) {
        match event {
            io::Event::Listening { listener, addr, .. } => {
                self.listener = Some(listener);
                self.address = addr.socket();
                if self.stopping {
                    self.requests.push(Request::Close { entity: listener });
                }
            }
            io::Event::Accepted { socket, .. } => {
                if self.stopping || self.connections.len() >= CONNECTIONS {
                    self.requests.push(Request::Reject { socket });
                    return;
                }
                let owner = Token::new(self.next_owner);
                self.next_owner = self.next_owner.checked_add(1).expect("connection tokens cannot exhaust");
                self.connections.insert(
                    owner,
                    Connection {
                        socket,
                        server: Server::new(&self.env.limits),
                        reply: Box::from([]),
                        call: None,
                        body: Vec::new(),
                        writer: None,
                        events: VecDeque::new(),
                        sent: 0,
                        deadline: self.env.now.checked_add(Duration::from_secs(30)).expect("deadline fits"),
                    },
                );
                self.requests.push(Request::Bind { socket, owner });
                self.down(owner, server::Request::Next);
            }
            io::Event::Stream { owner, up } => {
                if let Some(conn) = self.connections.get_mut(&owner) {
                    server::up(&mut conn.server, &self.env, up, &mut self.above, &mut self.below);
                    self.drain_connection(owner);
                }
            }
            io::Event::Closed { owner } => {
                if owner == LISTENER {
                    self.listener = None;
                } else {
                    self.connections.remove(&owner);
                }
            }
            io::Event::Shutdown { .. } => self.stop(),
            io::Event::Failed { owner, .. } => {
                if owner == LISTENER {
                    self.failed = true;
                    self.stop();
                }
            }
            io::Event::Output { .. }
            | io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. } => unreachable!("server owns only sockets and termination signals"),
        }
    }
    fn down(&mut self, owner: Token, request: server::Request) {
        let conn = self.connections.get_mut(&owner).expect("live connection");
        server::down(&mut conn.server, &self.env, request, &mut self.above, &mut self.below);
        self.drain_connection(owner);
    }
    fn drain_connection(&mut self, owner: Token) {
        let socket = self.connections.get(&owner).expect("live connection").socket;
        while let Some(down) = self.below.pop() {
            self.requests.push(Request::Stream { stream: socket, down });
        }
        while let Some(event) = self.above.pop() {
            match event {
                server::Event::Call(call) => {
                    let conn = self.connections.get_mut(&owner).expect("live connection");
                    conn.call = Some(call);
                    self.read_body(owner);
                }
                server::Event::Body(stream::Up::Bytes(bytes)) => {
                    self.connections.get_mut(&owner).expect("live connection").body.extend_from_slice(&bytes);
                    self.read_body(owner);
                }
                server::Event::Body(stream::Up::End) => self.respond(owner),
                server::Event::Body(stream::Up::Room) => unreachable!("body reads only"),
                server::Event::Reply(stream::Up::Room) => {
                    if self.connections.get(&owner).expect("live connection").writer.is_some() {
                        self.sse_up(owner, stream::Up::Room);
                        continue;
                    }
                    let conn = self.connections.get_mut(&owner).expect("live connection");
                    let end = conn
                        .sent
                        .checked_add(usize::try_from(self.env.limits.send).expect("send fits"))
                        .expect("bounded reply")
                        .min(conn.reply.len());
                    let bytes = Box::from(conn.reply.get(conn.sent..end).expect("sent stays within reply"));
                    conn.sent = end;
                    let finished = end == conn.reply.len();
                    self.down(owner, server::Request::Reply(stream::Down::Send(bytes)));
                    if finished {
                        self.down(owner, server::Request::Reply(stream::Down::Finish));
                    } else {
                        self.ask_reply(owner);
                    }
                }
                server::Event::Body(stream::Up::Failed(_))
                | server::Event::Done(_)
                | server::Event::Ended
                | server::Event::Failed(_) => {
                    self.down(owner, server::Request::Close);
                    self.requests.push(Request::Close { entity: socket });
                }
                server::Event::Reply(stream::Up::Failed(fault)) => {
                    if self.connections.get(&owner).expect("live connection").writer.is_some() {
                        self.sse_up(owner, stream::Up::Failed(fault));
                    }
                }
                server::Event::Closed => {}
                server::Event::Reply(stream::Up::Bytes(_) | stream::Up::End) => unreachable!("reply writes only"),
                server::Event::Refused(why) => unreachable!("bounded response is valid: {why:?}"),
            }
        }
    }
    fn read_body(&mut self, owner: Token) {
        self.down(
            owner,
            server::Request::Body(stream::Down::Demand { read: Read::Arrived { max: self.env.limits.read }, room: 0 }),
        );
    }
    fn respond(&mut self, owner: Token) {
        let streams = self.connections.values().filter(|conn| conn.writer.is_some()).count();
        let conn = self.connections.get_mut(&owner).expect("live connection");
        let call = conn.call.take().expect("request head before body");
        let opening = call.method == skein_http::Method::Get && call.target.as_ref() == b"/api/events";
        let answer = if opening && streams >= STREAMS {
            routes::stream_refused()
        } else {
            routes::route(&mut self.state, &self.bundle, &call, &conn.body)
        };
        conn.body.clear();
        conn.reply = answer.body;
        conn.sent = 0;
        if opening && answer.head.status == 200 {
            conn.writer = Some(sse::Writer::new(&sse_limits()));
            conn.deadline = Time::from_nanos(u64::MAX);
            self.down(owner, server::Request::Respond(answer.head));
            self.sse_down(owner, sse::Request::Comment(Box::from([])));
        } else {
            self.down(owner, server::Request::Respond(answer.head));
            self.ask_reply(owner);
        }
        if let Some(note) = self.state.take_made() {
            self.broadcast(&note);
        }
    }
    fn broadcast(&mut self, note: &[u8]) {
        use skein_lib::{
            Writer,
            base64::{self, Alphabet},
        };
        let len = base64::encoded_len(Alphabet::Standard, u32::try_from(note.len()).expect("note bounded"))
            .expect("event fits");
        let mut writer = Writer::new(usize::try_from(len).expect("event fits"));
        base64::encode(Alphabet::Standard, note, &mut writer).expect("event measured");
        let data = writer.finish();
        let owners: Vec<_> =
            self.connections.iter().filter_map(|(owner, conn)| conn.writer.as_ref().map(|_| *owner)).collect();
        for owner in owners {
            let conn = self.connections.get_mut(&owner).expect("live stream");
            assert!(conn.events.len() < 64, "at most 64 new notes per server");
            conn.events.push_back(data.clone());
            self.pump_stream(owner);
        }
    }
    fn pump_stream(&mut self, owner: Token) {
        let conn = self.connections.get_mut(&owner).expect("live stream");
        if conn.writer.as_ref().expect("stream writer").waiting() == sse::Waiting::Above
            && let Some(data) = conn.events.pop_front()
        {
            self.sse_down(
                owner,
                sse::Request::Event(sse::Outgoing { name: Box::from([]), data, id: None, retry: None }),
            );
        }
    }
    fn sse_down(&mut self, owner: Token, request: sse::Request) {
        let mut above = Queue::with_capacity(4);
        let mut below = Queue::with_capacity(4);
        let env = Env { now: self.env.now, wall: self.env.wall, limits: sse_limits() };
        let conn = self.connections.get_mut(&owner).expect("live stream");
        sse::down(conn.writer.as_mut().expect("stream writer"), &env, request, &mut above, &mut below);
        self.drain_sse(owner, &mut above, &mut below);
    }
    fn sse_up(&mut self, owner: Token, up: stream::Up) {
        let mut above = Queue::with_capacity(4);
        let mut below = Queue::with_capacity(4);
        let env = Env { now: self.env.now, wall: self.env.wall, limits: sse_limits() };
        let conn = self.connections.get_mut(&owner).expect("live stream");
        sse::up(conn.writer.as_mut().expect("stream writer"), &env, up, &mut above, &mut below);
        self.drain_sse(owner, &mut above, &mut below);
    }
    fn drain_sse(&mut self, owner: Token, above: &mut Queue<sse::Event>, below: &mut Queue<stream::Down>) {
        while let Some(down) = below.pop() {
            self.down(owner, server::Request::Reply(down));
        }
        while let Some(event) = above.pop() {
            match event {
                sse::Event::Sent => self.pump_stream(owner),
                sse::Event::Refused(_) | sse::Event::Failed(_) => {
                    let socket = self.connections.get(&owner).expect("live stream").socket;
                    self.requests.push(Request::Abort { entity: socket });
                }
                sse::Event::Closed => {}
            }
        }
    }
    fn ask_reply(&mut self, owner: Token) {
        let conn = self.connections.get(&owner).expect("live connection");
        let remaining = conn.reply.len().checked_sub(conn.sent).expect("sent stays within reply");
        if remaining > 0 {
            self.down(
                owner,
                server::Request::Reply(stream::Down::Demand {
                    read: Read::Nothing,
                    room: u32::try_from(remaining).expect("reply fits u32").min(self.env.limits.send),
                }),
            );
        }
    }
}
impl Host for Web {
    fn iterate(&mut self, now: Time, wall: Wall) {
        self.io_env.now = now;
        self.io_env.wall = wall;
        self.env.now = now;
        self.env.wall = wall;
        for _ in 0..QUEUE {
            if self.submissions.room() < 4 || self.events.room() < 4 {
                break;
            }
            if self.io.is_ready() {
                io::resume(&mut self.io, &self.io_env, &mut self.events, &mut self.submissions);
            } else if let Some(complete) = self.completions.pop() {
                io::up(&mut self.io, &self.io_env, complete, &mut self.events, &mut self.submissions);
            } else if self.io.is_due(now) {
                io::fire(&mut self.io, &self.io_env, &mut self.events, &mut self.submissions);
            } else {
                break;
            }
            while let Some(event) = self.events.pop() {
                self.event(event);
            }
        }
        for conn in self.connections.values_mut() {
            if conn.deadline <= now {
                self.requests.push(Request::Abort { entity: conn.socket });
                conn.deadline = Time::from_nanos(u64::MAX);
            }
        }
        for _ in 0..QUEUE {
            if self.submissions.room() < 4 {
                break;
            }
            let Some(request) = self.requests.pop() else {
                break;
            };
            io::down(&mut self.io, &self.io_env, request, &mut self.submissions);
        }
        self.io.reclaim();
    }
    fn completions(&mut self) -> &mut Queue<Complete> {
        &mut self.completions
    }
    fn submissions(&mut self) -> &mut Queue<Submit> {
        &mut self.submissions
    }
    fn work_pending(&self, now: Time) -> bool {
        self.io.is_ready() || self.io.is_due(now) || !self.completions.is_empty() || !self.requests.is_empty()
    }
    fn next_deadline(&self) -> Option<Time> {
        self.io.next_deadline().into_iter().chain(self.next_policy_deadline()).min()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        self.connections.values().map(|conn| conn.deadline).min()
    }
    fn is_empty(&self) -> bool {
        self.io.is_empty() && self.requests.is_empty() && self.completions.is_empty() && self.submissions.is_empty()
    }
    fn exit(&self) -> Option<Exit> {
        Some(Exit::Code(u8::from(self.failed)))
    }
    fn operations(&self) -> u32 {
        io::operations(&io_limits()).expect("startup priced operations")
    }
    fn drain(&mut self) {
        if !self.told
            && let Some(address) = self.address
        {
            eprintln!("skein-ui-example: listening at {address}");
            self.told = true;
        }
    }
}
