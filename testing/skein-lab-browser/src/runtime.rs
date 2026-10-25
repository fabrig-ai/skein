//! Chromium, origin listeners, proxy and standard pipes on skein-world's Ring.
#[path = "hand.rs"]
pub(crate) mod hand;
use crate::{
    calls::{self, Call},
    config::Config,
    files,
    json::{self, Value, number, object, string},
    proxy::Proxy,
};
use skein_browser::{
    self as browser, Browser,
    boundary::{self as b, Event, Request},
};
use skein_fake_person_browser::origins::Pages;
use skein_io::{
    self as io, Io,
    kernel::{self, Complete, Op, Pipe, Spawn, Submit, Way},
};
use skein_lib::{Duration, Env, List, Queue, Time, Token, Wall, stream};
use skein_shell::{Clock, Wait, open_root};
use skein_world::ring::Ring;
use std::{
    collections::{BTreeMap, VecDeque},
    os::unix::ffi::OsStrExt,
    path::Path,
};
const PROCESS: Token = Token::new(u64::MAX - 1);
const BROWSER: Token = Token::new(u64::MAX - 2);
const ROOT: Token = Token::new(u64::MAX);
const POLL: Duration = Duration::from_millis(10);
// lab-browser.md, 2.4: trouble contains the page's requests alone;
// viewport shots have no scrollbar gutter, redraw the whole raster, and
// keep a viewport in one raster tile, so no tile seams.
const CAPTURE_SWITCHES: &[&str] = &[
    "--disable-background-networking",
    "--disable-component-update",
    // On-demand model manifest updates bypass --disable-component-update;
    // Chromium's blocked port stops them, and update pings are off too.
    "--component-updater=url-source=http://127.0.0.1:1/,disable-pings",
    "--disable-sync",
    "--disable-default-apps",
    "--disable-client-side-phishing-detection",
    "--disable-domain-reliability",
    "--disable-field-trial-config",
    "--no-pings",
    "--metrics-recording-only",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-search-engine-choice-screen",
    // Gaia's session metrics probe and GCM's registration have no
    // service switch; Chromium's blocked port stops their requests.
    "--gaia-url=http://127.0.0.1:1/",
    "--gcm-checkin-url=http://127.0.0.1:1/",
    "--gcm-registration-url=http://127.0.0.1:1/",
    // Search preconnects retry refused connections to the default engine;
    // network time queries and AIM eligibility are browser traffic too.
    "--disable-features=AutofillServerCommunication,CertificateTransparencyComponentUpdater,\
         MediaRouter,OptimizationHints,OptimizationHintsFetching,OptimizationTargetPrediction,\
         OptimizationGuideModelDownloading,AutofillEnableAccountWalletStorage,\
         InterestFeedContentSuggestions,PreconnectToSearch,PreconnectFromKeyedService,SearchEnginePreconnect,\
         NetworkTimeServiceQuerying,AimEnabled",
    "--hide-scrollbars",
    "--disable-partial-raster",
    "--default-tile-width=2048",
    "--default-tile-height=2048",
    "--max-untiled-layer-width=2048",
    "--max-untiled-layer-height=2048",
];
// lab-browser.md, 2.4: spellcheck downloads its dictionary once a typed
// field is checked, and no switch stops it; the fresh profile's
// preferences turn spellcheck off and name no dictionary.
const PREFERENCES: &str =
    r#"{"browser":{"enable_spellchecking":false},"spellcheck":{"dictionaries":[],"dictionary":""}}"#;
fn capture_args(kit_args: &List<Box<[u8]>>, proxy: std::net::SocketAddr) -> List<Box<[u8]>> {
    let switch_count = u32::try_from(CAPTURE_SWITCHES.len()).expect("capture switches fit u32");
    let mut args = List::with_capacity(kit_args.len() + 2 + switch_count);
    // Keep the initial URL last so launchers preserve switch order: the
    // capture's feature list supersedes the kit's shorter feature list.
    let (initial_page, switches) = kit_args.as_slice().split_last().expect("kit's initial page");
    for arg in switches {
        args.push(arg.clone()).expect("args room");
    }
    args.push(format!("--proxy-server=http://{proxy}").into_bytes().into()).expect("proxy arg room");
    args.push(Box::from(b"--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1".as_slice()))
        .expect("resolver arg room");
    for arg in CAPTURE_SWITCHES {
        args.push(arg.as_bytes().into()).expect("capture arg room");
    }
    args.push(initial_page.clone()).expect("initial page arg room");
    args
}
struct OpenPage {
    page: Token,
    person: Token,
    trouble: Vec<Value>,
}
#[expect(
    clippy::struct_excessive_bools,
    reason = "pipe readiness and independent entity terminals are tracked separately"
)]
struct Runtime {
    io: Io,
    ring: Ring,
    browser: Browser,
    clock: Clock,
    ienv: Env<io::Limits>,
    benv: Env<browser::Limits>,
    complete: Queue<Complete>,
    submits: Queue<Submit>,
    io_events: Queue<io::Event>,
    events: Queue<Event>,
    down: Queue<b::Down>,
    requests: VecDeque<io::Request>,
    answers: VecDeque<Event>,
    proxy: Proxy,
    origins: BTreeMap<String, Pages>,
    pages: BTreeMap<String, OpenPage>,
    pipes: Option<[Token; 3]>,
    input: Token,
    output: Token,
    input_events: VecDeque<stream::Up>,
    output_room: bool,
    input_ended: bool,
    root: Option<kernel::Fd>,
    root_closed: bool,
    exited: bool,
    child_closed: bool,
    closing: bool,
    serial: u64,
    listener: Option<hand::Listener>,
    signals: Option<Token>,
    interrupted: bool,
}
pub fn run(config: &Config) -> Result<(), String> {
    std::fs::create_dir_all(&config.out).map_err(|e| e.to_string())?;
    let font_path = config.scratch.join("fonts.conf");
    let fonts = config.fonts.to_str().ok_or("fonts path must be UTF-8")?;
    let scratch = config.scratch.to_str().ok_or("scratch path must be UTF-8")?;
    std::fs::create_dir_all(config.scratch.join("fontconfig")).map_err(|e| e.to_string())?;
    std::fs::write(&font_path, crate::config::fonts(fonts, scratch)).map_err(|e| e.to_string())?;
    let mut runtime = Runtime::new(false)?;
    runtime.until_proxy()?;
    runtime.spawn(config, &font_path)?;
    let ready = runtime.next_event()?;
    let Event::Ready { version } = ready else {
        return Err(format!("expected browser Ready: {ready:?}"));
    };
    runtime.write(&object([("ready", object([("browser", json::bytes(&version))]))]))?;
    while let Some(line) = runtime.line()? {
        let answer = match line.and_then(|line| calls::parse(&line)) {
            Ok(call) => runtime.call(call, config)?,
            Err(detail) => calls::refused("call", detail),
        };
        if let Value::Object(fields) = &answer
            && fields.get("refused") == Some(&string("limit"))
        {
            eprintln!("skein-lab-browser: {}", String::from_utf8_lossy(&json::encode(&answer)));
        }
        runtime.write(&answer)?;
    }
    runtime.shutdown()
}
impl Runtime {
    fn new(hand: bool) -> Result<Self, String> {
        let bl = browser::Limits { screenshot: 16 << 20, message: 24 << 20, ..browser::Limits::default() };
        if hand {
            eprintln!(
                "skein-lab-browser limits: browser={bl:?}; call=131072; call-seconds=10; origins=1; proxy-line=8192; network=64; trouble=4096"
            );
        } else {
            eprintln!(
                "skein-lab-browser limits: browser={bl:?}; call=1048576; origins=8; file=8388608; origin-bytes=67108864; origin-files=4096; proxy-line=8192; network=64"
            );
        }
        // Four child slots, two standard pipes, eight origins with sixteen connections,
        // and the proxy with sixteen connections; extra slots cover closing sockets.
        let il = io::Limits {
            sockets: 192,
            refusals: 192,
            intake: browser::largest_read(&bl).expect("read fits"),
            receive: 65536,
            output: 32 << 20,
            sends: 16,
            accepts: 4,
            backlog: 16,
            close_timeout: Duration::from_secs(2),
            names: 0,
            retry: POLL,
        };
        let operations = io::operations(&il).expect("operations fit");
        let ring = Ring::new(operations + 1, vec![operations, 1]).map_err(|e| format!("io_uring: {e:?}"))?;
        let mut io = Io::new(&il);
        let input = if hand {
            Token::new(u64::MAX - 3)
        } else {
            io.adopt_read_pipe(kernel::Fd::new(0)).map_err(|_| "adopt stdin")?
        };
        let signals = if hand {
            let fd = skein_shell::open_termination_signals().map_err(|e| format!("termination signals: {e}"))?;
            Some(io.adopt_signals(fd).map_err(|_| "adopt termination signals")?)
        } else {
            None
        };
        let output = io.adopt_write_pipe(kernel::Fd::new(1), None).map_err(|_| "adopt stdout")?;
        Ok(Self {
            io,
            ring,
            browser: Browser::new(BROWSER, &bl),
            clock: Clock::new(),
            ienv: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: il },
            benv: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: bl },
            complete: Queue::with_capacity(4096),
            submits: Queue::with_capacity(4096),
            io_events: Queue::with_capacity(4096),
            events: Queue::with_capacity(256),
            down: Queue::with_capacity(256),
            requests: VecDeque::new(),
            answers: VecDeque::new(),
            proxy: Proxy::new(),
            origins: BTreeMap::new(),
            pages: BTreeMap::new(),
            pipes: None,
            input,
            output,
            input_events: VecDeque::new(),
            output_room: false,
            input_ended: false,
            root: None,
            root_closed: false,
            exited: false,
            child_closed: false,
            closing: false,
            serial: 100,
            listener: None,
            signals,
            interrupted: false,
        })
    }
    fn fresh(&mut self) -> Token {
        self.serial += 1;
        Token::new(self.serial)
    }
    fn until_proxy(&mut self) -> Result<(), String> {
        while self.proxy.address.is_none() {
            self.pump()?;
        }
        Ok(())
    }
    fn spawn(&mut self, config: &Config, fonts: &Path) -> Result<(), String> {
        let profile = config.scratch.join("profile");
        std::fs::create_dir_all(profile.join("Default")).map_err(|e| e.to_string())?;
        std::fs::write(profile.join("Default/Preferences"), PREFERENCES).map_err(|e| e.to_string())?;
        let mut command =
            browser::command::command(config.chromium.as_os_str().as_bytes(), profile.as_os_str().as_bytes());
        command.args = capture_args(&command.args, self.proxy.address.expect("bound proxy"));
        if let Some(hand) = &config.hand {
            let mut args = command.args.into_boxed().into_vec();
            let initial = args.pop().expect("initial URL");
            let mut bypass = format!("--proxy-bypass-list=<-loopback>;{}", hand.authority);
            for peer in &hand.peers {
                bypass.push(';');
                bypass.push_str(peer);
            }
            args.push(bypass.into_bytes().into());
            args.push(initial);
            command.args = List::with_capacity(u32::try_from(args.len()).expect("args fit"));
            for arg in args {
                command.args.push(arg).expect("args room");
            }
        }
        let mut env = List::with_capacity(command.env.len() + 1);
        for variable in &command.env {
            env.push(variable.clone()).expect("env room");
        }
        env.push(browser::command::Variable {
            name: b"FONTCONFIG_FILE".as_slice().into(),
            value: fonts.as_os_str().as_bytes().into(),
        })
        .expect("font env room");
        command.env = env;
        let root = open_root(Path::new("/")).map_err(|e| format!("Chromium root: {e:?}"))?;
        self.root = Some(root);
        let spawn = Spawn {
            program: command.program,
            args: command.args.into_boxed(),
            env: command
                .env
                .iter()
                .map(|v| {
                    let mut bytes = v.name.to_vec();
                    bytes.push(b'=');
                    bytes.extend_from_slice(&v.value);
                    bytes.into_boxed_slice()
                })
                .collect(),
            root,
            dir: b".".as_slice().into(),
            pipes: command
                .pipes
                .into_iter()
                .map(|p| Pipe {
                    child: p.descriptor,
                    way: match p.direction {
                        browser::command::Direction::Read => Way::In,
                        browser::command::Direction::Write => Way::Out,
                    },
                    parent: None,
                })
                .collect(),
        };
        self.requests.push_back(io::Request::Spawn { owner: PROCESS, spawn });
        Ok(())
    }
    fn ask(&mut self, request: Request) {
        browser::down(&mut self.browser, &self.benv, request, &mut self.events, &mut self.down);
    }
    fn stream(&mut self, stream: Token, down: stream::Down) {
        self.requests.push_back(io::Request::Stream { stream, down });
    }
    fn next_event(&mut self) -> Result<Event, String> {
        loop {
            if let Some(event) = self.answers.pop_front() {
                return Ok(event);
            }
            self.pump()?;
        }
    }
    fn write(&mut self, value: &Value) -> Result<(), String> {
        let mut bytes = json::encode(value);
        bytes.push(b'\n');
        self.write_raw(bytes)
    }
    fn write_raw(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        self.output_room = false;
        self.stream(
            self.output,
            stream::Down::Demand {
                read: stream::Read::Nothing,
                room: u32::try_from(bytes.len()).map_err(|e| e.to_string())?,
            },
        );
        while !self.output_room {
            self.pump()?;
        }
        self.stream(self.output, stream::Down::Send(bytes.into()));
        Ok(())
    }
    fn line(&mut self) -> Result<Option<Result<Vec<u8>, String>>, String> {
        if self.input_ended {
            return Ok(None);
        }
        let mut line = Vec::new();
        let mut over = false;
        loop {
            self.stream(
                self.input,
                stream::Down::Demand {
                    read: stream::Read::Scan { until: stream::Delimiter::new(b"\n").expect("LF"), max: (1 << 20) + 1 },
                    room: 0,
                },
            );
            while self.input_events.is_empty() {
                self.pump()?;
            }
            match self.input_events.pop_front().expect("input event") {
                stream::Up::Bytes(bytes) => {
                    let ended = bytes.ends_with(b"\n");
                    let piece = if ended { &bytes[..bytes.len() - 1] } else { &bytes };
                    if !over && line.len() + piece.len() <= json::LINE {
                        line.extend_from_slice(piece);
                    } else {
                        over = true;
                        line.clear();
                    }
                    if ended {
                        return Ok(Some(if over { Err("call line exceeds 1 MiB".into()) } else { Ok(line) }));
                    }
                }
                stream::Up::End => {
                    self.input_ended = true;
                    return Ok(if line.is_empty() && !over {
                        None
                    } else {
                        Some(Err("call line ended without newline".into()))
                    });
                }
                stream::Up::Failed(error) => return Err(format!("stdin: {error:?}")),
                stream::Up::Room => return Err("unexpected stdin room".into()),
            }
        }
    }
    fn pump(&mut self) -> Result<(), String> {
        let time = self.clock.now();
        self.ienv.now = time.now;
        self.ienv.wall = time.wall;
        self.benv.now = time.now;
        self.benv.wall = time.wall;
        while let Some(complete) = self.ring.reap().pop() {
            let (host, complete) = self.ring.complete(complete);
            self.ring.returned(host, complete);
        }
        self.ring.deliver(0, &mut self.complete);
        let mut roots = Queue::with_capacity(1);
        self.ring.deliver(1, &mut roots);
        if let Some(complete) = roots.pop() {
            complete.result.map_err(|e| format!("root close: {e:?}"))?;
            self.root_closed = true;
        }
        while self.io.is_ready() {
            io::resume(&mut self.io, &self.ienv, &mut self.io_events, &mut self.submits);
        }
        while let Some(complete) = self.complete.pop() {
            io::up(&mut self.io, &self.ienv, complete, &mut self.io_events, &mut self.submits);
        }
        while self.io.is_due(time.now) {
            io::fire(&mut self.io, &self.ienv, &mut self.io_events, &mut self.submits);
        }
        while let Some(event) = self.io_events.pop() {
            self.io_event(event)?;
        }
        if self.pipes.is_some() {
            for _ in 0..32 {
                if !self.browser.work_pending() && self.browser.next_deadline().is_none_or(|at| at > time.now) {
                    break;
                }
                browser::fire(&mut self.browser, &self.benv, time.now, &mut self.events, &mut self.down);
            }
        }
        self.browser_events();
        while let Some(down) = self.down.pop() {
            let [commands, replies, errors] = self.pipes.ok_or("browser used before spawn")?;
            let (pipe, down) = match down {
                b::Down::Commands(d) => (commands, d),
                b::Down::Replies(d) => (replies, d),
                b::Down::Errors(d) => (errors, d),
            };
            self.stream(pipe, down);
        }
        for pages in self.origins.values_mut() {
            pages.update_time(time.now, time.wall);
            while let Some(request) = pages.take() {
                self.requests.push_back(request);
            }
        }
        if let Some(listener) = &mut self.listener {
            listener.fire(time.now);
            self.requests.extend(listener.requests.drain(..));
        }
        self.requests.extend(self.proxy.requests.drain(..));
        while self.io.takes() {
            let Some(request) = self.requests.pop_front() else {
                break;
            };
            io::down(&mut self.io, &self.ienv, request, &mut self.submits);
        }
        self.io.reclaim();
        self.browser.reclaim();
        let pending = self.io.is_ready()
            || !self.requests.is_empty()
            || !self.answers.is_empty()
            || !self.input_events.is_empty()
            || self.browser.work_pending()
            || self.ring.has_ready();
        let until = self
            .io
            .next_deadline()
            .into_iter()
            .chain(self.browser.next_deadline())
            .fold(time.now.saturating_add(POLL), Time::min);
        while let Some(record) = self.submits.pop() {
            let host = usize::from(record.op == ROOT);
            let record = self.ring.register(host, record);
            self.ring.queue(record);
        }
        self.ring.enter(if pending { Wait::No } else { Wait::Until(until) });
        Ok(())
    }
    fn browser_events(&mut self) {
        while let Some(event) = self.events.pop() {
            if self.listener.is_some() {
                self.answers.push_back(event);
                continue;
            }
            match event {
                Event::Trouble { page, trouble, text } => {
                    let kind = match trouble {
                        b::Trouble::Exception => "exception",
                        b::Trouble::Console => "console",
                        b::Trouble::Log => "log",
                        b::Trouble::Crash => "crash",
                        b::Trouble::Dialog => "dialog",
                    };
                    if let Some(p) = self.pages.values_mut().find(|p| p.page == page) {
                        p.trouble.push(calls::trouble(kind, json::bytes(&text)));
                    }
                }
                Event::Warned { .. } => {}
                terminal @ (Event::Ready { .. }
                | Event::Opened { .. }
                | Event::Found { .. }
                | Event::Met { .. }
                | Event::Missed { .. }
                | Event::Done { .. }
                | Event::Refused { .. }
                | Event::Address { .. }
                | Event::Attribute { .. }
                | Event::Snapshot { .. }
                | Event::Screenshot { .. }
                | Event::Looked { .. }
                | Event::Closed { .. }) => self.answers.push_back(terminal),
            }
        }
    }
    fn io_event(&mut self, event: io::Event) -> Result<(), String> {
        let owner = owner(&event);
        if let Some(owner) = owner {
            if let Some(listener) = &mut self.listener
                && listener.owns(owner)
            {
                return listener.event(event, self.ienv.now);
            }
            if self.proxy.owns(owner) {
                return self.proxy.event(event);
            }
            if let Some(pages) = self.origins.values_mut().find(|p| p.owns(owner)) {
                pages.event(event);
                return Ok(());
            }
        }
        match event {
            io::Event::Spawned { pipes, .. } => {
                let [commands, replies, errors] = pipes.as_ref() else {
                    return Err("Chromium pipe layout".into());
                };
                self.pipes = Some([*commands, *replies, *errors]);
                let root = self.root.take().ok_or("Chromium root missing")?;
                self.submits.push(Submit { op: ROOT, kind: Op::Close { fd: root } });
            }
            io::Event::Stream { owner, up } => {
                if owner == self.input {
                    self.input_events.push_back(up);
                    return Ok(());
                }
                if owner == self.output {
                    match up {
                        stream::Up::Room => self.output_room = true,
                        stream::Up::End => {}
                        stream::Up::Failed(e) => return Err(format!("stdout: {e:?}")),
                        stream::Up::Bytes(_) => return Err("stdout read".into()),
                    }
                    return Ok(());
                }
                if let Some([commands, replies, errors]) = self.pipes {
                    let below = if owner == commands {
                        b::Below::Commands(up)
                    } else if owner == replies {
                        b::Below::Replies(up)
                    } else if owner == errors {
                        b::Below::Errors(up)
                    } else {
                        return Err(format!("unknown stream {owner:?}"));
                    };
                    browser::up(&mut self.browser, &self.benv, below, &mut self.events, &mut self.down);
                }
            }
            io::Event::Exited { exit, .. } => {
                if !self.closing {
                    return Err(format!(
                        "Chromium exited: {exit:?}; {}",
                        String::from_utf8_lossy(self.browser.stderr_tail())
                    ));
                }
                self.exited = true;
                if let Some(pipes) = self.pipes {
                    for entity in pipes {
                        self.requests.push_back(io::Request::Close { entity });
                    }
                }
            }
            io::Event::Closed { owner } => {
                if owner == PROCESS {
                    self.child_closed = true;
                }
            }
            io::Event::Failed { owner, error } => return Err(format!("io {owner:?}: {error:?}")),
            io::Event::Shutdown { .. } => self.interrupted = true,
            unexpected @ (io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Output { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }) => return Err(format!("unexpected io event: {unexpected:?}")),
        }
        Ok(())
    }
    fn call(&mut self, call: Call, config: &Config) -> Result<Value, String> {
        if let Call::Serve { origin, dir } = call {
            return self.serve(origin, &dir);
        }
        if let Call::Open { page, width, height, mobile } = call {
            return self.open(page, width, height, mobile);
        }
        let name = call.page().expect("page call").to_owned();
        let Some(p) = self.pages.get(&name) else {
            return Ok(calls::refused("page", format!("unknown page {name}")));
        };
        let page = p.page;
        let op = self.fresh();
        let request = match call {
            Call::Go { url, .. } => {
                if !calls::on_origin(&url, self.origins.values().filter_map(|p| p.addr.map(|a| format!("http://{a}"))))
                {
                    return Ok(calls::refused("origin", "URL is outside served origins"));
                }
                Request::Go { page, op, to: b::Go::Address(url.into_bytes().into()) }
            }
            Call::Read { read, .. } => {
                let file = if let calls::Read::Shot(_, _, file) = &read { Some(file.clone()) } else { None };
                let request = match read.request(page, op, self.benv.limits.properties) {
                    Ok(request) => request,
                    Err(answer) => return Ok(answer),
                };
                self.ask(request);
                let event = self.next_event()?;
                if let Event::Looked { op: answer, looked: b::Looked::Shot(png) } = &event
                    && *answer == op
                {
                    return shot(config, &file.expect("shot file"), png);
                }
                return calls::read_answer(&event, op).ok_or_else(|| format!("unexpected read answer: {event:?}"));
            }
            Call::Trouble { .. } => {
                let mut trouble = std::mem::take(&mut self.pages.get_mut(&name).expect("known page").trouble);
                trouble.extend(self.proxy.take().into_iter().map(|text| calls::trouble("network", string(text))));
                return Ok(object([("trouble", Value::Array(trouble))]));
            }
            Call::Close { .. } => {
                self.close_page(&name)?;
                return Ok(object([("closed", string(name))]));
            }
            Call::Serve { .. } | Call::Open { .. } => unreachable!("handled before page lookup"),
        };
        self.ask(request);
        self.answer(op, &name)
    }
    fn answer(&mut self, op: Token, name: &str) -> Result<Value, String> {
        match self.next_event()? {
            Event::Done { op: answer } if answer == op => Ok(object([("done", string(name))])),
            Event::Refused { op: answer, why } if answer == op => Ok(calls::kit_refusal(why)),
            other @ (Event::Ready { .. }
            | Event::Opened { .. }
            | Event::Found { .. }
            | Event::Met { .. }
            | Event::Missed { .. }
            | Event::Done { .. }
            | Event::Warned { .. }
            | Event::Refused { .. }
            | Event::Address { .. }
            | Event::Attribute { .. }
            | Event::Snapshot { .. }
            | Event::Screenshot { .. }
            | Event::Looked { .. }
            | Event::Trouble { .. }
            | Event::Closed { .. }) => Err(format!("unexpected call answer: {other:?}")),
        }
    }
    fn serve(&mut self, name: String, dir: &str) -> Result<Value, String> {
        if self.origins.contains_key(&name) {
            return Ok(calls::refused("origin", "origin name already served"));
        }
        if self.origins.len() >= 8 {
            return Ok(calls::refused("limit", "eight origins already served"));
        }
        let pages = match files::pages(Path::new(dir), files::LIMITS) {
            Ok(pages) => pages,
            Err((why, detail)) => return Ok(calls::refused(why, detail)),
        };
        let pages = Pages::verbatim(
            pages,
            self.ienv.now,
            self.ienv.wall,
            u64::try_from(self.origins.len()).expect("eight origins"),
        );
        self.origins.insert(name.clone(), pages);
        loop {
            if let Some(addr) = self.origins.get(&name).and_then(|p| p.addr) {
                return Ok(object([(
                    "served",
                    object([("address", string(format!("http://{addr}"))), ("origin", string(name))]),
                )]));
            }
            self.pump()?;
        }
    }
    fn open(&mut self, name: String, width: u32, height: u32, mobile: bool) -> Result<Value, String> {
        if self.pages.contains_key(&name) {
            return Ok(calls::refused("page", "page name already open"));
        }
        if self.pages.len() >= usize::try_from(self.benv.limits.persons).expect("person limit") {
            return Ok(calls::refused("limit", "person count"));
        }
        let person = self.fresh();
        let page = self.fresh();
        let op = self.fresh();
        self.ask(Request::Person { person });
        while self.browser.work_pending() || self.browser.next_deadline().is_some() {
            self.pump()?;
        }
        if let Some(event) = self.answers.pop_front() {
            return Err(format!("person creation failed: {event:?}"));
        }
        self.ask(Request::Page { person, page, url: b"about:blank".as_slice().into() });
        match self.next_event()? {
            Event::Opened { page: opened } if opened == page => {}
            other @ (Event::Ready { .. }
            | Event::Opened { .. }
            | Event::Found { .. }
            | Event::Met { .. }
            | Event::Missed { .. }
            | Event::Done { .. }
            | Event::Warned { .. }
            | Event::Refused { .. }
            | Event::Address { .. }
            | Event::Attribute { .. }
            | Event::Snapshot { .. }
            | Event::Screenshot { .. }
            | Event::Looked { .. }
            | Event::Trouble { .. }
            | Event::Closed { .. }) => return Err(format!("page creation failed: {other:?}")),
        }
        self.pages.insert(name.clone(), OpenPage { page, person, trouble: Vec::new() });
        self.ask(Request::Look {
            page,
            op,
            look: b::Look::Viewport { width, height, mobile, scheme: b::Scheme::Light, motion: b::Motion::Reduce },
        });
        match self.next_event()? {
            Event::Looked { op: answer, looked: b::Looked::Viewport } if answer == op => {
                Ok(object([("opened", string(name))]))
            }
            Event::Refused { op: answer, why } if answer == op => {
                self.close_page(&name)?;
                Ok(calls::kit_refusal(why))
            }
            other @ (Event::Ready { .. }
            | Event::Opened { .. }
            | Event::Found { .. }
            | Event::Met { .. }
            | Event::Missed { .. }
            | Event::Done { .. }
            | Event::Warned { .. }
            | Event::Refused { .. }
            | Event::Address { .. }
            | Event::Attribute { .. }
            | Event::Snapshot { .. }
            | Event::Screenshot { .. }
            | Event::Looked { .. }
            | Event::Trouble { .. }
            | Event::Closed { .. }) => Err(format!("viewport failed: {other:?}")),
        }
    }
    fn close_page(&mut self, name: &str) -> Result<(), String> {
        let p = self.pages.get(name).expect("known page");
        let page = p.page;
        let person = p.person;
        self.ask(Request::Close { entity: page });
        self.closed(page)?;
        self.ask(Request::Close { entity: person });
        self.closed(person)?;
        self.pages.remove(name);
        Ok(())
    }
    fn closed(&mut self, entity: Token) -> Result<(), String> {
        match self.next_event()? {
            Event::Closed { owner } if owner == entity => Ok(()),
            other @ (Event::Ready { .. }
            | Event::Opened { .. }
            | Event::Found { .. }
            | Event::Met { .. }
            | Event::Missed { .. }
            | Event::Done { .. }
            | Event::Warned { .. }
            | Event::Refused { .. }
            | Event::Address { .. }
            | Event::Attribute { .. }
            | Event::Snapshot { .. }
            | Event::Screenshot { .. }
            | Event::Looked { .. }
            | Event::Trouble { .. }
            | Event::Closed { .. }) => Err(format!("expected Closed {entity:?}: {other:?}")),
        }
    }
    fn shutdown(&mut self) -> Result<(), String> {
        let names = self.pages.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.close_page(&name)?;
        }
        self.closing = true;
        self.ask(Request::Close { entity: BROWSER });
        self.closed(BROWSER)?;
        for pages in self.origins.values_mut() {
            pages.stop();
        }
        self.proxy.stop();
        self.requests.push_back(io::Request::Close { entity: self.input });
        self.requests.push_back(io::Request::Close { entity: self.output });
        let deadline = self.clock.now().now.saturating_add(Duration::from_secs(8));
        while !(self.exited
            && self.child_closed
            && self.root_closed
            && self.proxy.closed()
            && self.origins.values().all(Pages::is_closed)
            && self.io.is_empty()
            && self.ring.is_empty()
            && self.requests.is_empty()
            && self.submits.is_empty())
        {
            if self.clock.now().now >= deadline {
                return Err("shutdown deadline passed".into());
            }
            self.pump()?;
        }
        Ok(())
    }
}
fn owner(event: &io::Event) -> Option<Token> {
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
// lab-browser.md, 8.10: reserve a new file before asking Chromium.
fn prepare_look_shot(config: &Config, file: &str) -> Result<Result<std::fs::File, Value>, String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let mut parent = config.out.clone();
    for component in file.split('/').take(file.split('/').count() - 1) {
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(m) if !m.is_dir() => return Ok(Err(calls::refused("call", "shot parent is not a directory"))),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new().mode(0o700).create(&parent).map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(config.out.join(file)) {
        Ok(file) => Ok(Ok(file)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(Err(calls::refused("call", "shot file exists"))),
        Err(e) => Err(e.to_string()),
    }
}
fn shot(config: &Config, file: &str, png: &[u8]) -> Result<Value, String> {
    let answer = shot_answer(file, png)?;
    let path = config.out.join(file);
    let mut parent = config.out.clone();
    for component in file.split('/').take(file.split('/').count() - 1) {
        parent.push(component);
        match std::fs::symlink_metadata(&parent) {
            Ok(m) if !m.is_dir() => return Ok(calls::refused("call", "shot parent is not a directory")),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&parent).map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
        return Ok(calls::refused("call", "shot target is not a regular file"));
    }
    std::fs::write(path, png).map_err(|e| e.to_string())?;
    Ok(answer)
}
fn shot_answer(file: &str, png: &[u8]) -> Result<Value, String> {
    let (width, height) = calls::png_size(png)?;
    Ok(object([(
        "shot",
        object([
            ("bytes", number(png.len())),
            ("file", string(file)),
            ("height", number(height)),
            ("sha256", string(calls::digest(png))),
            ("width", number(width)),
        ]),
    )]))
}

#[cfg(test)]
mod tests {
    use super::{CAPTURE_SWITCHES, browser, capture_args};

    #[test]
    fn capture_switches_follow_kit_switches_before_initial_url() {
        let command = browser::command::command(b"/chromium", b"/scratch/profile");
        let args = capture_args(&command.args, "127.0.0.1:12345".parse().unwrap());
        let (initial_page, switches) = args.as_slice().split_last().unwrap();
        assert_eq!(initial_page.as_ref(), b"about:blank");
        let (kit, capture) = switches.split_at(switches.len() - CAPTURE_SWITCHES.len());
        for (arg, expected) in capture.iter().zip(CAPTURE_SWITCHES) {
            assert_eq!(arg.as_ref(), expected.as_bytes());
        }
        // lab-browser.md, 2.4: the stable shot's whole set, in order.
        let stable: &[&[u8]] = &[
            b"--hide-scrollbars",
            b"--disable-partial-raster",
            b"--default-tile-width=2048",
            b"--default-tile-height=2048",
            b"--max-untiled-layer-width=2048",
            b"--max-untiled-layer-height=2048",
        ];
        let tail: Vec<&[u8]> = capture[capture.len() - stable.len()..].iter().map(AsRef::as_ref).collect();
        assert_eq!(tail, stable);
        assert_eq!(kit.last().unwrap().as_ref(), b"--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1");
        assert!(kit.iter().any(|arg| arg.as_ref() == b"--proxy-server=http://127.0.0.1:12345"));
    }
}
