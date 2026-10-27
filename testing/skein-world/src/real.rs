//! The real loop (testing-strategy.md, 2.8): the same processes and referee
//! as a simulated world, in one thread and one loop, whose calls go to the
//! real kernel through one backend, on loopback, with deadlines on the real
//! clock (examples.md, section 6). It keeps token bindings, operation limits,
//! hosted factories, descriptor ownership and queued referee signals; it
//! never inspects service state. `World::host` selects a spawn's factory,
//! while `spawn_signalfd` and `spawn_signals` supply roots' signal sources.
//! `run` blocks only when every process and the control queue are idle,
//! and returns once exits, operations and descriptor closures settle. It
//! checks policy deadlines kept before each wait after the referee's last
//! word, before hosts can clear them (testing-strategy.md, 6). It does not
//! replay. Checked constructors meter each process separately; their
//! outcome verifies exact release on drop (testing-strategy.md, section 6).

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::ops::{Deref, DerefMut};

use crate::ring::Ring;
use skein_heap::Span;
use skein_io::kernel::{Complete, Done, Error, Exit, Fd, Op, ServiceSignal, Signal, Spawn, Submit};
use skein_lib::{Duration, Queue, Time, Token};
use skein_shell::{Backend, Clock, Now, Wait};

use crate::Host;
use crate::heap::Heap;
use crate::program::Startup;
pub use crate::referee::Controls;
use crate::referee::Referee;
use crate::{HostedProgram, Inherited, StartupAppends, StartupRoots};
use std::os::unix::ffi::OsStrExt;

// Hosted identities occupy the high word; kernel PIDs fit in a u32.
const HOSTED_SENDER_BASE: u64 = 1_u64 << 32;

fn hosted_sender(host: usize) -> skein_io::kernel::Sender {
    let identity = HOSTED_SENDER_BASE
        .checked_add(u64::from(u32::try_from(host).expect("host index fits u32")))
        .expect("two u32 words fit u64");
    skein_io::kernel::Sender::new(identity)
}

/// What a real run left.
#[derive(Debug)]
pub struct Outcome<P> {
    /// The processes as the run left them, settled.
    pub procs: Vec<P>,
    pub killed: Vec<Killed>,
    /// Per-process peak and worst case, in surviving admission order.
    pub heap: Option<Vec<(u64, u64)>>,
    /// Bytes each surviving process must release when dropped.
    pub held: Option<Vec<i64>>,
    pub iterations: u32,
    /// When the run began and settled, on the real clock.
    pub start: Time,
    pub end: Time,
}

/// A checked real outcome, borrowed for assertions; its final drop verifies
/// exact per-process release (testing-strategy.md, section 6).
#[derive(Debug)]
pub struct CheckedOutcome<P>(Outcome<P>);

impl<P> Deref for CheckedOutcome<P> {
    type Target = Outcome<P>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<P> Drop for CheckedOutcome<P> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let held = self.0.held.take().expect("a checked outcome keeps its ownership ledger");
        assert_eq!(self.0.procs.len(), held.len(), "an ownership entry per surviving process");
        for (at, (proc, held)) in self.0.procs.drain(..).zip(held).enumerate() {
            let span = Span::start();
            drop(proc);
            let freed = span.end().net.checked_neg().expect("a heap within an i64");
            assert_eq!(freed, held, "real process {at} must release exactly its own metered heap");
        }
    }
}

/// A real world whose constructors and iterations are metered separately.
/// Its checked outcome verifies final release, while the existing unchecked
/// world permits callers to take surviving processes by value.
#[derive(Debug)]
pub struct CheckedWorld<P, R>(World<P, R>);

impl<P, R> Deref for CheckedWorld<P, R> {
    type Target = World<P, R>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<P, R> DerefMut for CheckedWorld<P, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<P: Host, R: Referee<P>> CheckedWorld<P, R> {
    /// Runs on one kernel and retains final ownership checking in the outcome.
    #[must_use]
    pub fn run(self, clock: &Clock, patience: Duration) -> CheckedOutcome<P> {
        CheckedOutcome(self.0.run(clock, patience))
    }
}

/// Runs unhosted processes on the shared real kernel; `World` also hosts spawns.
#[must_use]
pub fn run<P: Host, R: Referee<P>>(procs: Vec<P>, referee: R, clock: &Clock, patience: Duration) -> Outcome<P> {
    let mut world = World::new(referee);
    for proc in procs {
        world.spawn(|| proc);
    }
    world.run(clock, patience)
}

/// A killed hosted child, reported after its operations and descriptors settle.
#[derive(Debug)]
pub struct Killed {
    pub host: usize,
    pub exit: Exit,
    /// Peak and bound after exact metered release, when checked.
    pub heap: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug)]
enum SignalSource {
    Pipe(Fd),
    Real,
}

/// A real world built by the scenario, with factories shared with simulated worlds.
#[derive(Debug)]
pub struct World<P, R> {
    procs: Vec<P>,
    referee: R,
    programs: Vec<HostedProgram<P>>,
    startup: Vec<Startup>,
    inherited: Vec<Vec<Fd>>,
    controls: Controls,
    signals: BTreeMap<usize, SignalSource>,
    signalfd: bool,
    heap: Option<Heap>,
    backend: Option<Backend>,
}

impl<P: Host, R: Referee<P>> World<P, R> {
    /// Starts an empty real world with its observation referee.
    #[must_use]
    pub fn new(referee: R) -> Self {
        Self::new_controlled(|_| referee)
    }

    /// Constructs a referee with a handle for injecting signals into this world.
    #[must_use]
    pub fn new_controlled<F: FnOnce(Controls) -> R>(make_referee: F) -> Self {
        let controls = Controls::new();
        let referee = make_referee(controls.clone());
        Self {
            procs: Vec::new(),
            referee,
            programs: Vec::new(),
            startup: Vec::new(),
            inherited: Vec::new(),
            controls,
            signals: BTreeMap::new(),
            signalfd: false,
            heap: None,
            backend: None,
        }
    }

    /// Runs on `backend` rather than the host's (shell.md, 7.1 and 9).
    #[must_use]
    pub fn on(mut self, backend: Backend) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Starts a checked world under skein's counting allocator. Build every
    /// root inside its spawn closure; referee and backend storage is excluded.
    #[must_use]
    pub fn new_checked(referee: R) -> CheckedWorld<P, R> {
        Self::new_controlled_checked(|_| referee)
    }

    /// Constructs a checked world with referee-controlled signal injection.
    #[must_use]
    pub fn new_controlled_checked<F: FnOnce(Controls) -> R>(make_referee: F) -> CheckedWorld<P, R> {
        let mut world = Self::new_controlled(make_referee);
        world.heap = Some(Heap::new());
        CheckedWorld(world)
    }

    /// Adds a service whose signal records arrive through its own real pipe.
    pub fn spawn_signals<F: FnOnce(Fd) -> P>(&mut self, make: F) -> usize {
        let (reader, writer) = skein_shell::open_signal_pipe().expect("open the service signal pipe");
        let host = self.spawn_with_fds(vec![reader], || make(reader));
        self.signals.insert(host, SignalSource::Pipe(writer));
        host
    }

    /// Adds the one service that owns this process's blocked termination signalfd.
    /// Construct and run the world on the same thread.
    pub fn spawn_signalfd<F: FnOnce(Fd) -> P>(&mut self, make: F) -> usize {
        assert!(!self.signalfd, "only one service reads the process signalfd");
        let reader = skein_shell::open_termination_signals().expect("block termination signals and open signalfd");
        let host = self.spawn_with_fds(vec![reader], || make(reader));
        self.signals.insert(host, SignalSource::Real);
        self.signalfd = true;
        host
    }

    /// Registers a program and its maximum simultaneous instances before execution.
    pub fn host(&mut self, program: HostedProgram<P>) {
        self.host_roots(program, crate::program::no_roots);
    }

    /// Registers independently opened startup directories selected from each
    /// spawn, shared with simulated scenarios (testing-strategy.md, section 2.8).
    pub fn host_roots(&mut self, program: HostedProgram<P>, roots: StartupRoots) {
        self.host_startup(program, roots, crate::program::no_appends);
    }

    /// Registers independently opened roots and append files for each launch.
    pub fn host_startup(&mut self, program: HostedProgram<P>, roots: StartupRoots, appends: StartupAppends) {
        assert!(program.instances > 0 && program.operations > 0, "a hosted program has room to run");
        assert!(!self.programs.iter().any(|entry| entry.program == program.program), "one factory per program");
        self.programs.push(program);
        self.startup.push(Startup { roots, appends });
    }

    /// Adds a root host; descriptors opened at startup may be declared with `spawn_with_fds`.
    pub fn spawn<F: FnOnce() -> P>(&mut self, make: F) -> usize {
        self.spawn_with_fds(Vec::new(), make)
    }

    /// Adds a root host with descriptors it inherits and owns until close or exit.
    pub fn spawn_with_fds<F: FnOnce() -> P>(&mut self, fds: Vec<Fd>, make: F) -> usize {
        let host = self.procs.len();
        let proc = match &mut self.heap {
            Some(heap) => heap.admit(make, P::worst_case),
            None => make(),
        };
        self.procs.push(proc);
        self.inherited.push(fds);
        host
    }

    /// Opens one kernel with room for every root, hosted instance and teardown operation.
    #[must_use]
    pub fn run(self, clock: &Clock, patience: Duration) -> Outcome<P> {
        assert!(!self.procs.is_empty(), "a real loop runs at least one process");
        let limits: Vec<u32> = self.procs.iter().map(Host::operations).collect();
        let roots = limits.iter().try_fold(0_u32, |sum, limit| sum.checked_add(*limit)).expect("root operations fit");
        let hosted = self
            .programs
            .iter()
            .try_fold(0_u32, |sum, program| sum.checked_add(program.operations.checked_mul(program.instances)?))
            .expect("hosted operations fit");
        // One cancellation for every outstanding hosted operation, plus one
        // close, ensures a killed child can settle even at its admission limit.
        let operations = roots
            .checked_add(hosted.checked_mul(2).expect("cleanup operations fit"))
            .and_then(|sum| sum.checked_add(1))
            .expect("world operations fit");
        let ring = match self.backend {
            Some(backend) => Ring::new_on(operations, limits, backend).unwrap_or_else(|error| {
                crate::fail(&format!(
                    "forced backend {backend:?} is not usable here, so the real loop cannot run: {error}"
                ))
            }),
            None => Ring::new(operations, limits).unwrap_or_else(|error| {
                crate::fail(&format!("the host's backend is not usable here, so the real loop cannot run: {error}"))
            }),
        };
        let ids: Vec<usize> = (0..self.procs.len()).collect();
        let living_hosts = ids.iter().copied().collect();
        let positions = ids.iter().copied().map(Some).collect();
        let root_signals = self.signals.keys().copied().collect();
        let instances = vec![0; self.programs.len()];
        let processes = self
            .inherited
            .iter()
            .map(|fds| Process {
                fds: fds.iter().copied().collect(),
                exit: None,
                cleanup: 0,
                queued_cleanup: 0,
                cleaned: false,
                pending_recent: 0,
            })
            .collect();
        Running {
            world: self,
            ring,
            ids,
            positions,
            living_hosts,
            root_signals,
            instances,
            settling_children: BTreeSet::new(),
            unsettled_children: 0,
            submits: Queue::with_capacity(operations),
            processes,
            children: BTreeMap::new(),
            placeholders: BTreeMap::new(),
            waiting_children: BTreeMap::new(),
            housekeeping: BTreeMap::new(),
            cleanup: VecDeque::new(),
            killed: Vec::new(),
            dying: BTreeSet::new(),
        }
        .run(clock, patience)
    }
}

struct Process {
    fds: BTreeSet<Fd>,
    exit: Option<Exit>,
    cleanup: u32,
    queued_cleanup: u32,
    cleaned: bool,
    // One bit per completed pass, newest low: a fixed eight-pass window.
    pending_recent: u8,
}

impl Process {
    fn submitted(&mut self, operation: &Op) {
        // Close releases ownership at submission. Its completion may arrive
        // after another operation returned a newly reused descriptor number.
        if let Op::Close { fd } = operation {
            self.fds.remove(fd);
        }
    }

    fn completed(&mut self, complete: &Complete) {
        match &complete.result {
            Ok(Done::Fd(fd) | Done::Accepted { fd, .. } | Done::Spawned { pidfd: fd, .. }) => {
                self.fds.insert(*fd);
            }
            Ok(
                Done::Nothing
                | Done::Count(_)
                | Done::Bound(_)
                | Done::Stat(_)
                | Done::Exit(_)
                | Done::Usage(_)
                | Done::Signalled(_)
                | Done::Window(_),
            )
            | Err(_) => {}
        }
        if let Op::Spawn { spawn } = &complete.kind {
            for pipe in &spawn.pipes {
                if let Some(fd) = pipe.parent {
                    self.fds.insert(fd);
                }
            }
        }
    }
}

struct Child {
    host: usize,
    program: usize,
    signal_writer: Fd,
    exit: Option<Exit>,
    waiters: Vec<Submit>,
    placeholder_open: bool,
}

struct Running<P, R> {
    world: World<P, R>,
    ring: Ring,
    ids: Vec<usize>,
    // Stable host identities name the ledger; positions follow surviving hosts.
    positions: Vec<Option<usize>>,
    // Roots keep running; killed children stay here until their buffers return.
    living_hosts: BTreeSet<usize>,
    root_signals: Vec<usize>,
    instances: Vec<usize>,
    // Only children with a process exit need settlement checks.
    settling_children: BTreeSet<usize>,
    unsettled_children: usize,
    submits: Queue<Submit>,
    processes: Vec<Process>,
    children: BTreeMap<usize, Child>,
    placeholders: BTreeMap<Fd, usize>,
    waiting_children: BTreeMap<Token, usize>,
    housekeeping: BTreeMap<Token, usize>,
    cleanup: VecDeque<(usize, Op)>,
    killed: Vec<Killed>,
    dying: BTreeSet<usize>,
}

fn thread_cpu_nanos() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    // Linux scheduler/sched-stats documents field one in nanoseconds;
    // thread-self selects this thread's scheduler accounting.
    let source = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    let mut fields = source.split_whitespace();
    let runtime = fields.next()?.parse::<u64>().ok()?;
    let _delay = fields.next()?.parse::<u64>().ok()?;
    let _slices = fields.next()?.parse::<u64>().ok()?;
    // Disabled accounting emits zeros; a saturated counter has no usable delta.
    if fields.next().is_some() || runtime == 0 || runtime == u64::MAX {
        return None;
    }
    Some(runtime)
}

fn deadline_diagnostic(patience: Duration, iterations: u32, thread_cpu_start: Option<u64>) -> String {
    let elapsed = thread_cpu_nanos().zip(thread_cpu_start).and_then(|(end, start)| end.checked_sub(start));
    format!(
        "the real loop settles within {} ms; iterations (including failing iteration): {iterations}; thread CPU elapsed (schedstat): {}",
        patience.as_nanos().div_euclid(1_000_000),
        match elapsed {
            Some(nanos) => format!("{nanos} ns"),
            None => String::from("UNAVAILABLE (ns)"),
        }
    )
}

fn deadline_status(deadline: Option<Time>, now: Time) -> String {
    match deadline {
        Some(at) if at <= now => {
            format!("Some({} ns) (due {} ns ago)", at.as_nanos(), now.saturating_since(at).as_nanos())
        }
        Some(at) => format!("Some({} ns) (in {} ns)", at.as_nanos(), at.saturating_since(now).as_nanos()),
        None => String::from("None"),
    }
}

impl<P: Host, R: Referee<P>> Running<P, R> {
    fn deadline_diagnostic(
        &self,
        patience: Duration,
        iterations: u32,
        thread_cpu_start: Option<u64>,
        now: Time,
        last_word: Option<Time>,
    ) -> String {
        let mut text = deadline_diagnostic(patience, iterations, thread_cpu_start);
        writeln!(&mut text, "\nnow: {} ns", now.as_nanos()).expect("String formatting");
        let word = match last_word {
            Some(at) => format!("Some({} ns)", at.as_nanos()),
            None => String::from("None"),
        };
        writeln!(
            &mut text,
            "referee: passed={}; last_word={word}; next_deadline={}",
            self.world.referee.passed(),
            deadline_status(self.world.referee.next_deadline(), now),
        )
        .expect("String formatting");
        for host in &self.living_hosts {
            let proc = self.proc(*host);
            let recent = self.processes.get(*host).expect("a process per host").pending_recent.count_ones();
            writeln!(
                &mut text,
                "host {host}: work_pending={}; next_deadline={}; next_policy_deadline={}; is_empty={}; ring_count={}; work_pending_last_8={recent}/8",
                proc.work_pending(now),
                deadline_status(proc.next_deadline(), now),
                deadline_status(proc.next_policy_deadline(), now),
                proc.is_empty(),
                self.ring.counts.get(*host).expect("a ring count per host"),
            ).expect("String formatting");
        }
        text
    }

    fn work_pending(&mut self, now: Time) -> bool {
        // Sample every living host, including those after the first pending one.
        let mut any = false;
        for host in &self.living_hosts {
            let pending = self.proc(*host).work_pending(now);
            let recent = &mut self.processes.get_mut(*host).expect("a process per host").pending_recent;
            *recent = recent.wrapping_shl(1) | u8::from(pending);
            any |= pending;
        }
        any
    }

    fn run(mut self, clock: &Clock, patience: Duration) -> Outcome<P> {
        let start = clock.now().now;
        let deadline = start.saturating_add(patience);
        let thread_cpu_start = thread_cpu_nanos();
        let mut iterations: u32 = 0;
        let mut last_word = None;
        let mut policy_wake: Vec<(usize, Time)> = Vec::new();
        loop {
            iterations = iterations.checked_add(1).expect("fewer than 2^32 iterations");
            let Now { now, wall } = clock.now();
            check_policy_wake(&mut policy_wake, now);
            if now >= deadline {
                crate::fail(&self.deadline_diagnostic(patience, iterations, thread_cpu_start, now, last_word));
            }
            self.world.referee.act(now, &mut self.world.procs);
            self.signals();
            self.reap();
            let mut at = 0;
            let mut observed = false;
            while at < self.world.procs.len() {
                let host = *self.ids.get(at).expect("an id per host");
                let mut ran = self.processes.get(host).expect("a process per host").exit.is_none();
                if ran {
                    let proc = self.world.procs.get_mut(at).expect("a process per host");
                    self.ring.deliver(host, proc.completions());
                    match &mut self.world.heap {
                        Some(heap) => heap.around(at, || {
                            proc.iterate(now, wall);
                            proc.drain();
                        }),
                        None => {
                            proc.iterate(now, wall);
                            proc.drain();
                        }
                    }
                    while let Some(record) = proc.submissions().pop() {
                        self.submits.push(record);
                    }
                    while let Some(record) = self.submits.pop() {
                        let record = self.ring.register(host, record);
                        self.submit(record);
                    }
                    self.check_exit(at, host);
                }
                if self.dying.contains(&host) {
                    self.discard(at, host);
                    ran = true;
                }
                if self.ids.get(at) == Some(&host) {
                    at = at.checked_add(1).expect("bounded host count");
                }
                if ran {
                    self.world.referee.observe(now, &self.world.procs);
                    observed = true;
                }
            }
            if !observed {
                // Even a pass with only retained exits observes what act did.
                self.world.referee.observe(now, &self.world.procs);
            }
            if last_word.is_none() && self.world.referee.passed() {
                last_word = Some(self.world.referee.last_word().at(now));
            }
            self.settle_children();
            self.settle_root_signals();
            self.submit_cleanup();
            self.ring.enter(Wait::No);
            if self.work_pending(now) || self.ring.has_ready() || !self.world.controls.signals.borrow().is_empty() {
                continue;
            }
            if self.living_hosts.iter().all(|host| self.proc(*host).is_empty())
                && self.ring.is_empty()
                && self.cleanup.is_empty()
                && self.unsettled_children == 0
                && self.placeholders.is_empty()
                && self.world.referee.passed()
            {
                assert!(
                    self.processes.iter().all(|process| process.fds.is_empty()),
                    "settled hosts leave no owned descriptor open"
                );
                return Outcome {
                    heap: self.world.heap.as_ref().map(Heap::report),
                    held: self.world.heap.as_ref().map(Heap::held),
                    procs: self.world.procs,
                    killed: self.killed,
                    iterations,
                    start,
                    end: now,
                };
            }
            if let Some(why) = self.world.referee.overdue(now) {
                crate::fail(&format!(
                    "at {} ms, the referee failed the real loop:\n{why}",
                    now.saturating_since(start).as_nanos().div_euclid(1_000_000)
                ));
            }
            let until = self.wait_until(deadline);
            self.policy_wake(last_word, &mut policy_wake);
            self.ring.enter(Wait::Until(until));
        }
    }

    fn check_exit(&mut self, at: usize, host: usize) {
        // Killed hosts remain admitted until their records return.
        if self.ids.get(at) == Some(&host) {
            let proc = self.world.procs.get(at).expect("a process per host");
            if self.children.contains_key(&host)
                && let Some(exit) = proc.exit()
            {
                assert!(proc.is_empty(), "normal hosted exit releases every entity");
                self.processes.get_mut(host).expect("a process per host").exit = Some(exit);
                self.living_hosts.remove(&host);
                self.settling_children.insert(host);
            }
        }
    }

    fn wait_until(&self, patience: Time) -> Time {
        self.living_hosts
            .iter()
            .map(|host| self.proc(*host).next_deadline())
            .chain([self.world.referee.next_deadline()])
            .flatten()
            .fold(patience, Time::min)
    }

    fn proc(&self, host: usize) -> &P {
        let at = self.positions.get(host).expect("a position per host").expect("a living host remains admitted");
        self.world.procs.get(at).expect("a process per position")
    }

    fn policy_wake(&self, last_word: Option<Time>, deadlines: &mut Vec<(usize, Time)>) {
        if let Some(last_word) = last_word {
            for host in &self.living_hosts {
                let proc = self.proc(*host);
                if !proc.is_empty()
                    && let Some(deadline) = proc.next_policy_deadline()
                    && deadline > last_word
                {
                    deadlines.push((*host, deadline));
                }
            }
        }
    }

    fn signals(&mut self) {
        loop {
            let Some((host, signal)) = self.world.controls.signals.borrow_mut().pop() else { break };
            assert!(
                self.processes.get(host).expect("a signal names an admitted host").exit.is_none(),
                "a signal names a running service"
            );
            match self.world.signals.get(&host).expect("a service declares its signal source") {
                SignalSource::Pipe(writer) => {
                    skein_shell::write_service_signal(*writer, signal).expect("deliver the referee signal");
                }
                SignalSource::Real => {
                    skein_shell::signal_current_thread(signal).expect("deliver the real termination signal");
                }
            }
        }
    }

    fn settle_root_signals(&mut self) {
        let mut at = 0;
        while let Some(host) = self.root_signals.get(at).copied() {
            if self.proc(host).is_empty() && self.ring.count(host) == 0 {
                if let Some(SignalSource::Pipe(writer)) = self.world.signals.remove(&host) {
                    self.queue_cleanup(host, Op::Close { fd: writer });
                }
                self.root_signals.remove(at);
            } else {
                at = at.checked_add(1).expect("bounded root count");
            }
        }
    }

    fn reap(&mut self) {
        self.ring.kernel.reap(&mut self.ring.arrived);
        while let Some(complete) = self.ring.arrived.pop() {
            if let Some(host) = self.housekeeping.remove(&complete.op) {
                let process = self.processes.get_mut(host).expect("cleanup names a process");
                process.cleanup = process.cleanup.checked_sub(1).expect("cleanup was in flight");
            } else {
                self.answer(complete);
            }
        }
    }

    fn answer(&mut self, complete: Complete) {
        let (host, complete) = self.ring.complete(complete);
        let process = self.processes.get_mut(host).expect("a process per host");
        process.completed(&complete);
        self.ring.returned(host, complete);
    }

    fn synthetic(&mut self, record: Submit, result: Result<Done, Error>) {
        self.answer(Complete { op: record.op, kind: record.kind, result });
    }

    fn submit(&mut self, mut record: Submit) {
        assert!(record.kind.is_valid(), "only valid records cross the kernel boundary");
        let host = self.ring.bindings.get(&record.op).expect("a registered operation").host;
        self.processes.get_mut(host).expect("an operation owner").submitted(&record.kind);
        match &mut record.kind {
            Op::Spawn { spawn } => {
                if let Some(program) = self.world.programs.iter().position(|entry| entry.program == spawn.program) {
                    let result = self.spawn(host, program, spawn);
                    self.synthetic(record, result);
                    return;
                }
            }
            Op::Wait { pidfd, .. } => {
                if let Some(child) = self.placeholders.get(pidfd).copied().and_then(|host| self.children.get_mut(&host))
                {
                    match child.exit {
                        Some(exit) => self.synthetic(record, Ok(Done::Exit(exit))),
                        None => {
                            self.waiting_children.insert(record.op, child.host);
                            child.waiters.push(record);
                        }
                    }
                    return;
                }
            }
            Op::Signal { pidfd, signal, .. } => {
                if let Some(child) = self.placeholders.get(pidfd).and_then(|host| self.children.get(host)) {
                    let child_host = child.host;
                    let writer = child.signal_writer;
                    let result = if child.exit.is_some()
                        || self.processes.get(child_host).expect("child process").exit.is_some()
                    {
                        Err(Error::Other(3))
                    } else {
                        match signal {
                            Signal::Interrupt => skein_shell::write_service_signal(writer, ServiceSignal::Interrupt)
                                .map(|()| Done::Nothing),
                            Signal::Terminate => skein_shell::write_service_signal(writer, ServiceSignal::Terminate)
                                .map(|()| Done::Nothing),
                            Signal::Stop | Signal::Continue => Err(Error::InvalidArgument),
                            Signal::Kill => {
                                self.kill(child_host);
                                Ok(Done::Nothing)
                            }
                        }
                    };
                    self.synthetic(record, result);
                    return;
                }
            }
            Op::Close { fd } => {
                if let Some(host) = self.placeholders.remove(fd) {
                    let child = self.children.get_mut(&host).expect("registered placeholder");
                    assert!(child.placeholder_open, "a placeholder closes once");
                    child.placeholder_open = false;
                    // A placeholder is a real descriptor, released by a real ring close.
                }
            }
            Op::Cancel { target } => {
                if let Some(waiter) = self.take_waiter(*target) {
                    self.synthetic(waiter, Err(Error::Cancelled));
                    self.synthetic(record, Ok(Done::Nothing));
                    return;
                }
            }
            Op::Socket { .. }
            | Op::Bind { .. }
            | Op::Listen { .. }
            | Op::Accept { .. }
            | Op::Connect { .. }
            | Op::Recv { .. }
            | Op::Send { .. }
            | Op::Shutdown { .. }
            | Op::Open { .. }
            | Op::Read { .. }
            | Op::Write { .. }
            | Op::Append { .. }
            | Op::Truncate { .. }
            | Op::Sync { .. }
            | Op::Lock { .. }
            | Op::Stat { .. }
            | Op::Rename { .. }
            | Op::Remove { .. }
            | Op::MakeDirectory { .. }
            | Op::List { .. }
            | Op::Usage
            | Op::Window { .. }
            | Op::ReadSignal { .. }
            | Op::PipeRead { .. }
            | Op::PipeWrite { .. } => {}
        }
        self.ring.submits.push(record);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "hosted startup opens, rollback and child ownership transfer stay together (world.md, 3)"
    )]
    fn spawn(&mut self, parent: usize, program: usize, spawn: &mut Spawn) -> Result<Done, Error> {
        let entry = self.world.programs.get(program).expect("a registered program");
        let active = *self.instances.get(program).expect("a count per program");
        assert!(
            active < usize::try_from(entry.instances).expect("instances fit"),
            "hosted instances fit their provision"
        );
        let make = entry.make;
        let operations = entry.operations;
        let startup = self.world.startup.get(program).expect("startup selector per program");
        let declarations = (startup.roots)(spawn);
        let append_declarations = (startup.appends)(spawn);
        crate::program::check_appends(&append_declarations);
        crate::program::check_roots(&declarations);
        let mut roots = Vec::new();
        for declaration in declarations {
            let path = std::path::Path::new(std::ffi::OsStr::from_bytes(&declaration.path));
            match skein_shell::open_root(path) {
                Ok(fd) => roots.push((declaration.name, fd)),
                Err(errno) => {
                    for (_, fd) in roots {
                        self.queue_cleanup(parent, Op::Close { fd });
                    }
                    return Err(match errno {
                        2 => Error::NotFound,
                        13 => Error::Permission,
                        20 => Error::NotADirectory,
                        22 => Error::InvalidArgument,
                        24 => Error::TooManyOpenFiles,
                        _ => Error::Other(errno),
                    });
                }
            }
        }
        let mut appends = Vec::new();
        for declaration in append_declarations {
            match open_startup_append(&declaration) {
                Ok(fd) => appends.push((declaration.name, fd)),
                Err(error) => {
                    for (_, fd) in roots.into_iter().chain(appends) {
                        self.queue_cleanup(parent, Op::Close { fd });
                    }
                    return Err(error);
                }
            }
        }
        let path = if spawn.dir.is_empty() { b".".as_slice() } else { spawn.dir.as_ref() };
        let dir = match skein_shell::open_beneath(spawn.root, path) {
            Ok(fd) => fd,
            Err(error) => {
                for (_, fd) in roots.into_iter().chain(appends) {
                    self.queue_cleanup(parent, Op::Close { fd });
                }
                return Err(error);
            }
        };
        match skein_shell::hosted_pipes(spawn) {
            Ok(pipes) => {
                let inherited = Inherited { pipes: pipes.pipes, roots, appends, dir: Some(dir), signal: pipes.signal };
                let proc = match &mut self.world.heap {
                    Some(heap) => heap.admit(|| make(spawn, &inherited), P::worst_case),
                    None => make(spawn, &inherited),
                };
                assert!(proc.operations() <= operations, "hosted operations fit their provision");
                let child = self.ring.admit(proc.operations());
                self.world.procs.push(proc);
                self.ids.push(child);
                self.positions.push(Some(self.world.procs.len().checked_sub(1).expect("an admitted child")));
                self.living_hosts.insert(child);
                self.unsettled_children = self.unsettled_children.checked_add(1).expect("bounded child count");
                let active = self.instances.get_mut(program).expect("a count per program");
                *active = active.checked_add(1).expect("bounded instance count");
                self.processes.push(Process {
                    fds: inherited
                        .pipes
                        .iter()
                        .map(|(_, fd)| *fd)
                        .chain(inherited.roots.iter().map(|(_, fd)| *fd))
                        .chain(inherited.appends.iter().map(|(_, fd)| *fd))
                        .chain(inherited.dir)
                        .chain([inherited.signal])
                        .collect(),
                    exit: None,
                    cleanup: 0,
                    queued_cleanup: 0,
                    cleaned: false,
                    pending_recent: 0,
                });
                self.world.signals.insert(child, SignalSource::Pipe(pipes.signal_writer));
                self.placeholders.insert(pipes.pidfd, child);
                self.children.insert(
                    child,
                    Child {
                        host: child,
                        program,
                        signal_writer: pipes.signal_writer,
                        exit: None,
                        waiters: Vec::new(),
                        placeholder_open: true,
                    },
                );
                Ok(Done::Spawned { pidfd: pipes.pidfd, sender: hosted_sender(child) })
            }
            Err(error) => {
                self.queue_cleanup(parent, Op::Close { fd: dir });
                for (_, fd) in roots.into_iter().chain(appends) {
                    self.queue_cleanup(parent, Op::Close { fd });
                }
                Err(error)
            }
        }
    }

    fn kill(&mut self, host: usize) {
        let exit = Exit::Signal(9);
        self.processes.get_mut(host).expect("child process").exit = Some(exit);
        assert!(self.dying.insert(host), "a hosted child is killed once");
        self.settling_children.insert(host);
        // Keep the child until every submitted buffer returns. Returned records
        // are discarded under that child's meter, never by the backend.
        let targets: Vec<Token> =
            self.ring.bindings.iter().filter_map(|(token, binding)| (binding.host == host).then_some(*token)).collect();
        for target in targets {
            if let Some(waiter) = self.take_waiter(target) {
                self.synthetic(waiter, Err(Error::Cancelled));
            } else {
                self.queue_cleanup(host, Op::Cancel { target });
            }
        }
    }

    fn take_waiter(&mut self, token: Token) -> Option<Submit> {
        let host = self.waiting_children.remove(&token)?;
        let child = self.children.get_mut(&host).expect("a waiter names its child");
        let at = child.waiters.iter().position(|waiter| waiter.op == token).expect("an indexed waiter");
        Some(child.waiters.remove(at))
    }

    fn discard(&mut self, at: usize, host: usize) {
        let proc = self.world.procs.get_mut(at).expect("a dying process");
        self.ring.deliver(host, proc.completions());
        let mut discard = || {
            while let Some(complete) = proc.completions().pop() {
                drop(complete);
            }
        };
        match &mut self.world.heap {
            Some(heap) => heap.around(at, discard),
            None => discard(),
        }
    }

    fn release_killed(&mut self, host: usize, exit: Exit) {
        let at =
            self.positions.get_mut(host).expect("a position per host").take().expect("a dying child remains admitted");
        self.living_hosts.remove(&host);
        let proc = self.world.procs.remove(at);
        self.ids.remove(at);
        for (at, host) in self.ids.iter().enumerate().skip(at) {
            *self.positions.get_mut(*host).expect("a position per host") = Some(at);
        }
        let heap = match &mut self.world.heap {
            Some(heap) => Some(heap.release(at, proc)),
            None => {
                drop(proc);
                None
            }
        };
        self.killed.push(Killed { host, exit, heap });
    }

    fn settle_children(&mut self) {
        let mut after = None;
        loop {
            let next = match after {
                Some(host) => {
                    self.settling_children.range((core::ops::Bound::Excluded(host), core::ops::Bound::Unbounded)).next()
                }
                None => self.settling_children.first(),
            };
            let Some(host) = next.copied() else { break };
            after = Some(host);
            let process = self.processes.get_mut(host).expect("child process");
            let exit = process.exit.expect("only exited children settle");
            if self.ring.count(host) != 0 {
                continue;
            }
            if !process.cleaned {
                let fds = core::mem::take(&mut process.fds);
                process.cleaned = true;
                for fd in fds {
                    self.queue_cleanup(host, Op::Close { fd });
                }
                let child = self.children.get(&host).expect("registered child");
                self.queue_cleanup(host, Op::Close { fd: child.signal_writer });
            }
            let process = self.processes.get(host).expect("child process");
            if process.cleanup != 0 || process.queued_cleanup != 0 {
                continue;
            }
            if self.dying.remove(&host) {
                self.release_killed(host, exit);
            }
            let child = self.children.get_mut(&host).expect("registered child");
            child.exit = Some(exit);
            let active = self.instances.get_mut(child.program).expect("a count per program");
            *active = active.checked_sub(1).expect("a settled child was active");
            self.unsettled_children = self.unsettled_children.checked_sub(1).expect("a child settles once");
            self.settling_children.remove(&host);
            self.world.signals.remove(&host);
            let waiters = core::mem::take(&mut child.waiters);
            for waiter in waiters {
                self.waiting_children.remove(&waiter.op).expect("an indexed waiter");
                self.synthetic(waiter, Ok(Done::Exit(exit)));
            }
        }
    }

    fn queue_cleanup(&mut self, host: usize, kind: Op) {
        let process = self.processes.get_mut(host).expect("cleanup owner");
        process.queued_cleanup = process.queued_cleanup.checked_add(1).expect("cleanup operations fit");
        self.cleanup.push_back((host, kind));
    }

    fn submit_cleanup(&mut self) {
        let room =
            self.ring.kernel.room().checked_sub(self.ring.submits.len()).expect("pending submissions fit the ring");
        for _ in 0..room {
            let Some((host, kind)) = self.cleanup.pop_front() else { break };
            if let Op::Close { fd } = &kind
                && let Some(child_host) = self.placeholders.remove(fd)
            {
                self.children.get_mut(&child_host).expect("registered placeholder").placeholder_open = false;
            }
            let op = self.ring.fresh();
            self.housekeeping.insert(op, host);
            let process = self.processes.get_mut(host).expect("cleanup owner");
            process.queued_cleanup = process.queued_cleanup.checked_sub(1).expect("cleanup was queued");
            process.cleanup = process.cleanup.checked_add(1).expect("cleanup operations fit");
            self.ring.submits.push(Submit { op, kind });
        }
    }
}

fn open_startup_append(declaration: &crate::StartupAppend) -> Result<Fd, Error> {
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(&declaration.root));
    let root = skein_shell::open_root(path).map_err(|errno| match errno {
        2_i32 => Error::NotFound,
        13_i32 => Error::Permission,
        20_i32 => Error::NotADirectory,
        22_i32 => Error::InvalidArgument,
        24_i32 => Error::TooManyOpenFiles,
        _ => Error::Other(errno),
    })?;
    let opened = skein_shell::open_append(root, &declaration.path, declaration.mode);
    skein_shell::close_keeper_fd(root);
    opened
}

/// Checks deadlines kept before the wait, before a host can fire and clear them.
fn check_policy_wake(deadlines: &mut Vec<(usize, Time)>, now: Time) {
    for (process, deadline) in deadlines.drain(..) {
        if now >= deadline {
            crate::fail(&format!(
                "process {process} woke for policy deadline {} ns after the last word",
                deadline.as_nanos()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Child, Process, Running, World};
    use crate::{Host, Referee, ring::Ring};
    use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
    use alloc::rc::Rc;
    use core::cell::RefCell;
    use skein_io::kernel::{Complete, Done, Error, Exit, Family, Fd, Op, Submit};
    use skein_lib::{Queue, Time, Token, Wall};

    struct Idle {
        completions: Queue<Complete>,
        submissions: Queue<Submit>,
        iterations: u32,
        marked: bool,
    }

    impl Host for Idle {
        fn iterate(&mut self, _now: Time, _wall: Wall) {
            self.iterations = self.iterations.checked_add(1).expect("bounded test iterations");
        }
        fn completions(&mut self) -> &mut Queue<Complete> {
            &mut self.completions
        }
        fn submissions(&mut self) -> &mut Queue<Submit> {
            &mut self.submissions
        }
        fn work_pending(&self, _now: Time) -> bool {
            false
        }
        fn next_deadline(&self) -> Option<Time> {
            None
        }
        fn next_policy_deadline(&self) -> Option<Time> {
            None
        }
        fn is_empty(&self) -> bool {
            true
        }
        fn worst_case(&self) -> u64 {
            0
        }
        fn operations(&self) -> u32 {
            2
        }
    }

    struct Quiet;

    impl Referee<Idle> for Quiet {
        fn act(&mut self, _now: Time, _procs: &mut [Idle]) {}
        fn observe(&mut self, _now: Time, _procs: &[Idle]) {}
        fn next_deadline(&self) -> Option<Time> {
            None
        }
        fn overdue(&self, _now: Time) -> Option<String> {
            None
        }
        fn passed(&self) -> bool {
            true
        }
    }

    fn children_after_descriptor_cleanup() -> Running<Idle, Quiet> {
        children_with(Quiet)
    }

    fn children_with<R: Referee<Idle>>(referee: R) -> Running<Idle, R> {
        let mut world = World::new(referee);
        for _ in 0_usize..3 {
            world.spawn(|| Idle {
                completions: Queue::with_capacity(2),
                submissions: Queue::with_capacity(2),
                iterations: 0,
                marked: false,
            });
        }
        Running {
            world,
            ring: Ring::new(8, vec![2; 3]).expect("test ring"),
            ids: vec![0, 1, 2],
            positions: vec![Some(0), Some(1), Some(2)],
            living_hosts: [0, 1, 2].into_iter().collect(),
            root_signals: Vec::new(),
            instances: vec![2],
            settling_children: BTreeSet::new(),
            unsettled_children: 2,
            submits: Queue::with_capacity(8),
            processes: (0..3)
                .map(|_| Process {
                    fds: BTreeSet::new(),
                    exit: None,
                    cleanup: 0,
                    queued_cleanup: 0,
                    cleaned: true,
                    pending_recent: 0,
                })
                .collect(),
            children: (1..3)
                .map(|host| {
                    (
                        host,
                        Child {
                            host,
                            program: 0,
                            signal_writer: Fd::new(-1),
                            exit: None,
                            waiters: Vec::new(),
                            placeholder_open: false,
                        },
                    )
                })
                .collect(),
            placeholders: BTreeMap::new(),
            waiting_children: BTreeMap::new(),
            housekeeping: BTreeMap::new(),
            cleanup: VecDeque::new(),
            killed: Vec::new(),
            dying: BTreeSet::new(),
        }
    }

    struct Observations(Rc<RefCell<Vec<Vec<u32>>>>);

    impl Referee<Idle> for Observations {
        fn act(&mut self, _now: Time, procs: &mut [Idle]) {
            for proc in procs {
                proc.marked = true;
            }
        }
        fn observe(&mut self, _now: Time, procs: &[Idle]) {
            assert!(procs.iter().all(|proc| proc.marked), "act is visible to observe");
            self.0.borrow_mut().push(procs.iter().map(|proc| proc.iterations).collect());
        }
        fn next_deadline(&self) -> Option<Time> {
            None
        }
        fn overdue(&self, _now: Time) -> Option<String> {
            None
        }
        fn passed(&self) -> bool {
            !self.0.borrow().is_empty()
        }
    }

    fn retain_exited(running: &mut Running<Idle, Observations>, host: usize) {
        running.processes.get_mut(host).expect("test process").exit = Some(Exit::Code(0));
        running.living_hosts.remove(&host);
        if let Some(child) = running.children.get_mut(&host) {
            child.exit = Some(Exit::Code(0));
            running.unsettled_children = running.unsettled_children.checked_sub(1).expect("live test child");
            let instances = running.instances.first_mut().expect("test program");
            *instances = instances.checked_sub(1).expect("live test instance");
        }
    }

    #[test]
    fn observes_a_drive_but_not_retained_exited_positions() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut running = children_with(Observations(seen.clone()));
        retain_exited(&mut running, 1);
        retain_exited(&mut running, 2);
        let outcome = running.run(&super::Clock::new(), skein_lib::Duration::from_secs(1));
        assert_eq!(outcome.iterations, 1);
        assert_eq!(*seen.borrow(), [vec![1_u32, 0_u32, 0_u32]]);
    }

    #[test]
    fn observes_act_once_when_only_retained_exits_remain() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut running = children_with(Observations(seen.clone()));
        for host in 0_usize..3 {
            retain_exited(&mut running, host);
        }
        let outcome = running.run(&super::Clock::new(), skein_lib::Duration::from_secs(1));
        assert_eq!(outcome.iterations, 1);
        assert_eq!(*seen.borrow(), [vec![0_u32, 0_u32, 0_u32]]);
    }

    #[test]
    fn observes_a_discard_even_without_a_drive() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut running = children_with(Observations(seen.clone()));
        retain_exited(&mut running, 2);
        running.kill(1);
        let outcome = running.run(&super::Clock::new(), skein_lib::Duration::from_secs(1));
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.killed.len(), 1);
        assert_eq!(*seen.borrow(), [vec![1_u32, 0_u32, 0_u32], vec![1_u32, 0_u32, 0_u32]]);
    }

    #[test]
    fn simulated_empty_pass_observes_before_testing_passed() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let world = crate::World::<Idle, _>::new(
            7,
            skein_sim::Config::calm(),
            Observations(seen.clone()),
            crate::Memory::Unchecked,
        );
        let outcome = world.run();
        assert_eq!(outcome.iterations, 1);
        assert_eq!(*seen.borrow(), [Vec::<u32>::new()]);
    }

    #[test]
    fn releasing_a_killed_child_keeps_the_other_childs_position_and_instance() {
        let mut running = children_after_descriptor_cleanup();
        running.kill(1);
        running.settle_children();
        assert_eq!(running.ids, [0, 2]);
        assert_eq!(running.positions, [Some(0), None, Some(1)]);
        assert_eq!(running.living_hosts.into_iter().collect::<Vec<_>>(), [0, 2]);
        assert_eq!(running.instances, [1]);
        assert_eq!(running.unsettled_children, 1);
        assert!(running.children.get(&2).expect("living child").exit.is_none(), "the other child remains live");
        assert_eq!(running.killed.first().expect("released child").host, 1);
    }

    #[test]
    fn another_hosts_queued_cleanup_does_not_hold_a_settling_child() {
        let mut running = children_after_descriptor_cleanup();
        running.kill(1);
        running.kill(2);
        running.queue_cleanup(2, Op::Close { fd: Fd::new(-1) });
        running.settle_children();
        assert_eq!(running.killed.len(), 1);
        assert_eq!(running.killed.first().expect("released child").host, 1);
        assert_eq!(running.children.get(&1).expect("settled child").exit, Some(Exit::Signal(9)));
        assert!(
            running.children.get(&2).expect("pending child").exit.is_none(),
            "its own cleanup holds the second child"
        );
        assert_eq!(running.processes.get(2).expect("cleanup owner").queued_cleanup, 1);
        running.submit_cleanup();
        let process = running.processes.get(2).expect("cleanup owner");
        assert_eq!((process.queued_cleanup, process.cleanup), (0, 1));
        running.settle_children();
        assert_eq!(running.killed.len(), 1, "submitted cleanup still belongs to the second child");
    }

    #[test]
    fn cancelling_and_killing_waiters_keep_the_other_hosts_local_token() {
        let mut running = children_after_descriptor_cleanup();
        let pidfd = Fd::new(41);
        running.placeholders.insert(pidfd, 2);
        running.children.get_mut(&2).expect("waited child").placeholder_open = true;
        let local = Token::new(10);
        for host in [0, 1] {
            let record = running.ring.register(host, Submit { op: local, kind: Op::Wait { pidfd, reap: false } });
            running.submit(record);
        }
        let cancel = Token::new(11);
        let record = running.ring.register(0, Submit { op: cancel, kind: Op::Cancel { target: local } });
        running.submit(record);
        assert_eq!(running.waiting_children.len(), 1);
        assert_eq!(running.children.get(&2).expect("waited child").waiters.len(), 1);
        let mut returned = Queue::with_capacity(2);
        running.ring.deliver(0, &mut returned);
        let terminal = returned.pop().expect("cancelled wait");
        assert_eq!(terminal.op, local);
        assert_eq!(terminal.result, Err(Error::Cancelled));
        let terminal = returned.pop().expect("cancel answer");
        assert_eq!(terminal.op, cancel);
        assert_eq!(terminal.kind, Op::Cancel { target: local });
        assert_eq!(terminal.result, Ok(Done::Nothing));
        assert!(!running.ring.has_ready(), "the other wait still waits on its child");
        running.kill(1);
        assert!(running.waiting_children.is_empty(), "kill removes the killed hosts waiter");
        assert!(running.children.get(&2).expect("waited child").waiters.is_empty(), "both waits have terminals");
        assert!(running.ring.has_ready(), "kill returns its cancelled wait before releasing the child");
        running.ring.deliver(1, &mut returned);
        let terminal = returned.pop().expect("killed hosts cancelled wait");
        assert_eq!(terminal.op, local);
        assert_eq!(terminal.result, Err(Error::Cancelled));
        assert!(!running.ring.has_ready(), "every returned record has been delivered");
        running.settle_children();
        assert_eq!(running.killed.first().expect("released child").host, 1);
        assert!(running.children.get(&2).expect("waited child").exit.is_none(), "cancelling its waits leaves it live");
    }

    #[test]
    fn a_late_close_completion_does_not_erase_a_reused_descriptor() {
        let fd = Fd::new(42);
        let mut process = Process {
            fds: [fd].into_iter().collect(),
            exit: None,
            cleanup: 0,
            queued_cleanup: 0,
            cleaned: false,
            pending_recent: 0,
        };
        process.submitted(&Op::Close { fd });
        process.completed(&Complete {
            op: Token::new(2),
            kind: Op::Socket { family: Family::Ipv4 },
            result: Ok(Done::Fd(fd)),
        });
        process.completed(&Complete { op: Token::new(1), kind: Op::Close { fd }, result: Ok(Done::Nothing) });
        assert!(
            process.fds.contains(&fd),
            "kernel.md: completions may arrive in any order; the new descriptor remains owned"
        );
    }
}
