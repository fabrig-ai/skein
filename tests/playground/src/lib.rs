//! The visitor against a scripted HTTP server (playground.md, 8).
use core::net::SocketAddr;
use skein_io::{self as io, kernel};
use skein_lib::{Duration, Queue, Time, Wall};
use skein_playground::visitor::{Limits, Settings, Visitor};
use skein_ui_web::{bridge::Duty, fetch::Report};
use skein_world::{Host, Memory, Outcome, Referee, World};
mod contracts;
pub mod server;
const QUEUE: u32 = 256;
const FINISH: Time = Time::from_nanos(10_000_000_000);
pub const SLOW: Duration = Duration::from_millis(100);
#[must_use]
pub fn io_limits() -> io::Limits {
    io::Limits {
        sockets: 12,
        names: 0,
        refusals: 4,
        intake: 4096,
        receive: 4096,
        output: 4096,
        sends: 4,
        accepts: 4,
        backlog: 8,
        close_timeout: Duration::from_millis(10),
        retry: Duration::from_millis(1),
    }
}
#[must_use]
pub fn limits() -> Limits {
    Limits {
        io: io_limits(),
        http: skein_http::client::Limits { request: 1024, head: 512, headers: 8, read: 64, send: 64 },
        exchanges: 2,
        duties: 4,
        request: 512,
        answer: 256,
        piece: 8,
        cookies: 2,
        cookie: 16,
    }
}
fn settings(limits: Limits, listener: SocketAddr) -> Settings {
    Settings {
        limits,
        listener,
        host: b"application.example".as_slice().into(),
        origin: b"http://application.example".as_slice().into(),
    }
}
#[derive(Clone, Copy, Debug)]
pub enum When {
    At(Time),
    After(usize),
    AfterAt {
        reports: usize,
        at: Time,
    },
    /// A response has been queued by the independent peer, at this same instant.
    ServerAnswered(usize),
}
pub struct Step {
    pub when: When,
    pub duty: Duty,
}
pub struct Story {
    pub steps: Vec<Step>,
    pub reports: usize,
    /// A named keep to prove silence, expiry or read pacing before teardown.
    pub until: Time,
    pub limits: Limits,
    pub listener: SocketAddr,
    pub slow: Duration,
}
impl Story {
    #[must_use]
    pub fn new(steps: Vec<Step>, reports: usize) -> Self {
        Self { steps, reports, until: Time::ZERO, limits: limits(), listener: server::ADDRESS, slow: SLOW }
    }
}
pub struct VisitorProcess {
    visitor: Visitor,
    inputs: Vec<Option<Duty>>,
    taken: usize,
    pub reports: Vec<Report>,
    pub stamps: Vec<Time>,
    pub inputs_at: Vec<Time>,
    pub closing: bool,
    bound: u64,
}
impl VisitorProcess {
    fn new(story: &Story) -> Self {
        let inputs: Vec<_> = story.steps.iter().map(|step| Some(step.duty.clone())).collect();
        let records = story
            .steps
            .len()
            .checked_mul(3)
            .and_then(|count| count.checked_add(usize::try_from(story.limits.exchanges).ok()? + 2))
            .expect("finite story");
        let visitor = Visitor::new(settings(story.limits, story.listener));
        // Inputs are allocated inside this process's constructor. The referee
        // moves them into the preallocated duty queue between steps.
        let input_bytes = inputs.iter().flatten().map(duty_bytes).sum::<u64>();
        let bound = visitor.worst_case()
            + input_bytes
            + u64::try_from(
                inputs.capacity() * size_of::<Option<Duty>>()
                    + records * (size_of::<Report>() + size_of::<Time>())
                    + inputs.len() * size_of::<Time>()
                    + size_of::<Self>(),
            )
            .expect("fixture storage")
            + u64::try_from(records).expect("record count") * u64::from(story.limits.answer.max(story.limits.piece));
        Self {
            visitor,
            inputs,
            taken: 0,
            reports: Vec::with_capacity(records),
            stamps: Vec::with_capacity(records),
            inputs_at: Vec::with_capacity(story.steps.len()),
            closing: false,
            bound,
        }
    }
    fn iterate(&mut self, now: Time, wall: Wall) {
        if self.closing {
            self.visitor.close();
        }
        self.visitor.iterate(now, wall);
        while let Some(report) = self.visitor.report() {
            assert!(self.reports.len() < self.reports.capacity(), "the fixture's finite report history fits");
            self.reports.push(report);
            self.stamps.push(now);
        }
    }
}
fn duty_bytes(duty: &Duty) -> u64 {
    let bytes = match duty {
        Duty::Exchange(exchange) => {
            exchange.path.len()
                + exchange.body.len()
                + exchange.method.capacity()
                + exchange.headers.capacity() * size_of::<(String, String)>()
                + exchange.headers.iter().map(|(name, value)| name.capacity() + value.capacity()).sum::<usize>()
        }
        Duty::Stream(skein_ui_web::transport::StreamDuty::Open { path, headers, .. }) => {
            path.len()
                + headers.capacity() * size_of::<skein_ui_web::transport::Header>()
                + headers.iter().map(|(name, value)| name.len() + value.len()).sum::<usize>()
        }
        Duty::Abandon { .. } | Duty::ReadOn | Duty::Stream(skein_ui_web::transport::StreamDuty::Close) => 0,
    };
    u64::try_from(bytes).expect("fixture input bytes")
}

pub enum Process {
    Visitor(Box<VisitorProcess>),
    Server(Box<server::Server>),
}
impl Host for Process {
    fn iterate(&mut self, now: Time, wall: Wall) {
        match self {
            Self::Visitor(process) => process.iterate(now, wall),
            Self::Server(server) => server.iterate(now, wall),
        }
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        match self {
            Self::Visitor(process) => process.visitor.completions(),
            Self::Server(server) => server.completions(),
        }
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        match self {
            Self::Visitor(process) => process.visitor.submissions(),
            Self::Server(server) => server.submissions(),
        }
    }
    fn work_pending(&self, now: Time) -> bool {
        match self {
            Self::Visitor(process) => {
                process.visitor.work_pending(now) || (process.closing && !process.visitor.is_empty())
            }
            Self::Server(server) => server.work_pending(now),
        }
    }
    fn next_deadline(&self) -> Option<Time> {
        match self {
            Self::Visitor(process) => process.visitor.next_deadline(),
            Self::Server(server) => server.next_deadline(),
        }
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        match self {
            Self::Visitor(process) => process.visitor.next_policy_deadline(),
            Self::Server(server) => server.next_policy_deadline(),
        }
    }
    fn is_empty(&self) -> bool {
        match self {
            Self::Visitor(process) => process.visitor.is_empty(),
            Self::Server(server) => server.is_empty(),
        }
    }
    fn worst_case(&self) -> u64 {
        match self {
            Self::Visitor(process) => process.bound,
            Self::Server(server) => {
                server.worst_case() + u64::try_from(size_of::<server::Server>()).expect("server size")
            }
        }
    }
    fn operations(&self) -> u32 {
        match self {
            Self::Visitor(process) => process.visitor.operations(),
            Self::Server(server) => server.operations(),
        }
    }
}
struct Judge {
    steps: Vec<When>,
    entered: usize,
    expected: usize,
    until: Time,
    duties: usize,
    passed: bool,
    now: Time,
    wake: bool,
    contracts: contracts::Contracts,
    inputs: Vec<contracts::Input>,
    checked_inputs: usize,
    checked_reports: usize,
}
impl Referee<Process> for Judge {
    fn act(&mut self, now: Time, procs: &mut [Process]) {
        self.now = now;
        self.wake = false;
        let visitor = visitor(procs);
        let reports = visitor.reports.len();
        let taken = visitor.taken;
        let (ready, answers) = procs
            .iter()
            .find_map(|process| match process {
                Process::Server(server) => Some((server.ready, server.answers)),
                Process::Visitor(_) => None,
            })
            .expect("scripted server");
        if ready {
            while self.entered.saturating_sub(taken) < self.duties {
                let Some(when) = self.steps.get(self.entered) else { break };
                let due = match when {
                    When::At(at) => *at <= now,
                    When::After(count) => *count <= reports,
                    When::AfterAt { reports: count, at } => *count <= reports && *at <= now,
                    When::ServerAnswered(count) => *count <= answers,
                };
                if !due {
                    break;
                }
                self.entered += 1;
            }
        }
        assert!(reports <= self.expected, "no surplus reports: {:?}", visitor.reports);
        if self.entered == self.steps.len() && taken == self.entered && reports == self.expected && now >= self.until {
            self.passed = true;
        }
        for process in procs {
            match process {
                Process::Visitor(visitor) => {
                    let span = skein_heap::Span::start();
                    while visitor.taken < self.entered {
                        visitor.visitor.duty(visitor.inputs[visitor.taken].take().expect("one duty per step"));
                        visitor.inputs_at.push(now);
                        visitor.taken += 1;
                    }
                    let growth = span.end();
                    assert_eq!(growth.net, 0, "the referee moves process-owned inputs without allocating or freeing");
                    assert_eq!(growth.peak, 0, "the referee's transfers need no transient allocation");
                    visitor.closing = self.passed;
                }
                Process::Server(server) => server.closing = self.passed,
            }
        }
    }
    fn observe(&mut self, now: Time, procs: &[Process]) {
        let visitor = visitor(procs);
        for input in &self.inputs[self.checked_inputs..visitor.taken] {
            self.contracts.input(*input);
        }
        self.checked_inputs = visitor.taken;
        for report in &visitor.reports[self.checked_reports..] {
            self.contracts.report(report);
        }
        self.checked_reports = visitor.reports.len();
        if self.passed {
            self.contracts.settled();
        }
        let (ready, answers) = procs
            .iter()
            .find_map(|process| match process {
                Process::Server(server) => Some((server.ready, server.answers)),
                Process::Visitor(_) => None,
            })
            .expect("server");
        let due = ready
            && self.steps.get(self.entered).is_some_and(|when| match when {
                When::At(at) => *at <= now,
                When::After(count) => *count <= visitor.reports.len(),
                When::AfterAt { reports, at } => *reports <= visitor.reports.len() && *at <= now,
                When::ServerAnswered(count) => *count <= answers,
            });
        self.wake = !self.passed
            && ((due && self.entered == visitor.taken)
                || (self.entered == self.steps.len()
                    && visitor.taken == self.entered
                    && visitor.reports.len() == self.expected
                    && now >= self.until));
        assert!(visitor.reports.len() <= self.expected, "no surplus reports: {:?}", visitor.reports);
    }
    fn next_deadline(&self) -> Option<Time> {
        if self.passed {
            return None;
        }
        if self.wake {
            return Some(self.now);
        }
        let input = self.steps.get(self.entered).and_then(|when| match when {
            When::At(at) | When::AfterAt { at, .. } => Some(*at),
            When::After(_) | When::ServerAnswered(_) => None,
        });
        // Past instants are already enabled or waiting for a report, not timers.
        input
            .filter(|at| *at > self.now)
            .into_iter()
            .chain((self.until > self.now).then_some(self.until))
            .chain(Some(FINISH))
            .min()
    }
    fn overdue(&self, now: Time) -> Option<String> {
        (now >= FINISH && !self.passed)
            .then(|| format!("visitor story stopped after {} of {} duties", self.entered, self.steps.len()))
    }
    fn passed(&self) -> bool {
        self.passed
    }
}
#[must_use]
pub fn visitor(procs: &[Process]) -> &VisitorProcess {
    procs
        .iter()
        .find_map(|process| match process {
            Process::Visitor(visitor) => Some(visitor.as_ref()),
            Process::Server(_) => None,
        })
        .expect("visitor process")
}
fn copy_report(report: &Report) -> Report {
    match report {
        Report::Answered { exchange, status, body } => {
            Report::Answered { exchange: *exchange, status: *status, body: body.clone() }
        }
        Report::Failed { exchange } => Report::Failed { exchange: *exchange },
        Report::StreamOpened => Report::StreamOpened,
        Report::StreamRefused { status } => Report::StreamRefused { status: *status },
        Report::StreamPiece { bytes } => Report::StreamPiece { bytes: bytes.clone() },
        Report::StreamEnded => Report::StreamEnded,
    }
}
/// Each story runs calm, under the counting allocator, to settled io teardown.
#[must_use]
pub fn run(seed: u64, story: Story) -> (Outcome<Process>, Vec<Report>) {
    let mut world = World::new(
        seed,
        skein_sim::Config::calm(),
        Judge {
            steps: story.steps.iter().map(|step| step.when).collect(),
            entered: 0,
            expected: story.reports,
            until: story.until,
            duties: usize::try_from(story.limits.duties).expect("duty count fits"),
            passed: false,
            now: Time::ZERO,
            wake: false,
            contracts: contracts::Contracts::new(),
            inputs: story.steps.iter().map(|step| contracts::Input::of(&step.duty)).collect(),
            checked_inputs: 0,
            checked_reports: 0,
        },
        Memory::Checked,
    );
    world.spawn(|| Process::Server(Box::new(server::Server::new(story.slow))));
    world.spawn(|| Process::Visitor(Box::new(VisitorProcess::new(&story))));
    drop(story);
    let outcome = world.run();
    let reports = visitor(&outcome.procs).reports.iter().map(copy_report).collect();
    (outcome, reports)
}
