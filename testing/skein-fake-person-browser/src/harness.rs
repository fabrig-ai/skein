//! The server and Chromium are children of one io loop (ui-web.md, 14.2).
use crate::BrowserFace;
use browser::boundary::{Below, Down, Event, Request};
use skein_browser::{self as browser, Browser};
use skein_fake_peers::Listening;
use skein_fake_person::Next;
use skein_io::{
    self as io, Io,
    kernel::{self, Complete, Op, Pipe, Spawn, Submit, Way},
};
use skein_lib::{Duration, Env, Queue, Time, Token, Wall, stream};
use skein_shell::{Clock, Wait, open_root};
use skein_world::ring::Ring;
use std::fmt::Write as _;
use std::{collections::VecDeque, ffi::OsString, os::unix::ffi::OsStrExt, path::PathBuf};

const POLL: Duration = Duration::from_millis(10);
pub(crate) const SERVER: Token = Token::new(10);
pub(crate) const PROCESS: Token = Token::new(11);
pub(crate) const BROWSER: Token = Token::new(12);
const SNAPSHOT: Token = Token::new(u64::MAX - 3);
const IMAGE: Token = Token::new(u64::MAX - 2);
const ROOT: Token = Token::new(u64::MAX - 1);
const SERVER_ROOT: Token = Token::new(u64::MAX);

/// A peer borrowed for this run; its observations remain the test's.
pub struct Lent<'a> {
    pub name: &'a [u8],
    pub peer: &'a mut dyn Listening,
}

#[derive(Clone, Copy)]
pub struct Printed<'a> {
    pub sent: Sent,
    pub prefix: &'a [u8],
    pub on: Output,
    pub names: Names,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sent {
    Page,
    Command,
}
pub struct Command {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub stdin: Stdin,
    pub opens: Option<Opens>,
    pub within: Duration,
}
pub enum Stdin {
    Closed,
    Printed,
}
pub struct Opens {
    pub prefix: Box<[u8]>,
    pub on: Output,
    pub keep: Box<[u8]>,
}
pub(crate) enum CommandAction {
    Run(Command),
    Exit { code: i32, within: Duration },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Output {
    Stdout,
    Stderr,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Names {
    Address,
    Url,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Opened {
    pub url: String,
    pub address: String,
}

pub use crate::origins::{Origin, Page};
pub struct Setup<'a> {
    pub server: PathBuf,
    pub args: Vec<OsString>,
    pub printed: Printed<'a>,
    pub restart: Vec<OsString>,
    pub chromium: PathBuf,
    pub downloads: Option<PathBuf>,
    pub expected: Vec<Box<[u8]>>,
    pub origins: Vec<Origin>,
}
pub struct Asked {
    pub method: skein_http::Method,
    pub path: Box<[u8]>,
    pub host: Box<[u8]>,
    pub headers: Vec<skein_ui_web::transport::Header>,
    pub body: Box<[u8]>,
}
/// The request's failure at the io or HTTP boundary. These boundaries expose
/// typed errors, not the kernel's original errno.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestFailure {
    Io(io::Error),
    Http(skein_http::client::Error),
}
pub trait Play {
    fn next(&mut self, face: &mut BrowserFace<'_>, now: Time) -> Next<Box<[u8]>>;
    /// Current zero-based script position, for diagnostics.
    fn expected(&self) -> Vec<Box<[u8]>> {
        Vec::new()
    }
    fn request(&mut self) -> Option<Asked> {
        None
    }
    fn answered(&mut self, _status: u16) {}
    /// A request ended without a response head. The default preserves the
    /// status-only face; scripts retain the error in their failure report.
    fn request_failed(&mut self, _failure: RequestFailure) {
        self.answered(0);
    }
    fn at(&self) -> u32 {
        0
    }
    fn check_deadline(&mut self, _face: &mut BrowserFace<'_>, _now: Time) {}
    fn server_action(&mut self) -> Option<ServerAction> {
        None
    }
}
mod commands;
mod request;
mod script;
pub use script::{Script, Step};
#[derive(Clone, Copy)]
pub enum ServerAction {
    Stop,
    Start,
}
#[derive(PartialEq, Eq, Debug)]
pub enum Outcome {
    Done,
    Failed { person: u32, step: u32, report: String },
}
#[derive(Debug, PartialEq, Eq)]
pub struct Kept {
    pub person: u32,
    pub name: Box<[u8]>,
    pub value: Box<[u8]>,
}
pub struct Harness {
    kept: Vec<Kept>,
    server: PathBuf,
    args: Vec<OsString>,
    sent: Sent,
    prefix: Box<[u8]>,
    output: Output,
    names: Names,
    restart: Vec<OsString>,
    chromium: PathBuf,
    downloads: Option<PathBuf>,
    expected: Vec<Box<[u8]>>,
    origins: Vec<Origin>,
}
impl Harness {
    #[must_use]
    pub fn new(setup: Setup<'_>) -> Self {
        Self {
            kept: Vec::new(),
            server: setup.server,
            args: setup.args,
            sent: setup.printed.sent,
            prefix: setup.printed.prefix.into(),
            output: setup.printed.on,
            names: setup.printed.names,
            restart: setup.restart,
            chromium: setup.chromium,
            downloads: setup.downloads,
            expected: setup.expected,
            origins: setup.origins,
        }
    }
    #[must_use]
    pub fn kept(&self) -> &[Kept] {
        &self.kept
    }
    pub fn run(&mut self, people: &mut [&mut dyn Play]) -> Outcome {
        self.run_lending(people, &mut [])
    }
    pub fn run_lending(&mut self, people: &mut [&mut dyn Play], peers: &mut [Lent<'_>]) -> Outcome {
        self.kept.clear();
        let mut peers = match peers::Peers::new(peers) {
            Ok(peers) => peers,
            Err(report) => return Outcome::Failed { person: 0, step: 0, report },
        };
        let profile = skein_scratch::Scratch::new("ui-browser");
        let (outcome, kept) = Runtime::new(self, profile.path(), people, &peers.limits).run(self, people, &mut peers);
        self.kept = kept;
        outcome
    }
}
pub(crate) mod capacity;
pub(crate) mod closing;
mod lines;
mod peers;
pub use lines::printed;
pub(crate) use lines::{opening, restart, server_pipes, substitute};
#[derive(PartialEq, Eq, Debug)]
enum ServerState {
    Running,
    Stopping,
    Stopped,
    Starting,
}
struct Runtime {
    kept: Vec<Kept>,
    commands: Vec<Option<commands::Process>>,
    command_serial: u64,
    pending_commands: Vec<Option<Command>>,
    held: Vec<Option<Box<[u8]>>>,
    raw_lines: Vec<Box<[u8]>>,
    request_serial: u64,
    exchanges: Vec<request::Exchange>,
    expected: Vec<Vec<Box<[u8]>>>,
    origins: Vec<crate::origins::Pages>,
    browser: Browser,
    io: Io,
    ring: Ring,
    startup: VecDeque<io::Request>,
    peer_addresses: Vec<(Box<[u8]>, String)>,
    clock: Clock,
    benv: Env<browser::Limits>,
    ienv: Env<io::Limits>,
    complete: Queue<Complete>,
    submits: Queue<Submit>,
    events: Queue<Event>,
    io_events: Queue<io::Event>,
    down: Queue<Down>,
    requests: VecDeque<io::Request>,
    roots: [kernel::Fd; 2],
    roots_closing: [bool; 2],
    pipes: Option<[Token; 3]>,
    server_pipes: Vec<Token>,
    output: Vec<u8>,
    lines: Vec<Opened>,
    closed: u32,
    spawned: u32,
    server_state: ServerState,
    stderr: Vec<u8>,
    server_stderr: Vec<u8>,
    address: Option<String>,
    ready: bool,
    context: bool,
    opening: usize,
    opened: Vec<bool>,
    // The last request asked of the browser, for a deadline's report.
    asked: String,
    outcome: Option<Outcome>,
    capturing: u8,
    snapshot: String,
    screenshot: Option<PathBuf>,
    closing: closing::Closing,
}
impl Runtime {
    fn origins(setup: &Harness) -> Vec<crate::origins::Pages> {
        setup
            .origins
            .iter()
            .enumerate()
            .map(|(i, origin)| {
                crate::origins::Pages::numbered(
                    origin.pages.clone(),
                    Time::ZERO,
                    Wall::EPOCH,
                    u64::try_from(i).expect("origin fits"),
                )
            })
            .collect()
    }
    fn browser_spawn(command: browser::command::Command, root: kernel::Fd) -> Spawn {
        Spawn {
            program: command.program,
            args: command.args.into_boxed(),
            env: command
                .env
                .iter()
                .map(|entry| {
                    let mut bytes = entry.name.to_vec();
                    bytes.push(b'=');
                    bytes.extend_from_slice(&entry.value);
                    bytes.into()
                })
                .collect(),
            root,
            dir: b".".as_slice().into(),
            pipes: command
                .pipes
                .into_iter()
                .map(|pipe| Pipe {
                    child: pipe.descriptor,
                    way: match pipe.direction {
                        browser::command::Direction::Read => Way::In,
                        browser::command::Direction::Write => Way::Out,
                    },
                    parent: None,
                })
                .collect(),
        }
    }
    fn new(setup: &Harness, profile: &std::path::Path, people: &[&mut dyn Play], peers: &[u32]) -> Self {
        let bl = browser::Limits::default();
        let capacity = capacity::Capacity::new(
            u32::try_from(people.len()).expect("person count fits"),
            u32::try_from(setup.origins.len()).expect("origin count fits"),
            setup.output,
            &bl,
        )
        .expect("harness capacity fits");
        let il = capacity.io;
        let mut limits = vec![capacity.operations, capacity::ROOT_OPERATIONS];
        limits.extend_from_slice(peers);
        let operations =
            limits.iter().try_fold(0_u32, |total, limit| total.checked_add(*limit)).expect("shared ring capacity fits");
        let ring = Ring::new(operations, limits).expect("io_uring is required for the browser suite");
        let command = browser::command::command(setup.chromium.as_os_str().as_bytes(), profile.as_os_str().as_bytes());
        let root = open_root(std::path::Path::new("/")).expect("child root");
        let server_root = open_root(std::path::Path::new("/")).expect("server root");
        let browser_spawn = Self::browser_spawn(command, root);
        let server_spawn = Spawn {
            program: setup.server.as_os_str().as_bytes().into(),
            args: setup.args.iter().map(|arg| arg.as_bytes().into()).collect(),
            env: Box::new([]),
            root: server_root,
            dir: b".".as_slice().into(),
            pipes: server_pipes(setup.output),
        };
        Self {
            kept: Vec::new(),
            commands: (0..people.len()).map(|_| None).collect(),
            command_serial: 0,
            pending_commands: (0..people.len()).map(|_| None).collect(),
            held: vec![None; people.len()],
            raw_lines: Vec::new(),
            request_serial: 0,
            exchanges: Vec::new(),
            expected: people.iter().map(|play| play.expected()).collect(),
            origins: Self::origins(setup),
            browser: Browser::new(BROWSER, &bl),
            io: Io::new(&il),
            ring,
            startup: VecDeque::from([
                io::Request::Spawn { owner: SERVER, spawn: server_spawn },
                io::Request::Spawn { owner: PROCESS, spawn: browser_spawn },
            ]),
            peer_addresses: Vec::new(),
            clock: Clock::new(),
            benv: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: bl },
            ienv: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: il },
            complete: Queue::with_capacity(capacity.queue),
            submits: Queue::with_capacity(capacity.queue),
            events: Queue::with_capacity(capacity.browser_queue),
            io_events: Queue::with_capacity(capacity.queue),
            down: Queue::with_capacity(capacity.browser_queue),
            requests: VecDeque::new(),
            roots: [server_root, root],
            roots_closing: [false; 2],
            pipes: None,
            server_pipes: Vec::new(),
            output: Vec::new(),
            lines: Vec::new(),
            closed: 0,
            spawned: 0,
            server_state: ServerState::Running,
            stderr: Vec::new(),
            server_stderr: Vec::new(),
            address: None,
            ready: false,
            context: false,
            opening: 0,
            opened: vec![false; people.len()],
            asked: String::new(),
            outcome: None,
            capturing: 0,
            snapshot: String::new(),
            screenshot: None,
            closing: closing::Closing::new(),
        }
    }
    fn ask(&mut self, request: Request) {
        self.asked = format!("{request:?}").chars().take(200).collect();
        browser::down(&mut self.browser, &self.benv, request, &mut self.events, &mut self.down);
    }
    fn read_server(&mut self, pipe: Token) {
        self.requests.push_back(io::Request::Stream {
            stream: pipe,
            down: stream::Down::Demand { read: stream::Read::Arrived { max: 32768 }, room: 0 },
        });
    }
    /// Where each person stood when the run's deadline passed.
    fn stuck(
        &self,
        people: &[&mut dyn Play],
        faces: &[BrowserFace<'_>],
        done: &[bool],
        began: bool,
        peers_empty: bool,
    ) -> String {
        let each = people.iter().enumerate().map(|(index, play)| {
            format!(
                "\nperson {index}: step {}, opened {}, done {}, pending {}",
                play.at(),
                self.opened[index],
                done[index],
                faces.get(index).is_some_and(BrowserFace::pending)
            )
        });
        let closing = format!(
            "\nclosing {}: closed {} of {} spawned, io empty {}, ring empty {}, submits empty {}, origins closed {}, \
             peers empty {peers_empty}",
            self.closing.active(),
            self.closed,
            self.spawned,
            self.io.is_empty(),
            self.ring.is_empty(),
            self.submits.is_empty(),
            self.origins.iter().all(crate::origins::Pages::is_closed),
        );
        format!(
            "browser harness deadline passed: began {began}, ready {}, server {:?}, outcome before {:?}, last request {}{}{closing}",
            self.ready,
            self.server_state,
            self.outcome,
            self.asked,
            each.collect::<String>()
        )
    }
    fn fail(&mut self, person: usize, step: u32, report: String) {
        if !matches!(self.outcome, Some(Outcome::Failed { .. })) {
            self.outcome = Some(Outcome::Failed { person: u32::try_from(person).expect("person fits"), step, report });
        }
    }
    fn origin_event(&mut self, event: io::Event) -> Option<io::Event> {
        for pages in &mut self.origins {
            let owner = match &event {
                io::Event::Listening { owner, .. }
                | io::Event::Accepted { owner, .. }
                | io::Event::Stream { owner, .. }
                | io::Event::Closed { owner }
                | io::Event::Failed { owner, .. } => Some(*owner),
                io::Event::Spawned { .. }
                | io::Event::Exited { .. }
                | io::Event::Connecting { .. }
                | io::Event::Connected { .. }
                | io::Event::Output { .. }
                | io::Event::Usage { .. }
                | io::Event::Resized
                | io::Event::Window { .. }
                | io::Event::Shutdown { .. } => None,
            };
            if owner.is_some_and(|owner| pages.owns(owner)) {
                pages.event(event);
                return None;
            }
        }
        Some(event)
    }
    fn read_output(&mut self, owner: Token, bytes: &[u8], setup: &Harness) {
        let selected = usize::from(setup.output == Output::Stdout);
        if self.server_pipes.get(selected) == Some(&owner) {
            self.output.extend_from_slice(bytes);
            self.raw_lines.extend(
                self.output
                    .split_inclusive(|byte| *byte == b'\n')
                    .filter(|line| {
                        line.ends_with(b"\n")
                            && !printed(
                                line,
                                &Printed {
                                    sent: setup.sent,
                                    prefix: &setup.prefix,
                                    on: setup.output,
                                    names: setup.names,
                                },
                            )
                            .is_empty()
                    })
                    .map(Box::from),
            );
            self.lines.extend(printed(
                &self.output,
                &Printed { sent: setup.sent, prefix: &setup.prefix, on: setup.output, names: setup.names },
            ));
            if let Some(end) = self.output.iter().rposition(|b| *b == b'\n') {
                self.output.drain(..=end);
            }
            if let Some(line) = self.lines.last() {
                for pages in &mut self.origins {
                    pages.address(&line.address);
                }
                self.address = Some(line.address.clone());
                self.server_state = ServerState::Running;
            }
        }
    }
    fn io_event(&mut self, event: io::Event, setup: &Harness) {
        if self.command_event(&event) {
            let index = self
                .commands
                .iter()
                .position(|command| {
                    command
                        .as_ref()
                        .is_some_and(|command| command.owns(event_owner(&event).expect("command event owner")))
                })
                .expect("owned command event");
            self.commands[index].as_mut().expect("command").event(event);
            return;
        }
        let Some(event) = request::route(&mut self.exchanges, event).and_then(|event| self.origin_event(event)) else {
            return;
        };
        self.closing.io_event(&event, &mut self.requests);
        match event {
            io::Event::Spawned { owner, pipes, .. } => {
                self.spawned += 1;
                if owner == SERVER {
                    self.server_pipes = pipes.into_vec();
                    for pipe in self.server_pipes.clone() {
                        self.read_server(pipe);
                    }
                } else {
                    let [commands, replies, errors] = pipes.as_ref() else { panic!("Chromium pipes") };
                    self.pipes = Some([*commands, *replies, *errors]);
                    self.close_root(1);
                }
            }
            io::Event::Stream { owner, up } => {
                if self.server_pipes.contains(&owner) {
                    if let stream::Up::Bytes(bytes) = up {
                        if self.server_pipes.first() == Some(&owner) {
                            tail(&mut self.server_stderr, &bytes);
                        }
                        self.read_output(owner, &bytes, setup);
                        self.read_server(owner);
                    }
                    return;
                }
                let Some([commands, replies, errors]) = self.pipes else { return };
                let below = if owner == commands {
                    Below::Commands(up)
                } else if owner == replies {
                    if let stream::Up::Bytes(bytes) = &up {
                        self.context |= bytes.windows(16).any(|part| part == b"browserContextId");
                    }
                    Below::Replies(up)
                } else if owner == errors {
                    if let stream::Up::Bytes(bytes) = &up {
                        tail(&mut self.stderr, bytes);
                    }
                    Below::Errors(up)
                } else {
                    return;
                };
                browser::up(&mut self.browser, &self.benv, below, &mut self.events, &mut self.down);
            }
            io::Event::Exited { owner, exit } => {
                if !(self.closing.active() || owner == SERVER && self.server_state == ServerState::Stopping) {
                    self.fail(0, 0, format!("child {owner:?} exited: {exit:?}"));
                }
                if owner == PROCESS {
                    if let Some(pipes) = self.pipes {
                        for entity in pipes {
                            self.requests.push_back(io::Request::Close { entity });
                        }
                    }
                } else {
                    for entity in &self.server_pipes {
                        self.requests.push_back(io::Request::Close { entity: *entity });
                    }
                }
            }
            io::Event::Closed { owner } => {
                if owner == PROCESS || owner == SERVER {
                    self.closed += 1;
                    if owner == SERVER {
                        self.server_state = ServerState::Stopped;
                    }
                }
            }
            io::Event::Failed { owner, error } => self.fail(0, 0, format!("io {owner:?}: {error:?}")),
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Output { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => {}
        }
    }
    fn event(&mut self, event: &Event, setup: &Harness) {
        match event {
            Event::Ready { .. } => {
                self.ready = true;
            }
            Event::Opened { page } => {
                let index = usize::try_from(page.raw() - 100).expect("page index");
                if let Some(opened) = self.opened.get_mut(index) {
                    *opened = true;
                }
                self.opening += 1;
                self.context = false;
                if self.opening < self.opened.len() {
                    self.ask(Request::Person {
                        person: person(self.opening),
                        downloads: setup.downloads.as_ref().map(|path| path.as_os_str().as_bytes().into()),
                    });
                }
            }
            Event::Trouble { page, trouble, text } => {
                let person = usize::try_from(page.raw().saturating_sub(100)).unwrap_or(0);
                if !setup.expected.iter().chain(self.expected.get(person).into_iter().flatten()).any(|expected| {
                    !expected.is_empty() && text.windows(expected.len()).any(|part| part == expected.as_ref())
                }) {
                    self.fail(
                        usize::try_from(page.raw().saturating_sub(100)).unwrap_or(0),
                        0,
                        format!("unexpected {trouble:?}: {}", String::from_utf8_lossy(text)),
                    );
                }
            }
            Event::Snapshot { op: SNAPSHOT, text } => {
                self.snapshot = String::from_utf8_lossy(text).into_owned();
                self.capturing = 2;
                self.ask(Request::Screenshot { page: self.failure_page(), op: IMAGE });
            }
            Event::Screenshot { op: IMAGE, png } => {
                let path = std::env::temp_dir().join(format!(
                    "skein-ui-failure-{}-{}.png",
                    std::process::id(),
                    self.clock.now().now.as_nanos()
                ));
                match std::fs::write(&path, png) {
                    Ok(()) => self.screenshot = Some(path),
                    Err(error) => {
                        write!(self.snapshot, "\nscreenshot failed: {error}").expect("string write");
                    }
                }
                self.capturing = 3;
            }
            Event::Refused { op, why } if *op == SNAPSHOT || *op == IMAGE => {
                write!(self.snapshot, "\ncapture refused: {why:?}").expect("string write");
                self.capturing = 3;
            }
            Event::Closed { owner } => self.closing.browser_closed(*owner, &mut self.requests),
            Event::Downloaded { .. }
            | Event::Warned { .. }
            | Event::Found { .. }
            | Event::Met { .. }
            | Event::Missed { .. }
            | Event::Done { .. }
            | Event::Refused { .. }
            | Event::Attribute { .. }
            | Event::Address { .. }
            | Event::Snapshot { .. }
            | Event::Screenshot { .. }
            | Event::Looked { .. } => {}
        }
    }
    fn failure_page(&self) -> Token {
        match &self.outcome {
            Some(Outcome::Failed { person, .. }) => page(usize::try_from(*person).expect("person fits")),
            Some(Outcome::Done) | None => page(0),
        }
    }
    fn close_root(&mut self, index: usize) {
        if !self.roots_closing[index] {
            self.roots_closing[index] = true;
            let op = if index == 0 { SERVER_ROOT } else { ROOT };
            self.submits.push(Submit { op, kind: Op::Close { fd: self.roots[index] } });
        }
    }
    fn close(&mut self) {
        let close = self.closing.begin(!self.startup.is_empty());
        for pages in &mut self.origins {
            pages.stop();
        }
        for exchange in &mut self.exchanges {
            exchange.stop();
        }
        for command in self.commands.iter_mut().flatten() {
            command.stop();
        }
        self.close_root(0);
        if let Some(close) = close {
            self.ask(close);
        } else {
            self.startup.clear();
            self.close_root(1);
        }
    }

    fn server_action(&mut self, action: ServerAction, setup: &Harness) {
        match action {
            ServerAction::Stop => {
                assert!(self.server_state == ServerState::Running, "server running before stop");
                self.server_state = ServerState::Stopping;
                self.requests.push_back(io::Request::Signal {
                    child: self.closing.server_child().expect("server is running"),
                    signal: kernel::Signal::Terminate,
                    to: kernel::Target::Child,
                });
            }
            ServerAction::Start => {
                assert!(self.server_state == ServerState::Stopped, "server stopped before start");
                self.server_state = ServerState::Starting;
                self.server_stderr.clear();
                self.output.clear();
                self.lines.clear();
                self.raw_lines.clear();
                let args =
                    restart(&setup.restart, self.address.as_deref().expect("server address"), &self.peer_addresses);
                self.requests.push_back(io::Request::Spawn {
                    owner: SERVER,
                    spawn: Spawn {
                        program: setup.server.as_os_str().as_bytes().into(),
                        args: args.into(),
                        env: Box::new([]),
                        root: self.roots[0],
                        dir: b".".as_slice().into(),
                        pipes: server_pipes(setup.output),
                    },
                });
            }
        }
    }
    fn play(
        &mut self,
        people: &mut [&mut dyn Play],
        faces: &mut [BrowserFace<'_>],
        done: &mut [bool],
        now: Time,
        setup: &Harness,
    ) {
        if matches!(self.server_state, ServerState::Stopping | ServerState::Starting) {
            return;
        }
        for exchange in &mut self.exchanges {
            exchange.answer(people[exchange.person]);
        }
        self.exchanges.retain(|exchange| !exchange.closed);
        for (index, (play, face)) in people.iter_mut().zip(faces).enumerate() {
            if !self.opened[index] || done[index] {
                continue;
            }
            if self.exchanges.iter().filter(|exchange| exchange.person == index).count() >= 2 {
                continue;
            }
            play.check_deadline(face, now);
            if let Some(why) = face.failure() {
                self.fail(index, play.at().saturating_sub(1), why);
                break;
            }
            for request in face.requests() {
                self.ask(request);
            }
            if face.pending() {
                continue;
            }
            match play.next(face, now) {
                Next::Do(doing) => {
                    if let Err(why) = face.perform(doing) {
                        self.fail(index, play.at().saturating_sub(1), why);
                        break;
                    }
                }
                Next::Wait { .. } => {}
                Next::Done => {
                    if let Some(command) = &mut self.commands[index]
                        && command.exit.is_none()
                    {
                        let name = command.name.clone();
                        command.stop();
                        self.fail(index, play.at(), format!("command {name} still running at story end"));
                    } else {
                        done[index] = true;
                    }
                }
                Next::Failed { step, why } => {
                    self.fail(index, step, format!("{why:?}"));
                    break;
                }
            }
            if let Some(action) = face.command_action() {
                self.command_action(index, action, face, play.at(), now);
            }
            if let Some(asked) = play.request() {
                self.exchanges.push(request::Exchange::new(
                    index,
                    self.request_serial,
                    asked,
                    self.address.as_ref().expect("server address"),
                    now,
                ));
                self.request_serial += 1;
            }
            if let Some(action) = play.server_action() {
                self.server_action(action, setup);
            }
            for request in face.requests() {
                self.ask(request);
            }
            if matches!(self.server_state, ServerState::Stopping | ServerState::Starting) {
                break;
            }
        }
        if self.outcome.is_none() && done.iter().all(|done| *done) {
            self.outcome = Some(Outcome::Done);
        }
    }
    fn tick(&mut self, setup: &Harness, faces: &mut [BrowserFace<'_>], now: Time) -> bool {
        let mut opened = false;
        while self.io.is_ready() {
            io::resume(&mut self.io, &self.ienv, &mut self.io_events, &mut self.submits);
        }
        while let Some(complete) = self.complete.pop() {
            if complete.op != ROOT && complete.op != SERVER_ROOT {
                io::up(&mut self.io, &self.ienv, complete, &mut self.io_events, &mut self.submits);
            }
        }
        while self.io.is_due(now) {
            io::fire(&mut self.io, &self.ienv, &mut self.io_events, &mut self.submits);
        }
        while let Some(event) = self.io_events.pop() {
            self.io_event(event, setup);
        }
        if self.pipes.is_some() {
            for _ in 0..32 {
                if !self.browser.work_pending() && self.browser.next_deadline().is_none_or(|at| at > now) {
                    break;
                }
                browser::fire(&mut self.browser, &self.benv, now, &mut self.events, &mut self.down);
            }
        }
        while let Some(event) = self.events.pop() {
            for (index, face) in faces.iter_mut().enumerate() {
                let navigating = face.navigating();
                if !matches!(&event, Event::Downloaded { person: owner, .. } if *owner != person(index)) {
                    BrowserFace::event(face, &event);
                }
                if navigating
                    && !face.navigating()
                    && let Some(command) = &mut self.commands[index]
                    && command.opening()
                    && command.error.is_none()
                {
                    if let commands::Progress::Kept { name, value } = command.advance(now, false, face.failed()) {
                        self.kept.push(Kept { person: u32::try_from(index).expect("person fits"), name, value });
                    }
                    face.command_complete();
                }
                if let Some((name, value)) = face.kept() {
                    self.kept.push(Kept { person: u32::try_from(index).expect("person fits"), name, value });
                }
            }
            if let Event::Opened { .. } = event {
                opened = true;
            }
            self.event(&event, setup);
        }
        opened
    }
    fn open_page(&mut self, setup: &Harness, faces: &mut [BrowserFace<'_>], pending: bool) -> bool {
        if self.context
            && !pending
            && self.opening < faces.len()
            && let Some(line) = opening(&self.lines, setup.names, self.opening)
        {
            let raw_index = if setup.names == Names::Address { 0 } else { self.opening };
            self.held[self.opening] = self.raw_lines.get(raw_index).cloned();
            let url = if setup.sent == Sent::Command { "about:blank" } else { &line.url };
            url.clone_into(&mut faces[self.opening].first_url);
            self.ask(Request::Page {
                person: person(self.opening),
                page: page(self.opening),
                url: url.as_bytes().into(),
            });
            return true;
        }
        false
    }
    fn faces(&self, setup: &Harness, count: usize) -> Vec<BrowserFace<'static>> {
        (0..count)
            .map(|index| {
                let mut face = BrowserFace::new(page(index), self.address.clone().expect("listening"));
                face.origins = setup
                    .origins
                    .iter()
                    .zip(&self.origins)
                    .map(|(origin, pages)| {
                        format!(
                            "http://{}:{}",
                            String::from_utf8_lossy(&origin.host),
                            pages.addr.expect("listening origin").port()
                        )
                    })
                    .collect();
                face
            })
            .collect()
    }
    fn origin_requests(&mut self, now: Time, wall: Wall) {
        for exchange in &mut self.exchanges {
            exchange.update(now, wall);
            self.requests.extend(exchange.requests.drain(..));
        }
        for pages in &mut self.origins {
            pages.update_time(now, wall);
            while let Some(request) = pages.take() {
                self.requests.push_back(request);
            }
        }
    }
    fn run(
        mut self,
        setup: &Harness,
        people: &mut [&mut dyn Play],
        peers: &mut peers::Peers<'_, '_>,
    ) -> (Outcome, Vec<Kept>) {
        let start = self.clock.now().now;
        let deadline = start.saturating_add(Duration::from_secs(40));
        // Reserve io's close window inside the run's forty-second horizon.
        let close_at = start.saturating_add(Duration::from_secs(36));
        let mut faces = Vec::new();
        let mut done = vec![false; people.len()];
        let mut began = false;
        let mut opening_page = false;
        let mut started = false;
        loop {
            self.reap(peers);
            let time = self.clock.now();
            self.benv.now = time.now;
            self.benv.wall = time.wall;
            self.ienv.now = time.now;
            self.ienv.wall = time.wall;
            let stop_peers = self.closing.active()
                && self.closed == self.spawned
                && self.io.is_empty()
                && self.requests.is_empty()
                && self.origins.iter().all(crate::origins::Pages::is_closed);
            for report in peers.drive(&mut self.ring, time.now, time.wall, stop_peers) {
                self.fail(0, 0, report);
            }
            if !started && peers.ready() && self.outcome.is_none() {
                started = true;
                self.start(setup, peers);
            }
            if time.now >= close_at {
                let report = if started {
                    self.stuck(people, &faces, &done, began, peers.empty())
                } else {
                    format!("lent peer startup deadline: {}", peers.waiting())
                };
                self.fail(0, 0, report);
                if !self.closing.active() {
                    self.close();
                }
                self.closing.force(&mut self.requests);
                if time.now >= deadline {
                    break;
                }
            }
            if self.tick(setup, &mut faces, time.now) {
                opening_page = false;
            }
            if !began && self.ready && self.address.is_some() && self.origins.iter().all(|pages| pages.addr.is_some()) {
                began = true;
                faces = self.faces(setup, people.len());
                if people.is_empty() {
                    self.outcome = Some(Outcome::Done);
                } else {
                    self.ask(Request::Person {
                        person: person(0),
                        downloads: setup.downloads.as_ref().map(|path| path.as_os_str().as_bytes().into()),
                    });
                }
            }
            if began && self.outcome.is_none() {
                opening_page |= self.open_page(setup, &mut faces, opening_page);
            }
            self.drive_commands(&mut faces, people, time.now);
            if began && self.outcome.is_none() {
                self.play(people, &mut faces, &mut done, time.now, setup);
            }
            if self.outcome.is_some() && !self.closing.active() {
                if matches!(self.outcome, Some(Outcome::Failed { .. }))
                    && self.opened.iter().any(|open| *open)
                    && self.capturing == 0
                {
                    self.capturing = 1;
                    self.ask(Request::Snapshot { page: self.failure_page(), op: SNAPSHOT });
                } else if (self.capturing == 0 && !opening_page) || self.capturing == 3 {
                    self.close();
                }
            }
            self.flush(started, time.now, time.wall);
            if self.closing.active()
                && self.closed == self.spawned
                && self.origins.iter().all(crate::origins::Pages::is_closed)
                && self.io.is_empty()
                && self.ring.is_empty()
                && self.submits.is_empty()
                && peers.empty()
            {
                break;
            }
            self.submit(peers, time.now);
        }
        self.report()
    }
    fn flush(&mut self, started: bool, now: Time, wall: Wall) {
        self.closing.advance(&mut self.requests);
        while let Some(down) = self.down.pop() {
            let [commands, replies, errors] = self.pipes.expect("browser spawned");
            let (pipe, down) = match down {
                Down::Commands(d) => (commands, d),
                Down::Replies(d) => (replies, d),
                Down::Errors(d) => (errors, d),
            };
            self.requests.push_back(io::Request::Stream { stream: pipe, down });
        }
        if started {
            self.origin_requests(now, wall);
        }
        for command in self.commands.iter_mut().flatten() {
            self.requests.extend(command.requests.drain(..));
        }
        while self.io.takes() {
            let Some(request) = self.requests.pop_front() else { break };
            io::down(&mut self.io, &self.ienv, request, &mut self.submits);
        }
        self.io.reclaim();
    }
    fn submit(&mut self, peers: &peers::Peers<'_, '_>, now: Time) {
        let pending = self.io.is_ready()
            || !self.complete.is_empty()
            || !self.io_events.is_empty()
            || !self.events.is_empty()
            || !self.down.is_empty()
            || !self.requests.is_empty()
            || self.browser.work_pending()
            || peers.pending()
            || self.ring.has_ready();
        let until = self
            .io
            .next_deadline()
            .into_iter()
            .chain(self.browser.next_deadline())
            .chain(peers.deadline())
            .chain(self.commands.iter().flatten().filter_map(|command| command.wait.as_ref().map(|wait| wait.0)))
            .fold(now.saturating_add(POLL), Time::min);
        while let Some(record) = self.submits.pop() {
            let host = usize::from(record.op == ROOT || record.op == SERVER_ROOT);
            let record = self.ring.register(host, record);
            self.ring.queue(record);
        }
        self.ring.enter(if pending { Wait::No } else { Wait::Until(until) });
    }
    fn reap(&mut self, peers: &mut peers::Peers<'_, '_>) {
        while let Some(complete) = self.ring.reap().pop() {
            let (host, complete) = self.ring.complete(complete);
            if host >= peers::FIRST
                && let Some(report) = peers.completion(host, &complete)
            {
                self.fail(0, 0, report);
            }
            self.ring.returned(host, complete);
        }
        self.ring.deliver(0, &mut self.complete);
        let mut roots = Queue::with_capacity(capacity::ROOT_OPERATIONS);
        self.ring.deliver(1, &mut roots);
        while let Some(complete) = roots.pop() {
            if let Err(error) = complete.result {
                self.fail(0, 0, format!("root close: {error:?}"));
            }
        }
    }
    fn start(&mut self, setup: &Harness, peers: &peers::Peers<'_, '_>) {
        self.peer_addresses = peers.addresses();
        for pages in &mut self.origins {
            pages.peers(&self.peer_addresses);
        }
        while let Some(mut request) = self.startup.pop_front() {
            if let io::Request::Spawn { owner: SERVER, spawn } = &mut request {
                spawn.args =
                    setup.args.iter().map(|arg| substitute(arg.as_bytes(), None, &self.peer_addresses)).collect();
            }
            self.requests.push_back(request);
        }
    }
    fn report(self) -> (Outcome, Vec<Kept>) {
        let mut outcome = self.outcome.expect("terminal outcome");
        if let Outcome::Failed { report, .. } = &mut outcome {
            write!(
                report,
                "\nSnapshot:\n{}\nScreenshot: {}\nChromium stderr:\n{}\nServer stderr:\n{}",
                self.snapshot,
                self.screenshot.as_ref().map_or_else(|| "unavailable".into(), |p| p.display().to_string()),
                String::from_utf8_lossy(&self.stderr),
                String::from_utf8_lossy(&self.server_stderr)
            )
            .expect("string write");
            for (index, command) in self.commands.iter().enumerate() {
                if let Some(command) = command {
                    write!(
                        report,
                        "\nPerson {index} command {} stderr:\n{}",
                        command.name,
                        String::from_utf8_lossy(&command.stderr)
                    )
                    .expect("string write");
                }
            }
        }
        (outcome, self.kept)
    }
}
fn page(index: usize) -> Token {
    Token::new(100 + u64::try_from(index).expect("person fits"))
}
fn person(index: usize) -> Token {
    Token::new(100_000 + u64::try_from(index).expect("person fits"))
}
fn tail(buffer: &mut Vec<u8>, bytes: &[u8]) {
    buffer.extend_from_slice(bytes);
    if buffer.len() > 32768 {
        buffer.drain(..buffer.len() - 32768);
    }
}
fn event_owner(event: &io::Event) -> Option<Token> {
    match event {
        io::Event::Listening { owner, .. }
        | io::Event::Accepted { owner, .. }
        | io::Event::Connecting { owner, .. }
        | io::Event::Connected { owner }
        | io::Event::Stream { owner, .. }
        | io::Event::Output { owner, .. }
        | io::Event::Spawned { owner, .. }
        | io::Event::Exited { owner, .. }
        | io::Event::Usage { owner, .. }
        | io::Event::Failed { owner, .. }
        | io::Event::Closed { owner } => Some(*owner),
        io::Event::Resized | io::Event::Window { .. } | io::Event::Shutdown { .. } => None,
    }
}
