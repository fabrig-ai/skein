//! Real Chromium in one service loop with a skein-http page server.

#[path = "real/dialogs.rs"]
mod dialogs;
#[path = "real/downloads.rs"]
mod downloads;
#[path = "real/look.rs"]
mod look;
#[path = "real/scope.rs"]
mod scope;
use skein_fake_person_browser::{
    BrowserFace, origins as serve,
    person::{Doing, Face, Find, Found},
};
use skein_ui::{Act, Role};
#[path = "../../src/pages.rs"]
mod pages;

use std::collections::VecDeque;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use skein_browser::boundary::{Below, Down, Event, Expect, Key, Match, Point, Query, Refusal, Request, Trouble};
use skein_browser::{self as browser, Browser};
use skein_io::kernel::{self, Complete, Done, Op, Pipe, Spawn, Submit, Way};
use skein_io::{self as io, Io};
use skein_lib::stream;
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};
use skein_shell::{Clock, Config, Kernel, Wait, open_root};

const PROCESS: Token = Token::new(10);
const BROWSER: Token = Token::new(11);
const PERSON: Token = Token::new(12);
const PAGE: Token = Token::new(13);
const FIND: Token = Token::new(14);
const PRESS: Token = Token::new(15);
const EXPECT: Token = Token::new(16);
const KEY: Token = Token::new(17);
const IMAGE: Token = Token::new(18);
const SNAPSHOT: Token = Token::new(19);
const ROOT_CLOSE: Token = Token::new(u64::MAX);

#[derive(Debug)]
pub struct Profile(pub PathBuf);

impl Profile {
    pub fn fresh() -> Profile {
        let mut path = std::env::temp_dir();
        let nonce = Clock::new().now().now.as_nanos();
        path.push(format!("skein-browser-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).expect("fresh Chromium profile directory");
        Profile(path)
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

pub fn chromium() -> PathBuf {
    if let Some(configured) = std::env::var_os("SKEIN_TEST_CHROMIUM") {
        let path = PathBuf::from(configured);
        assert!(path.is_file(), "SKEIN_TEST_CHROMIUM does not name a file: {}", path.display());
        return path.canonicalize().expect("resolve configured Chromium path");
    }
    let path = std::env::var_os("PATH").expect("PATH locates chromium");
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join("chromium");
        if candidate.is_file() {
            return candidate.canonicalize().expect("resolve Chromium on PATH");
        }
    }
    panic!("Chromium is required for the browser suite: set SKEIN_TEST_CHROMIUM or put chromium on PATH");
}

pub fn fonts() -> PathBuf {
    let fallback = PathBuf::from("/usr/share/fonts/truetype/dejavu");
    let configured = std::env::var_os("SKEIN_TEST_FONTS").map(PathBuf::from);
    let directory = configured.as_ref().unwrap_or(&fallback);
    assert!(
        directory.join("DejaVuSans.ttf").is_file(),
        "the browser suite needs DejaVuSans.ttf in SKEIN_TEST_FONTS ({configured:?}) or, unset, in {} \
         (Debian's fonts-dejavu-core)",
        fallback.display()
    );
    directory.clone()
}

fn spawn(chromium: &Path, profile: &Path) -> (Spawn, kernel::Fd) {
    let root = open_root(Path::new("/")).expect("open the child working directory root");
    let command = browser::command::command(chromium.as_os_str().as_bytes(), profile.as_os_str().as_bytes());
    let mut env = Vec::new();
    for variable in &command.env {
        let mut entry = variable.name.to_vec();
        entry.push(b'=');
        entry.extend_from_slice(&variable.value);
        env.push(entry.into_boxed_slice());
    }
    let mut pipes = Vec::new();
    for spec in command.pipes {
        let way = match spec.direction {
            browser::command::Direction::Read => Way::In,
            browser::command::Direction::Write => Way::Out,
        };
        pipes.push(Pipe { child: spec.descriptor, way, parent: None });
    }
    (
        Spawn {
            program: command.program,
            args: command.args.into_boxed(),
            env: env.into_boxed_slice(),
            root,
            dir: Box::from(&b"."[..]),
            pipes: pipes.into_boxed_slice(),
        },
        root,
    )
}

fn io_limits(browser: &browser::Limits) -> io::Limits {
    io::Limits {
        sockets: 8,
        refusals: 8,
        intake: browser.message.checked_add(1).expect("message with NUL fits u32"),
        receive: 65_536,
        output: browser.command.checked_add(1).expect("command with NUL fits u32"),
        sends: 16,
        accepts: 1,
        backlog: 1,
        close_timeout: Duration::from_secs(2),
        names: 0,
        retry: Duration::from_millis(10),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Starting,
    Context,
    Opening,
    Finding,
    Reloading,
    Pressing,
    Typing,
    Keying,
    Expecting,
    Capturing,
    Closing,
    Complete,
}

#[derive(Clone, Copy, Debug)]
pub enum Scenario {
    Button,
    Form,
    Covered,
    Scroll,
    Throw,
    Csp,
    Words,
    Figure,
    Facts,
    Choose { name: &'static [u8] },
    Select,
    Dates,
    Reduction,
    ScopeReload,
    Look,
    Attribute { selector: &'static [u8], name: &'static [u8], expected: Option<&'static [u8]> },
    Leave,
    Confirm,
    Download { allow: bool },
    Letter,
}

impl Scenario {
    fn path(self) -> &'static str {
        match self {
            Scenario::Leave | Scenario::Confirm => "/leave",
            Scenario::Download { .. } => "/download",
            Scenario::Letter => "/letter",
            Scenario::Button => "/button",
            Scenario::Form => "/form",
            Scenario::Covered => "/covered",
            Scenario::Scroll => "/scroll",
            Scenario::Throw => "/throw",
            Scenario::Csp => "/csp",
            Scenario::Words => "/words",
            Scenario::Figure => "/figure",
            Scenario::Facts
            | Scenario::Choose { .. }
            | Scenario::Select
            | Scenario::Dates
            | Scenario::Reduction
            | Scenario::Look
            | Scenario::ScopeReload
            | Scenario::Attribute { .. } => "/facts",
        }
    }
}

pub struct Rig {
    browser: Browser,
    pages: serve::Pages,
    io: Io,
    kernel: Kernel,
    clock: Clock,
    browser_env: Env<browser::Limits>,
    io_env: Env<io::Limits>,
    complete: Queue<Complete>,
    submits: Queue<Submit>,
    io_events: Queue<io::Event>,
    io_requests: VecDeque<io::Request>,
    events: Queue<Event>,
    browser_down: Queue<Down>,
    pipes: Option<[Token; 3]>,
    child: Option<Token>,
    root: kernel::Fd,
    pub root_closed: bool,
    pub exited: bool,
    pub child_closed: bool,
    stage: Stage,
    scenario: Scenario,
    context_ready: bool,
    exception_seen: bool,
    csp_log_seen: bool,
    await_met: bool,
    pub version: Box<[u8]>,
    stderr: Vec<u8>,
    pub snapshot: Box<[u8]>,
    pub tree: Option<skein_lib::List<skein_browser::boundary::Entry>>,
    dialog: dialogs::State,
    scope: Option<u64>,
    choice: Option<BrowserFace<'static>>,
    pub choice_failure: Option<String>,
    pub download_directory: PathBuf,
    pub downloaded: Option<(Box<[u8]>, PathBuf)>,
    pub download_trouble: bool,
    form_node: u64,
    look: look::State,
}

impl Rig {
    pub fn new(chromium: &Path, profile: &Path, scenario: Scenario) -> Rig {
        let browser_limits = browser::Limits::default();
        let limits = io_limits(&browser_limits);
        assert!(browser::largest_read(&browser_limits).expect("read size") <= limits.largest_read());
        assert!(browser::largest_room(&browser_limits).expect("room size") <= limits.largest_room());
        let operations = io::operations(&limits).expect("ring size fits u32");
        let kernel = Kernel::open(Config { operations })
            .unwrap_or_else(|error| panic!("io_uring is required for the browser suite: {error}"));
        let (spawn, root) = spawn(chromium, profile);
        let mut io_requests = VecDeque::new();
        io_requests.push_back(io::Request::Spawn { owner: PROCESS, spawn });
        Rig {
            browser: Browser::new(BROWSER, &browser_limits),
            pages: serve::Pages::new(pages::pages(), Time::ZERO, Wall::EPOCH),
            io: Io::new(&limits),
            kernel,
            clock: Clock::new(),
            browser_env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: browser_limits },
            io_env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            complete: Queue::with_capacity(256),
            submits: Queue::with_capacity(256),
            io_events: Queue::uncapped(),
            io_requests,
            events: Queue::uncapped(),
            browser_down: Queue::uncapped(),
            pipes: None,
            child: None,
            root,
            root_closed: false,
            exited: false,
            child_closed: false,
            stage: Stage::Starting,
            scenario,
            dialog: dialogs::State::new(scenario),
            scope: None,
            choice: None,
            choice_failure: None,
            download_directory: profile.join("downloads"),
            downloaded: None,
            download_trouble: false,
            form_node: 0,
            look: look::State::default(),
            context_ready: false,
            exception_seen: false,
            csp_log_seen: false,
            await_met: false,
            version: Box::default(),
            stderr: Vec::new(),
            snapshot: Box::default(),
            tree: None,
        }
    }

    /// Read the accessibility tree as well as the flat snapshot, to check subtree order.
    pub fn capture_tree(&mut self) {
        self.tree = Some(skein_lib::List::with_capacity(0));
    }

    pub fn run(&mut self) {
        let deadline = self.clock.now().now.saturating_add(Duration::from_secs(60));
        for _ in 0..50_000_u32 {
            self.kernel.reap(&mut self.complete);
            let now = self.clock.now();
            assert!(
                now.now < deadline,
                "Chromium smoke test timed out at {:?}, stderr: {}",
                self.stage,
                String::from_utf8_lossy(&self.stderr)
            );
            self.browser_env.now = now.now;
            self.browser_env.wall = now.wall;
            self.io_env.now = now.now;
            self.io_env.wall = now.wall;
            self.pages.update_time(now.now, now.wall);
            while self.io.is_ready() {
                io::resume(&mut self.io, &self.io_env, &mut self.io_events, &mut self.submits);
            }
            while let Some(complete) = self.complete.pop() {
                if complete.op == ROOT_CLOSE {
                    assert_eq!(complete.result, Ok(Done::Nothing), "close the process root descriptor");
                } else {
                    io::up(&mut self.io, &self.io_env, complete, &mut self.io_events, &mut self.submits);
                }
            }
            while self.io.is_due(now.now) {
                io::fire(&mut self.io, &self.io_env, &mut self.io_events, &mut self.submits);
            }
            while let Some(event) = self.io_events.pop() {
                self.io_event(event);
            }
            self.browser_steps(now.now);
            for _ in 0..self.events.len() {
                let event = self.events.pop().expect("this browser stage's events");
                self.browser_event(event);
            }
            self.drain_browser_down();
            while self.io.takes() {
                let Some(request) = self.io_requests.pop_front().or_else(|| self.pages.take()) else { break };
                io::down(&mut self.io, &self.io_env, request, &mut self.submits);
            }
            self.io.reclaim();
            if self.stage == Stage::Complete
                && self.child_closed
                && self.pages.is_closed()
                && self.io.is_empty()
                && self.kernel.in_flight() == 0
            {
                return;
            }
            let pending = self.io.is_ready()
                || !self.complete.is_empty()
                || !self.io_events.is_empty()
                || !self.events.is_empty()
                || !self.browser_down.is_empty()
                || !self.io_requests.is_empty()
                || self.pages.pending()
                || self.browser.work_pending();
            let wait = if pending {
                Wait::No
            } else {
                let mut until = deadline;
                if let Some(next) = self.io.next_deadline() {
                    until = until.min(next);
                }
                if let Some(next) = self.browser.next_deadline() {
                    until = until.min(next);
                }
                Wait::Until(until)
            };
            self.kernel.submit(&mut self.submits, wait);
        }
        panic!("Chromium smoke test did not settle at {:?}", self.stage);
    }

    pub fn fixed_page(&mut self, body: Vec<u8>) {
        self.pages.pages.push(serve::Page {
            path: b"/facts".as_slice().into(),
            kind: b"text/html; charset=utf-8".as_slice().into(),
            body: body.into(),
            headers: vec![],
        });
    }

    fn browser_steps(&mut self, now: Time) {
        if self.pipes.is_none() {
            return;
        }
        for _ in 0..32_u32 {
            let due = self.browser.next_deadline().is_some_and(|at| at <= now);
            if !self.browser.work_pending() && !due {
                break;
            }
            browser::fire(&mut self.browser, &self.browser_env, now, &mut self.events, &mut self.browser_down);
        }
    }

    fn io_event(&mut self, event: io::Event) {
        if matches!(
            &event,
            io::Event::Listening { owner: serve::OWNER, .. } | io::Event::Accepted { owner: serve::OWNER, .. }
        ) || matches!(&event, io::Event::Stream { owner, .. } | io::Event::Closed { owner } | io::Event::Failed { owner, .. } if self.pages.owns(*owner))
        {
            self.pages.event(event);
            self.open_if_ready();
            return;
        }
        match event {
            io::Event::Spawned { owner: PROCESS, child, pipes } => {
                let [commands, replies, errors] = pipes.as_ref() else {
                    panic!("Chromium needs pipes 3, 4 and stderr")
                };
                self.pipes = Some([*commands, *replies, *errors]);
                self.child = Some(child);
                self.submits.push(Submit { op: ROOT_CLOSE, kind: Op::Close { fd: self.root } });
                self.root_closed = true;
            }
            io::Event::Spawned { owner, .. } => panic!("unexpected spawned owner {owner:?}"),
            io::Event::Output { .. } => panic!("Chromium uses only classic output"),
            io::Event::Stream { owner, up } => {
                let [commands, replies, errors] = self.pipes.expect("pipe event after spawn");
                if owner == errors
                    && let stream::Up::Bytes(bytes) = &up
                {
                    self.stderr.extend_from_slice(bytes);
                    if self.stderr.len() > 32_768 {
                        let excess = self.stderr.len() - 32_768;
                        self.stderr.drain(..excess);
                    }
                }
                let context_reply = owner == replies
                    && match &up {
                        stream::Up::Bytes(bytes) => {
                            bytes.windows(b"browserContextId".len()).any(|part| part == b"browserContextId")
                        }
                        stream::Up::Room | stream::Up::End | stream::Up::Failed(_) => false,
                    };
                let from = if owner == commands {
                    Below::Commands(up)
                } else if owner == replies {
                    Below::Replies(up)
                } else if owner == errors {
                    Below::Errors(up)
                } else {
                    panic!("unknown Chromium pipe {owner:?}")
                };
                browser::up(&mut self.browser, &self.browser_env, from, &mut self.events, &mut self.browser_down);
                if context_reply && self.stage == Stage::Context {
                    self.context_ready = true;
                    self.open_if_ready();
                }
            }
            io::Event::Exited { owner: PROCESS, exit } => {
                assert_eq!(
                    exit,
                    kernel::Exit::Code(0),
                    "Chromium exited with {exit:?}; stderr: {}",
                    String::from_utf8_lossy(&self.stderr)
                );
                self.exited = true;
                for pipe in self.pipes.expect("spawned pipes") {
                    self.io_requests.push_back(io::Request::Close { entity: pipe });
                }
            }
            io::Event::Closed { owner: PROCESS } => self.child_closed = true,
            io::Event::Failed { owner: PROCESS, error } => panic!("Chromium spawn failed: {error:?}"),
            io::Event::Closed { .. } => {}
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. }
            | io::Event::Failed { .. } => panic!("unexpected io event: {event:?}"),
        }
    }

    fn open_if_ready(&mut self) {
        if self.stage != Stage::Context || !self.context_ready {
            return;
        }
        let Some(addr) = self.pages.addr else { return };
        self.stage = Stage::Opening;
        let url = if matches!(self.scenario, Scenario::Look) {
            "about:blank".into()
        } else {
            format!("http://{addr}{}", self.scenario.path())
        };
        self.ask(Request::Page { person: PERSON, page: PAGE, url: url.into_bytes().into_boxed_slice() });
    }

    fn ask(&mut self, request: Request) {
        browser::down(&mut self.browser, &self.browser_env, request, &mut self.events, &mut self.browser_down);
    }

    fn choice_event(&mut self, event: &Event) {
        let Scenario::Choose { name } = self.scenario else { unreachable!("choice scenario") };
        let face = self.choice.as_mut().expect("choice face active");
        face.event(event);
        if self.stage == Stage::Finding {
            match face.find(&Find::exactly_named(Role::Option, name)) {
                Found::Pending => {}
                Found::Absent => panic!("option {} absent", String::from_utf8_lossy(name)),
                Found::Node(node) => {
                    face.perform(Doing::Act { node, act: Act::Press }).expect("press option");
                    self.stage = Stage::Typing;
                }
            }
        }
        let done = self.stage == Stage::Typing && !face.pending();
        let requests: Vec<_> = face.requests().collect();
        self.choice_failure = face.failure();
        for request in requests {
            self.ask(request);
        }
        if done {
            self.choice = None;
            self.stage = Stage::Capturing;
            self.ask(Request::Snapshot { page: PAGE, op: SNAPSHOT });
        }
    }

    fn browser_event(&mut self, event: Event) {
        if self.choice.is_some() {
            self.choice_event(&event);
            return;
        }
        let event = if matches!(self.scenario, Scenario::Download { .. } | Scenario::Letter) {
            match self.download_event(event) {
                Ok(()) => return,
                Err(event) => event,
            }
        } else {
            event
        };
        let event = if matches!(self.scenario, Scenario::Look) {
            match self.look_event(event) {
                Ok(()) => return,
                Err(event) => event,
            }
        } else {
            event
        };
        let event = if let Scenario::Attribute { selector, name, expected } = self.scenario {
            match event {
                Event::Opened { page } => {
                    assert_eq!(page, PAGE);
                    self.stage = Stage::Finding;
                    self.ask(Request::Attribute { page: PAGE, op: FIND, selector: selector.into(), name: name.into() });
                    return;
                }
                Event::Attribute { op, read } => {
                    assert_eq!(op, FIND);
                    let expected = expected.map_or(skein_browser::boundary::Read::NoElement, |bytes| {
                        skein_browser::boundary::Read::Value(bytes.into())
                    });
                    assert_eq!(read, expected);
                    self.close_browser();
                    return;
                }
                event => event,
            }
        } else {
            event
        };
        let event = if matches!(self.scenario, Scenario::ScopeReload) {
            match self.scope_event(event) {
                Ok(()) => return,
                Err(event) => event,
            }
        } else {
            event
        };
        let event = if matches!(self.scenario, Scenario::Leave | Scenario::Confirm) {
            match self.dialog_event(event) {
                Ok(()) => return,
                Err(event) => event,
            }
        } else {
            event
        };
        match event {
            Event::Ready { version } => {
                assert_eq!(self.stage, Stage::Starting);
                assert!(!version.is_empty(), "Chromium gives its version");
                self.version = version;
                self.stage = Stage::Context;
                let downloads = if matches!(self.scenario, Scenario::Download { allow: true }) {
                    std::fs::create_dir(&self.download_directory).expect("fresh download directory");
                    Some(self.download_directory.as_os_str().as_bytes().into())
                } else {
                    None
                };
                self.ask(Request::Person { person: PERSON, downloads });
            }
            Event::Opened { page } => {
                assert_eq!(page, PAGE);
                assert_eq!(self.stage, Stage::Opening);
                self.stage = Stage::Finding;
                match self.scenario {
                    Scenario::Download { .. }
                    | Scenario::Letter
                    | Scenario::Look
                    | Scenario::Leave
                    | Scenario::Confirm
                    | Scenario::ScopeReload
                    | Scenario::Attribute { .. } => {
                        unreachable!("dialog scenario handled above")
                    }
                    Scenario::Reduction => {
                        self.stage = Stage::Expecting;
                        self.ask(Request::Await {
                            page: PAGE,
                            op: EXPECT,
                            query: query(b"heading", b"Reloaded"),
                            expect: Expect::Present,
                            within: Duration::from_secs(2),
                        });
                    }
                    Scenario::Facts => {
                        self.stage = Stage::Capturing;
                        self.ask(Request::Snapshot { page: PAGE, op: SNAPSHOT });
                    }
                    Scenario::Dates => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"Date", b"case-entry_date") });
                    }
                    Scenario::Choose { .. } => {
                        self.choice = Some(BrowserFace::new(PAGE, ""));
                        self.choice_event(&event);
                    }
                    Scenario::Select => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"combobox", b"Pick") });
                    }
                    Scenario::Button => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"button", b"Press") })
                    }
                    Scenario::Form => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"textbox", b"Name") })
                    }
                    Scenario::Covered => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"button", b"Covered") })
                    }
                    Scenario::Scroll => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"button", b"Far away") })
                    }
                    Scenario::Throw => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"button", b"Throw") })
                    }
                    Scenario::Csp => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"heading", b"CSP page") })
                    }
                    Scenario::Words => {
                        let mut words = query(b"StaticText", b"3 failing");
                        words.matching = Match::Words;
                        self.ask(Request::Find { page: PAGE, op: FIND, query: words });
                    }
                    Scenario::Figure => {
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"figure", b"Error rate") });
                    }
                }
            }
            Event::Found { op, seen, more } if matches!(self.scenario, Scenario::Dates) => {
                assert_eq!(op, FIND);
                assert_eq!(more, 0);
                assert_eq!(seen.len(), 1);
                let node = seen.get(0).expect("native field").node;
                let text = match self.stage {
                    Stage::Finding => {
                        self.stage = Stage::Typing;
                        b"10152026".as_slice()
                    }
                    Stage::Keying => {
                        self.stage = Stage::Expecting;
                        b"0230P".as_slice()
                    }
                    stage => panic!("unexpected native field at {stage:?}"),
                };
                self.ask(Request::Keys { page: PAGE, op: PRESS, node, text: text.into() });
            }
            Event::Found { op, seen, more } if matches!(self.scenario, Scenario::Form) => {
                assert_eq!((op, seen.len(), more), (FIND, 1, 0));
                let field = seen.get(0).expect("form field");
                self.form_node = field.node;
                match self.stage {
                    Stage::Finding => {
                        assert_eq!(field.value.as_ref(), b"Old words");
                        self.stage = Stage::Pressing;
                        self.ask(Request::Type { page: PAGE, op: PRESS, node: field.node, text: Box::new([]) });
                    }
                    Stage::Capturing => {
                        assert_eq!(field.value.as_ref(), b"Ada");
                        assert!(field.states.focused);
                        self.stage = Stage::Keying;
                        self.ask(Request::Key { page: PAGE, op: KEY, key: Key::Enter, shift: false });
                    }
                    stage => panic!("unexpected form find at {stage:?}"),
                }
            }
            Event::Found { op, seen, more } => {
                assert_eq!(op, FIND);
                assert_eq!(self.stage, Stage::Finding);
                assert_eq!(more, 0);
                assert_eq!(seen.len(), 1, "one accessible target for {:?}", self.scenario);
                let node = seen.get(0).expect("target").node;
                match self.scenario {
                    Scenario::Download { .. }
                    | Scenario::Letter
                    | Scenario::Look
                    | Scenario::Leave
                    | Scenario::Confirm
                    | Scenario::ScopeReload
                    | Scenario::Attribute { .. } => {
                        unreachable!("dialog scenario handled above")
                    }
                    Scenario::Choose { .. } => unreachable!("choice handled by the face"),
                    Scenario::Select => {
                        self.stage = Stage::Pressing;
                        self.ask(Request::Press { page: PAGE, op: PRESS, node, at: None });
                    }
                    Scenario::Reduction | Scenario::Facts | Scenario::Dates => panic!("facts handled separately"),
                    Scenario::Form => {
                        self.stage = Stage::Typing;
                        self.ask(Request::Type { page: PAGE, op: PRESS, node, text: Box::from(&b"Ada"[..]) });
                    }
                    Scenario::Csp => {
                        self.stage = Stage::Expecting;
                        self.ask(Request::Await {
                            page: PAGE,
                            op: EXPECT,
                            query: query(b"heading", b"Wrong"),
                            expect: Expect::Absent,
                            within: Duration::from_millis(500),
                        });
                    }
                    Scenario::Words => {
                        assert_eq!(seen.get(0).expect("sentence").name.as_ref(), b"Checks: 3 failing, 2 passed");
                        self.close_browser();
                    }
                    Scenario::Figure => {
                        self.stage = Stage::Pressing;
                        self.ask(Request::Press { page: PAGE, op: PRESS, node, at: Some(Point { x: 30, y: 25 }) });
                    }
                    Scenario::Button | Scenario::Covered | Scenario::Scroll | Scenario::Throw => {
                        self.stage = Stage::Pressing;
                        self.ask(Request::Press { page: PAGE, op: PRESS, node, at: None });
                    }
                }
            }
            Event::Done { op } if matches!(self.scenario, Scenario::Form) && self.stage != Stage::Keying => {
                match self.stage {
                    Stage::Pressing => {
                        assert_eq!(op, PRESS);
                        self.stage = Stage::Reloading;
                        self.ask(Request::Key { page: PAGE, op: KEY, key: Key::SelectAll, shift: false });
                    }
                    Stage::Reloading => {
                        assert_eq!(op, KEY);
                        self.stage = Stage::Typing;
                        self.ask(Request::Type {
                            page: PAGE,
                            op: PRESS,
                            node: self.form_node,
                            text: b"Ada".as_slice().into(),
                        });
                    }
                    Stage::Typing => {
                        assert_eq!(op, PRESS);
                        self.stage = Stage::Capturing;
                        self.ask(Request::Find { page: PAGE, op: FIND, query: query(b"textbox", b"Name") });
                    }
                    stage => panic!("unexpected form done at {stage:?}"),
                }
            }
            Event::Done { op } if matches!(self.scenario, Scenario::Dates) => {
                assert_eq!(op, PRESS);
                match self.stage {
                    Stage::Typing => {
                        self.stage = Stage::Keying;
                        self.ask(Request::Find {
                            page: PAGE,
                            op: FIND,
                            query: query(b"InputTime", b"case-entry_time"),
                        });
                    }
                    Stage::Expecting => {
                        self.stage = Stage::Capturing;
                        self.ask(Request::Snapshot { page: PAGE, op: SNAPSHOT });
                    }
                    stage => panic!("unexpected native entry Done at {stage:?}"),
                }
            }
            Event::Done { op } if matches!(self.scenario, Scenario::Select) => {
                assert_eq!((self.stage, op), (Stage::Pressing, PRESS));
                self.close_browser();
            }
            Event::Done { op } => match self.stage {
                Stage::Typing => {
                    assert_eq!(op, PRESS);
                    self.stage = Stage::Keying;
                    self.ask(Request::Key { page: PAGE, op: KEY, key: Key::Enter, shift: false });
                }
                Stage::Keying | Stage::Pressing => {
                    assert_eq!(op, if self.stage == Stage::Keying { KEY } else { PRESS });
                    self.stage = Stage::Expecting;
                    let heading = match self.scenario {
                        Scenario::Button => b"Done".as_slice(),
                        Scenario::Form => b"Hello Ada".as_slice(),
                        Scenario::Scroll => b"Scrolled".as_slice(),
                        Scenario::Throw => b"Before".as_slice(),
                        Scenario::Figure => b"Pressed shape".as_slice(),
                        Scenario::Leave
                        | Scenario::Confirm
                        | Scenario::Covered
                        | Scenario::Csp
                        | Scenario::Words
                        | Scenario::Facts
                        | Scenario::Choose { .. }
                        | Scenario::Select
                        | Scenario::Dates
                        | Scenario::Reduction
                        | Scenario::Download { .. }
                        | Scenario::Letter
                        | Scenario::Look
                        | Scenario::ScopeReload
                        | Scenario::Attribute { .. } => {
                            panic!("no action should complete for {:?}", self.scenario)
                        }
                    };
                    self.ask(Request::Await {
                        page: PAGE,
                        op: EXPECT,
                        query: query(b"heading", heading),
                        expect: Expect::Present,
                        within: Duration::from_secs(3),
                    });
                }
                _ => panic!("unexpected Done {op:?} at {:?}", self.stage),
            },
            Event::Met { op, seen } => {
                assert_eq!(op, EXPECT);
                assert_eq!(self.stage, Stage::Expecting);
                assert_eq!(seen.len(), if matches!(self.scenario, Scenario::Csp) { 0 } else { 1 });
                if matches!(self.scenario, Scenario::Throw | Scenario::Csp | Scenario::Reduction) {
                    self.await_met = true;
                    if (matches!(self.scenario, Scenario::Throw | Scenario::Reduction) && self.exception_seen)
                        || (matches!(self.scenario, Scenario::Csp) && self.csp_log_seen)
                    {
                        self.close_browser();
                    }
                } else if matches!(self.scenario, Scenario::Button | Scenario::Form) {
                    self.stage = Stage::Capturing;
                    self.ask(if matches!(self.scenario, Scenario::Button) {
                        Request::Screenshot { page: PAGE, op: IMAGE }
                    } else {
                        Request::Snapshot { page: PAGE, op: SNAPSHOT }
                    });
                } else {
                    self.close_browser();
                }
            }
            Event::Screenshot { op: IMAGE, png } => {
                assert_eq!(self.stage, Stage::Capturing);
                assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
                self.close_browser();
            }
            Event::Snapshot { op: SNAPSHOT, text } => {
                assert_eq!(self.stage, Stage::Capturing);
                if matches!(self.scenario, Scenario::Form) {
                    assert!(text.windows(b"Hello Ada".len()).any(|part| part == b"Hello Ada"));
                }
                self.snapshot = text;
                if self.tree.is_some() {
                    self.ask(Request::Look { page: PAGE, op: SNAPSHOT, look: skein_browser::boundary::Look::Tree });
                } else {
                    self.close_browser();
                }
            }
            Event::Looked { op: SNAPSHOT, looked: skein_browser::boundary::Looked::Tree(entries) } => {
                self.tree = Some(entries);
                self.close_browser();
            }
            Event::Closed { owner: BROWSER } => {
                assert_eq!(self.stage, Stage::Closing);
                self.stage = Stage::Complete;
                self.pages.stop();
            }
            Event::Closed { owner } if owner == PAGE || owner == PERSON => {}
            Event::Trouble { trouble: Trouble::Log, .. } if matches!(self.scenario, Scenario::Csp) => {
                self.csp_log_seen = true;
                if self.await_met && self.stage == Stage::Expecting {
                    self.close_browser();
                }
            }
            Event::Trouble { trouble: Trouble::Exception, text, .. }
                if matches!(self.scenario, Scenario::Throw | Scenario::Reduction) =>
            {
                assert!(!text.is_empty(), "exception has a message");
                self.exception_seen = true;
                if self.await_met && self.stage == Stage::Expecting {
                    self.close_browser();
                }
            }
            Event::Trouble { trouble, text, .. } => {
                panic!("unexpected page trouble {trouble:?}: {}", String::from_utf8_lossy(&text))
            }
            Event::Refused { op, why } => {
                if matches!(self.scenario, Scenario::Covered)
                    && self.stage == Stage::Pressing
                    && op == PRESS
                    && why == Refusal::Covered
                {
                    self.close_browser();
                    return;
                }
                panic!("browser refused operation {op:?}: {why:?}; stderr: {}", String::from_utf8_lossy(&self.stderr))
            }
            Event::Missed { op, seen, more } => panic!("expectation {op:?} missed; saw {seen:?}, {more} more"),
            other => panic!("unexpected browser event: {other:?}"),
        }
    }

    fn close_browser(&mut self) {
        self.stage = Stage::Closing;
        self.ask(Request::Close { entity: BROWSER });
    }

    fn drain_browser_down(&mut self) {
        for _ in 0..self.browser_down.len() {
            let down = self.browser_down.pop().expect("this browser stage's requests");
            let [commands, replies, errors] = self.pipes.expect("browser output follows spawn");
            let (pipe, down) = match down {
                Down::Commands(down) => (commands, down),
                Down::Replies(down) => (replies, down),
                Down::Errors(down) => (errors, down),
            };
            self.io_requests.push_back(io::Request::Stream { stream: pipe, down });
        }
    }
}

fn query(role: &[u8], name: &[u8]) -> Query {
    Query { role: Box::from(role), name: Box::from(name), matching: Match::Exact, within: None, boxes: false }
}
