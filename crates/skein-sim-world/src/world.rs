//! The simulated world's state and its steps (world.md).
use crate::{Bare, Finished, Meter, Step, Turns};
mod digests;
mod startup;

pub use digests::{Digests, StateDigests};

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Write};

use skein_io::kernel::{Done, Exit, Fd, Op};
use skein_lib::{Queue, Time};
use skein_sim::{Answer, Ask, Config, Handle, Pid, Program, Reply, Sim};

use crate::Host;
use crate::program::Startup;
use crate::{HostedProgram, Inherited, Machine, NoMachine, StartupAppends, StartupRoot, StartupRoots};

/// How many lines of the trace a failure prints.
const TAIL: usize = 80;

/// Startup descriptors prepared for a spawn before its completion is reaped.
struct PreparedStartup {
    roots: Vec<(Box<[u8]>, Handle)>,
    appends: Vec<(Box<[u8]>, Handle)>,
}

/// How a scenario cuts its processes (simulator.md, 3.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cut {
    /// Cuts one process, keeping every machine operation that completed.
    Kill,
    /// Cuts every process, keeping the seeded machine crash model's state.
    PowerLoss,
}

struct Restart<P> {
    roots: Vec<StartupRoot>,
    appends: Vec<crate::StartupAppend>,
    make: fn(&Inherited) -> P,
}

#[derive(Clone, Copy)]
struct PlannedCut {
    host: usize,
    before: u32,
    cut: Cut,
    done: bool,
}

/// A world (world.md): the simulator, its processes each a `Host`, the
/// machine behind the simulator's seam, the program registry that hosts
/// a spawn, and the clock. It runs no loop of its own: a loop runs its steps.
pub struct World<P, M = NoMachine, H = Bare> {
    seed: u64,
    sim: Sim,
    signals: BTreeMap<usize, (Pid, Fd)>,
    next_host: usize,
    procs: Vec<P>,
    /// Each process's, in the simulator.
    pids: Vec<Pid>,
    meter: H,
    steps: u64,
    printer: crate::printer::Printing,
    programs: Vec<HostedProgram<P>>,
    startup: Vec<Startup>,
    pending_startup: BTreeMap<(Pid, skein_lib::Token), PreparedStartup>,
    hosted: Vec<Option<usize>>,
    finished: Vec<bool>,
    restart: Vec<Option<Restart<P>>>,
    submissions: Vec<u32>,
    cuts: Vec<PlannedCut>,
    machine: M,
    digests: Digests,
    killed: Vec<Killed>,
    calls: Queue<skein_sim::Call>,
    answers: Queue<Answer>,
}

/// Briefly: its seed, and how many processes it hosts.
impl<P, M, H> Debug for World<P, M, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World").field("seed", &self.seed).field("procs", &self.procs.len()).finish_non_exhaustive()
    }
}

/// A terminated hosted child, reported by the world after its resources settle.
#[derive(Debug)]
pub struct Killed {
    pub pid: Pid,
    pub exit: Exit,
    /// Its separate peak and worst case, after exact ownership was checked on drop.
    pub heap: Option<(u64, u64)>,
}

impl<P: Host> World<P> {
    /// Makes a world at time zero, with no machine and no meter.
    #[must_use]
    pub fn new(seed: u64, config: Config) -> World<P> {
        World {
            seed,
            sim: Sim::new(seed, config),
            signals: BTreeMap::new(),
            next_host: 0,
            procs: Vec::new(),
            pids: Vec::new(),
            meter: Bare,
            steps: 0,
            printer: crate::printer::Printing::new(),
            programs: Vec::new(),
            startup: Vec::new(),
            pending_startup: BTreeMap::new(),
            hosted: Vec::new(),
            finished: Vec::new(),
            restart: Vec::new(),
            submissions: Vec::new(),
            cuts: Vec::new(),
            machine: NoMachine,
            digests: Digests::Untraced,
            killed: Vec::new(),
            calls: Queue::with_capacity(256),
            answers: Queue::with_capacity(256),
        }
    }
}

impl<P: Host, H: Meter> World<P, NoMachine, H> {
    /// Supplies the scenario's fake machine before the world starts.
    pub fn with_machine<M: Machine>(self, machine: M) -> World<P, M, H> {
        World {
            seed: self.seed,
            sim: self.sim,
            signals: self.signals,
            next_host: self.next_host,
            procs: self.procs,
            pids: self.pids,
            meter: self.meter,
            steps: self.steps,
            printer: self.printer,
            programs: self.programs,
            startup: self.startup,
            pending_startup: self.pending_startup,
            hosted: self.hosted,
            finished: self.finished,
            restart: self.restart,
            submissions: self.submissions,
            cuts: self.cuts,
            machine,
            digests: self.digests,
            killed: self.killed,
            calls: self.calls,
            answers: self.answers,
        }
    }
}

impl<P: Host, M: Machine, H: Meter> World<P, M, H> {
    /// Supplies a loop's meter before admitting any process.
    #[must_use]
    pub fn with_meter<N: Meter>(self, meter: N) -> World<P, M, N> {
        assert!(self.procs.is_empty(), "the meter owns every process from admission");
        World {
            seed: self.seed,
            sim: self.sim,
            signals: self.signals,
            next_host: self.next_host,
            procs: self.procs,
            pids: self.pids,
            meter,
            steps: self.steps,
            printer: self.printer,
            programs: self.programs,
            startup: self.startup,
            pending_startup: self.pending_startup,
            hosted: self.hosted,
            finished: self.finished,
            restart: self.restart,
            submissions: self.submissions,
            cuts: self.cuts,
            machine: self.machine,
            digests: self.digests,
            killed: self.killed,
            calls: self.calls,
            answers: self.answers,
        }
    }

    /// Registers a hosted program before execution, rejecting duplicate names.
    pub fn host(&mut self, program: HostedProgram<P>) {
        self.host_roots(program, crate::program::no_roots);
    }

    /// Registers a factory and the named directories opened independently for
    /// each launch, before its factory runs (simulator.md, section 3.1).
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

    /// Adds a process with a fake-machine root inherited from startup.
    pub fn spawn_root<F: FnOnce(Fd) -> P>(&mut self, root: Handle, make: F) -> usize {
        let pid = self.sim.spawn_process();
        let root = self.sim.root(pid, root);
        self.admit(pid, None, || make(root))
    }

    /// Adds a process whose factory owns independently opened named roots
    /// and a signal source. A cut restarts it at the same index over these
    /// root paths (simulator.md, 3.3).
    pub fn spawn_restartable(&mut self, roots: Vec<StartupRoot>, make: fn(&Inherited) -> P) -> usize {
        self.spawn_restartable_startup(roots, Vec::new(), make)
    }

    /// Cuts `host` once before its zero-based `before`th submission.
    /// Its factory must be restartable; power loss requires factories for
    /// every process the world started; hosted children stay dead.
    pub fn cut(&mut self, host: usize, before: u32, cut: Cut) {
        assert!(self.restart.get(host).is_some_and(Option::is_some), "a cut needs a restart factory");
        if cut == Cut::PowerLoss {
            assert!(
                self.restart.iter().zip(&self.hosted).all(|(restart, hosted)| restart.is_some() || hosted.is_some()),
                "power loss needs every process's restart factory"
            );
        }
        assert!(!self.cuts.iter().any(|planned| planned.host == host), "a host is cut once");
        self.cuts.push(PlannedCut { host, before, cut, done: false });
    }

    /// Adds a process with an append file the scenario's machine opened at startup.
    pub fn spawn_append<F: FnOnce(Fd) -> P>(&mut self, file: Handle, make: F) -> usize {
        let pid = self.sim.spawn_process();
        let file = self.sim.append(pid, file);
        self.admit(pid, None, || make(file))
    }

    /// Adds the process `make` builds: its heap, from its making on, is the
    /// processes'.
    pub fn spawn<F: FnOnce() -> P>(&mut self, make: F) -> usize {
        let pid = self.sim.spawn_process();
        self.admit(pid, None, make)
    }

    /// Adds a process whose termination records arrive through its simulated signal source.
    pub fn spawn_signals<F: FnOnce(Fd) -> P>(&mut self, make: F) -> usize {
        let pid = self.sim.spawn_process();
        let signal = self.sim.open_signal_source(pid);
        self.signals.insert(self.next_host, (pid, signal));
        self.admit(pid, None, || make(signal))
    }

    fn admit<F: FnOnce() -> P>(&mut self, pid: Pid, hosted: Option<usize>, make: F) -> usize {
        let proc = self.meter.admit(make, P::worst_case);
        self.next_host = self.next_host.checked_add(1).expect("bounded admission count");
        self.pids.push(pid);
        self.procs.push(proc);
        self.hosted.push(hosted);
        self.finished.push(false);
        self.restart.push(None);
        self.submissions.push(0);
        self.procs.len().checked_sub(1).expect("just pushed")
    }

    /// One step, bare: a playground's loop.
    pub fn step(&mut self) -> Step {
        self.step_with(&mut ())
    }

    /// One step with a loop's hooks inside it.
    pub fn step_with<T: Turns<P>>(&mut self, turns: &mut T) -> Step {
        let now = self.now();
        let wall = self.wall();
        let mut at = 0;
        let mut observed = false;
        while at < self.procs.len() {
            let ran;
            if self.hosted.get(at).expect("a status per process").is_some()
                && !*self.finished.get(at).expect("a status per process")
                && !self.sim.service_running(self.pids.get(at).copied().expect("a pid per process"))
            {
                self.drop_killed(at);
                ran = true;
            } else {
                ran = !*self.finished.get(at).expect("a status per process");
                if ran {
                    self.turn(at, now, wall, turns);
                }
                at = at.checked_add(1).expect("a bounded process count");
            }
            if ran {
                turns.turned(now, &self.procs);
                observed = true;
            }
        }
        self.trace_machine_digest();
        self.steps = self.steps.checked_add(1).expect("world's step count fits u64");
        Step { ran: observed }
    }

    /// Checks the settled world's contracts and returns what its steps left.
    #[must_use]
    pub fn finish(self) -> Finished<P, M> {
        assert!(self.cuts.iter().all(|cut| cut.done), "each chosen cut point was reached");
        assert!(self.pending_startup.is_empty(), "all prepared startup descriptors were admitted or rolled back");
        for (proc, pid) in self.procs.iter().zip(&self.pids) {
            assert!(proc.is_empty(), "seed {}: {pid} holds nothing once settled", self.seed);
            self.sim.assert_quiescent(*pid);
            self.sim.assert_no_open_fds(*pid);
            self.sim.assert_hosted_settled(*pid);
        }
        Finished {
            machine: self.machine,
            killed: self.killed,
            seed: self.seed,
            trace: self.sim.trace().to_vec(),
            end: self.sim.now(),
            procs: self.procs,
            steps: self.steps,
            submissions: self.submissions,
        }
    }

    /// Delivers a signal to a host's declared source between steps.
    pub fn signal(&mut self, host: usize, signal: skein_io::kernel::ServiceSignal) {
        let (pid, source) = *self.signals.get(&host).expect("a service declares its signal source");
        let at = self.pids.iter().position(|current| *current == pid).expect("a signal names a running host");
        assert!(!self.procs.get(at).expect("admitted process").is_empty(), "a signal names a running service");
        self.sim.deliver_service_signal(pid, source, signal, None);
    }

    /// Lends the simulator, process identities and processes between steps.
    pub fn parts(&mut self) -> (&mut Sim, &[Pid], &mut [P]) {
        (&mut self.sim, &self.pids, &mut self.procs)
    }

    /// A process lent to the loop for its next step's input.
    pub fn process(&mut self, at: usize) -> &mut P {
        self.procs.get_mut(at).expect("an admitted process")
    }

    /// Processes in admission order.
    #[must_use]
    pub fn processes(&self) -> &[P] {
        &self.procs
    }

    /// Steps completed, from zero.
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.steps
    }

    /// The world's seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The monotonic clock.
    #[must_use]
    pub fn now(&self) -> Time {
        self.sim.now()
    }

    /// The wall clock, from the configured start.
    #[must_use]
    pub fn wall(&self) -> skein_lib::Wall {
        self.sim.wall()
    }

    /// The loop's meter, for its report before finishing.
    #[must_use]
    pub const fn meter(&self) -> &H {
        &self.meter
    }

    /// Moves the idle world's clock without turning a process (world.md, 4).
    pub fn advance(&mut self, to: Time) {
        assert!(!self.busy(), "time moves only while the world is idle");
        assert!(to >= self.now(), "time never moves backwards");
        assert!(self.next_due().is_none_or(|due| to <= due), "time never moves past the next due instant");
        self.sim.advance_to(to);
    }

    /// One turn of process `at`'s loop: reap, iterate, submit.
    fn turn<T: Turns<P>>(&mut self, at: usize, now: Time, wall: skein_lib::Wall, turns: &mut T) {
        let pid = *self.pids.get(at).expect("a pid for each process");
        if *self.finished.get(at).expect("a status per process") {
            return;
        }
        // Reaping owns simulator bookkeeping; only delivery grows the process's
        // queue. Stage no more than its room and the records it has in flight.
        let room = self.procs.get_mut(at).expect("a process at each index").completions().room();
        let mut arrived = Queue::with_capacity(room.min(self.sim.in_flight(pid)));
        self.sim.reap(pid, &mut arrived);
        if !self.programs.is_empty() {
            // Inspect the completion before the parent's next iterate: a hosted
            // child already exists when its parent learns that it was spawned.
            for complete in &arrived {
                if let (Op::Spawn { spawn }, Ok(Done::Spawned { pidfd, .. })) = (&complete.kind, &complete.result)
                    && let Some((program_at, entry)) =
                        self.programs.iter().enumerate().find(|(_, entry)| entry.program == spawn.program)
                {
                    let active = self
                        .hosted
                        .iter()
                        .zip(&self.finished)
                        .filter(|(hosted, finished)| **hosted == Some(program_at) && !**finished)
                        .count();
                    assert!(
                        active < usize::try_from(entry.instances).expect("instances fit"),
                        "hosted program fits its instance provision"
                    );
                    let make = entry.make;
                    let operations = entry.operations;
                    let (child, pipes) = self.sim.bind_service(pid, *pidfd);
                    let roots = self
                        .pending_startup
                        .remove(&(pid, complete.op))
                        .expect("a hosted launch prepared its startup descriptors");
                    let appends = roots
                        .appends
                        .into_iter()
                        .map(|(name, handle)| (name, self.sim.append(child, handle)))
                        .collect();
                    let roots =
                        roots.roots.into_iter().map(|(name, handle)| (name, self.sim.root(child, handle))).collect();
                    let inherited = Inherited { pipes, roots, appends, signal: self.sim.open_signal_source(child) };
                    self.signals.insert(self.next_host, (child, inherited.signal));
                    let child_at = self.admit(child, Some(program_at), || make(spawn, &inherited));
                    assert!(
                        self.procs.get(child_at).expect("newly admitted child").operations() <= operations,
                        "hosted program operations fit its provision"
                    );
                }
            }
        }
        let proc = self.procs.get_mut(at).expect("a process at each index");
        self.meter.around(at, || {
            while let Some(complete) = arrived.pop() {
                proc.completions().push(complete);
            }
            proc.iterate(now, wall);
            proc.drain();
        });
        if self.digests == Digests::Traced
            && let Some(digest) = proc.state_digest()
        {
            self.sim.trace_digest(pid, digest);
        }
        if let Some(faults) = turns.faults(at, now, proc.submissions()) {
            self.sim.set_faults(faults);
        }
        let batch = proc.submissions().len();
        let count = *self.submissions.get(at).expect("a counter per process");
        let end = count.checked_add(batch).expect("submission count fits u32");
        let planned =
            self.cuts.iter().position(|cut| !cut.done && cut.host == at && cut.before >= count && cut.before < end);
        if let Some(planned) = planned {
            let before = self.cuts.get(planned).expect("selected cut").before;
            let prefix = before.checked_sub(count).expect("the cut is in this batch");
            let mut submits = Queue::with_capacity(prefix);
            for _ in 0..prefix {
                submits.push(proc.submissions().pop().expect("a submission in the selected prefix"));
            }
            if prefix > 0 {
                self.sim.submit(pid, &mut submits);
            }
            *self.submissions.get_mut(at).expect("the same counter") = before;
            self.perform_cut(planned);
            return;
        }
        *self.submissions.get_mut(at).expect("the same counter") = end;
        self.sim.submit(pid, proc.submissions());
        if self.hosted.get(at).expect("a status per process").is_some()
            && let Some(exit) = proc.exit()
        {
            assert!(proc.is_empty(), "a normal hosted exit releases every entity");
            self.sim.assert_quiescent(pid);
            self.sim.assert_no_open_fds(pid);
            self.sim.finish_service(pid, exit);
            *self.finished.get_mut(at).expect("a status per process") = true;
        }
        self.serve_machine();
    }

    fn drop_killed(&mut self, at: usize) {
        let pid = *self.pids.get(at).expect("a pid per process");
        let exit = self.sim.service_exit(pid).expect("the child terminated");
        while self.sim.in_flight(pid) > 0 {
            let proc = self.procs.get_mut(at).expect("a process per index");
            let mut arrived = Queue::with_capacity(proc.completions().room().min(self.sim.in_flight(pid)));
            self.sim.reap_exited_service(pid, &mut arrived);
            let pending = self.sim.in_flight(pid) > 0;
            self.meter.around(at, || {
                while let Some(complete) = arrived.pop() {
                    proc.completions().push(complete);
                }
                if pending {
                    // Delivery and freeing returned records belong to this
                    // process, metered as part of its destruction.
                    while let Some(complete) = proc.completions().pop() {
                        drop(complete);
                    }
                }
            });
        }
        let proc = self.procs.remove(at);
        let heap = self.meter.release(at, proc);
        self.pids.remove(at);
        self.hosted.remove(at);
        self.finished.remove(at);
        self.restart.remove(at);
        self.submissions.remove(at);
        self.sim.close_exited_service(pid);
        self.serve_machine();
        let mut closed = Queue::with_capacity(self.sim.in_flight(pid));
        self.sim.reap_exited_service(pid, &mut closed);
        while let Some(complete) = closed.pop() {
            assert!(matches!(complete.kind, Op::Close { .. }), "teardown creates only buffer-free close records");
        }
        self.sim.assert_quiescent(pid);
        self.sim.assert_no_open_fds(pid);
        self.killed.push(Killed { pid, exit, heap });
    }

    fn perform_cut(&mut self, planned: usize) {
        let cut = *self.cuts.get(planned).expect("selected cut");
        self.cuts.get_mut(planned).expect("selected cut").done = true;
        let affected: Vec<_> = match cut.cut {
            Cut::Kill => vec![cut.host],
            Cut::PowerLoss => {
                assert!(
                    self.restart
                        .iter()
                        .zip(&self.hosted)
                        .all(|(restart, hosted)| restart.is_some() || hosted.is_some()),
                    "power loss needs every live process's factory"
                );
                (0..self.procs.len()).collect()
            }
        };
        let mut handles = Vec::new();
        for at in &affected {
            let pid = *self.pids.get(*at).expect("a pid per process");
            let meter = &mut self.meter;
            let held = self.sim.cut_with(pid, |op| meter.around(*at, || drop(op)));
            handles.extend_from_slice(&held);
            let pending: Vec<_> = self.pending_startup.keys().filter(|(owner, _)| *owner == pid).copied().collect();
            for key in pending {
                let prepared = self.pending_startup.remove(&key).expect("selected prepared startup");
                handles.extend(prepared.roots.into_iter().chain(prepared.appends).map(|(_, handle)| handle));
            }
            self.sim.assert_quiescent(pid);
            self.sim.assert_no_open_fds(pid);
        }
        self.machine.cut(cut.cut, &handles, self.seed);
        for at in affected.into_iter().rev() {
            let old_pid = *self.pids.get(at).expect("a pid per process");
            if self.hosted.get(at).expect("a status per process").is_some() {
                let proc = self.procs.remove(at);
                self.meter.release(at, proc);
                self.pids.remove(at);
                self.hosted.remove(at);
                self.finished.remove(at);
                self.restart.remove(at);
                self.submissions.remove(at);
                self.signals.retain(|_, (owner, _)| *owner != old_pid);
                continue;
            }
            let declaration =
                self.restart.get_mut(at).expect("a slot per process").take().expect("a factory for a cut");
            let pid = self.sim.spawn_process();
            let inherited = self.restart_inherited(pid, &declaration.roots, &declaration.appends);
            let old = self.procs.remove(at);
            let make = declaration.make;
            let proc = self.meter.restart(at, old, || make(&inherited), P::worst_case);
            self.procs.insert(at, proc);
            *self.pids.get_mut(at).expect("same process index") = pid;
            *self.finished.get_mut(at).expect("same process index") = false;
            self.signals.retain(|_, (owner, _)| *owner != old_pid);
            self.signals.insert(at, (pid, inherited.signal));
            *self.restart.get_mut(at).expect("same factory slot") = Some(declaration);
        }
    }

    fn serve_machine(&mut self) {
        self.sim.calls(&mut self.calls);
        while let Some(call) = self.calls.pop() {
            if let Ask::Spawn { program, .. } = &call.ask
                && self.programs.iter().any(|entry| entry.program == *program)
            {
                let (pid, token, spawn) = self.sim.hosted_spawn(call.ticket);
                let program =
                    self.programs.iter().position(|entry| entry.program == spawn.program).expect("registered");
                let startup = self.startup.get(program).expect("startup selector per program");
                let declarations = (startup.roots)(spawn);
                let append_declarations = (startup.appends)(spawn);
                crate::program::check_appends(&append_declarations);
                crate::program::check_roots(&declarations);
                let descriptors = declarations
                    .len()
                    .checked_add(append_declarations.len())
                    .and_then(|count| count.checked_add(spawn.pipes.len()))
                    .and_then(|count| count.checked_add(1));
                if descriptors.is_none_or(|count| {
                    count > usize::try_from(self.sim.config().max_fds).expect("descriptor limit fits")
                }) {
                    self.answers
                        .push(Answer { ticket: call.ticket, result: Err(skein_io::kernel::Error::TooManyOpenFiles) });
                    continue;
                }
                let result = match self.open_startup(&declarations, &append_declarations) {
                    Ok(prepared) => {
                        self.pending_startup.insert((pid, token), prepared);
                        Ok(Reply::Program(Program::Service))
                    }
                    Err((_, error)) => Err(error),
                };
                self.answers.push(Answer { ticket: call.ticket, result });
            } else {
                self.machine.step(call, &mut self.answers);
            }
        }
        self.sim.answer(&mut self.answers);
    }

    /// Whether any process has work now: its loop's, the kernel's deferred
    /// to its next entry, or completions delivered and not reaped.
    #[must_use]
    pub fn busy(&self) -> bool {
        let now = self.now();
        for (proc, pid) in self.procs.iter().zip(&self.pids) {
            if proc.work_pending(now) || self.sim.deferred(*pid) || self.sim.ready(*pid) > 0 {
                return true;
            }
        }
        false
    }

    /// Whether every process holds nothing and has nothing in flight.
    #[must_use]
    pub fn settled(&self) -> bool {
        for (at, (proc, pid)) in self.procs.iter().zip(&self.pids).enumerate() {
            if self.hosted.get(at).expect("a hosting status per process").is_some()
                && !self.finished.get(at).expect("a terminal status per process")
            {
                return false;
            }
            if !proc.is_empty() || self.sim.in_flight(*pid) > 0 {
                return false;
            }
        }
        true
    }

    /// The earliest of what the simulator has due and every deadline in the
    /// world: the processes' (simulator.md, 3).
    #[must_use]
    pub fn next_due(&self) -> Option<Time> {
        let mut next = self.sim.next_due();
        let deadlines = self.procs.iter().map(Host::next_deadline);
        for at in deadlines.flatten() {
            next = Some(match next {
                Some(next) => next.min(at),
                None => at,
            });
        }
        next
    }

    /// The end of the trace, under its seed, for a failure.
    #[must_use]
    pub fn tail(&self) -> String {
        if self.sim.config().trace == skein_sim::Trace::Off {
            return String::from("the trace is off\n");
        }
        let text = self.sim.render_trace();
        let lines: Vec<&str> = text.lines().collect();
        let from = lines.len().saturating_sub(TAIL);
        let mut tail = format!("the last {} lines of the trace:\n", lines.len().saturating_sub(from));
        for line in lines.get(from..).unwrap_or_default() {
            writeln!(tail, "{line}").expect("writing to a String");
        }
        tail
    }
}

impl<P, M, H> World<P, M, H> {
    /// A printer handed to the named process at its making.
    pub fn printer(&mut self, process: &[u8], stream: crate::Stream) -> crate::Printer {
        self.printer.printer(process, stream)
    }

    /// Takes whole lines once, in write order; unfinished lines wait.
    pub fn printed(&mut self) -> Vec<crate::Printed> {
        self.printer.printed()
    }
}
