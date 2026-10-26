//! The referee's loop and verdicts over the simulated world's steps (world.md, 3).
use crate::heap::{Heap, Metered};
use crate::{
    Controls, Cut, Digests, Host, HostedProgram, Inherited, Machine, Memory, NoMachine, Outcome, Referee,
    StartupAppend, StartupAppends, StartupRoot, StartupRoots,
};
use alloc::{format, vec::Vec};
use core::fmt::{self, Debug};
use skein_io::kernel::{Fd, Submit};
use skein_lib::{Queue, Time};
use skein_sim::{Config, Faults, Handle, Pid, Sim};
use skein_sim_world::Turns;

/// The most iterations a world runs before it is declared stuck.
const STEPS: u64 = 1_000_000;

/// The harness's world: the stepped world, its referee and controls.
pub struct World<P, R, M = NoMachine> {
    inner: skein_sim_world::World<P, M, Metered>,
    referee: R,
    controls: Controls,
}

impl<P, R, M> Debug for World<P, R, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World").field("world", &self.inner).finish_non_exhaustive()
    }
}

impl<P: Host, R: Referee<P>> World<P, R> {
    /// Makes a world with the scenario's referee and memory checks.
    #[must_use]
    pub fn new(seed: u64, config: Config, referee: R, memory: Memory) -> World<P, R> {
        Self::new_controlled(seed, config, memory, |_| referee)
    }

    /// Builds the referee with the same signal controls as a real world.
    #[must_use]
    pub fn new_controlled<F: FnOnce(Controls) -> R>(
        seed: u64,
        config: Config,
        memory: Memory,
        make_referee: F,
    ) -> World<P, R> {
        let controls = Controls::new();
        let referee = make_referee(controls.clone());
        let heap = match memory {
            Memory::Checked => Some(Heap::new()),
            Memory::Unchecked => None,
        };
        World { inner: skein_sim_world::World::new(seed, config).with_meter(Metered(heap)), referee, controls }
    }

    /// Supplies the scenario's fake machine before the world starts.
    pub fn with_machine<M: Machine>(self, machine: M) -> World<P, R, M> {
        World { inner: self.inner.with_machine(machine), referee: self.referee, controls: self.controls }
    }
}

impl<P: Host, R: Referee<P>, M: Machine> World<P, R, M> {
    /// Registers a hosted program before execution, rejecting duplicate names.
    pub fn host(&mut self, program: HostedProgram<P>) {
        self.inner.host(program);
    }

    /// Registers a factory and the named directories opened independently for
    /// each launch, before its factory runs (simulator.md, section 3.1).
    pub fn host_roots(&mut self, program: HostedProgram<P>, roots: StartupRoots) {
        self.inner.host_roots(program, roots);
    }

    /// Registers independently opened roots and append files for each launch.
    pub fn host_startup(&mut self, program: HostedProgram<P>, roots: StartupRoots, appends: StartupAppends) {
        self.inner.host_startup(program, roots, appends);
    }

    /// Adds a process with a fake-machine root inherited from startup.
    pub fn spawn_root<F: FnOnce(Fd) -> P>(&mut self, root: Handle, make: F) -> usize {
        self.inner.spawn_root(root, make)
    }

    /// Adds a process whose factory owns independently opened named roots
    /// and a signal source. A cut restarts it at the same index over these
    /// root paths (simulator.md, 3.3).
    pub fn spawn_restartable(&mut self, roots: Vec<StartupRoot>, make: fn(&Inherited) -> P) -> usize {
        self.inner.spawn_restartable(roots, make)
    }

    /// Cuts `host` once before its zero-based `before`th submission.
    /// Its factory must be restartable; power loss requires factories for
    /// every process the world started; hosted children stay dead.
    pub fn cut(&mut self, host: usize, before: u32, cut: Cut) {
        self.inner.cut(host, before, cut);
    }

    /// Adds a process with an append file the scenario's machine opened at startup.
    pub fn spawn_append<F: FnOnce(Fd) -> P>(&mut self, file: Handle, make: F) -> usize {
        self.inner.spawn_append(file, make)
    }

    /// Adds the process `make` builds: its heap, from its making on, is the
    /// processes'.
    pub fn spawn<F: FnOnce() -> P>(&mut self, make: F) -> usize {
        self.inner.spawn(make)
    }

    /// Adds a process whose termination records arrive through its simulated signal source.
    pub fn spawn_signals<F: FnOnce(Fd) -> P>(&mut self, make: F) -> usize {
        self.inner.spawn_signals(make)
    }

    /// Adds a restartable process with independently opened roots and append files.
    pub fn spawn_restartable_startup(
        &mut self,
        roots: Vec<StartupRoot>,
        appends: Vec<StartupAppend>,
        make: fn(&Inherited) -> P,
    ) -> usize {
        self.inner.spawn_restartable_startup(roots, appends, make)
    }

    /// Sets the digest option before running the world.
    pub fn digests(&mut self, digests: Digests) {
        self.inner.digests(digests);
    }

    /// Runs the world until the referee passed and everything settled, then
    /// checks that every process holds nothing, and that the simulator has
    /// nothing in flight and no descriptor open.
    #[must_use]
    pub fn run(self) -> Outcome<P, M> {
        self.run_with_faults(|_, _, _| None)
    }

    /// Runs with a scenario's fault schedule at each kernel submission batch.
    /// The hook observes records outside the host's memory span; returning
    /// faults changes the simulator from that batch onward, while `None`
    /// keeps its current configuration. Records and their order are untouched.
    #[must_use]
    pub fn run_with_faults<F>(self, faults: F) -> Outcome<P, M>
    where
        F: FnMut(usize, Time, &Queue<Submit>) -> Option<Faults>,
    {
        self.run_inner(faults, |_, _, _| {})
    }

    /// Sets scenario-owned model inputs, such as per-process resource usage,
    /// between iterations. Hosts keep their records.
    #[must_use]
    pub fn run_with_inputs<F>(self, inputs: F) -> Outcome<P, M>
    where
        F: FnMut(&mut Sim, &[Pid], &[P]),
    {
        self.run_inner(|_, _, _| None, inputs)
    }

    fn run_inner<F, I>(mut self, mut faults: F, mut inputs: I) -> Outcome<P, M>
    where
        F: FnMut(usize, Time, &Queue<Submit>) -> Option<Faults>,
        I: FnMut(&mut Sim, &[Pid], &[P]),
    {
        let mut last_word = None;
        loop {
            if self.inner.steps().checked_add(1).expect("within STEPS") >= STEPS {
                crate::fail(&format!(
                    "seed {}: the world settles within {STEPS} iterations\n{}",
                    self.inner.seed(),
                    self.inner.tail()
                ));
            }
            let now = self.inner.now();
            let (sim, pids, procs) = self.inner.parts();
            inputs(sim, pids, procs);
            self.referee.act(now, procs);
            loop {
                let signal = self.controls.signals.borrow_mut().pop();
                let Some((host, signal)) = signal else { break };
                self.inner.signal(host, signal);
            }
            let mut turns = Observing { referee: &mut self.referee, faults: &mut faults };
            let step = self.inner.step_with(&mut turns);
            if !step.ran {
                self.referee.observe(now, self.inner.processes());
            }
            if last_word.is_none() && self.referee.passed() {
                last_word = Some(self.referee.last_word().at(now));
            }
            if self.inner.busy() || !self.controls.signals.borrow().is_empty() {
                continue;
            }
            if self.inner.settled() && self.referee.passed() {
                break;
            }
            if let Some(why) = self.referee.overdue(now) {
                crate::fail(&format!(
                    "seed {}: at {} ns, the referee failed the world:\n{why}\n{}",
                    self.inner.seed(),
                    now.as_nanos(),
                    self.inner.tail()
                ));
            }
            let next = match (self.inner.next_due(), self.referee.next_deadline()) {
                (Some(world), Some(referee)) => Some(world.min(referee)),
                (Some(at), None) | (None, Some(at)) => Some(at),
                (None, None) => None,
            };
            match next {
                Some(at) => {
                    let until = at.max(now);
                    self.check_teardown(last_word, until);
                    self.inner.advance(until);
                }
                None => crate::fail(&format!(
                    "seed {}: at {} ns, the world is idle, unsettled, with nothing due\n{}",
                    self.inner.seed(),
                    now.as_nanos(),
                    self.inner.tail()
                )),
            }
        }
        let heap = self.inner.meter().0.as_ref().map(Heap::report);
        let held = self.inner.meter().0.as_ref().map(Heap::held);
        let finished = self.inner.finish();
        Outcome {
            machine: finished.machine,
            killed: finished.killed,
            seed: finished.seed,
            trace: finished.trace,
            procs: finished.procs,
            iterations: u32::try_from(finished.steps).expect("within STEPS"),
            end: finished.end,
            heap,
            held,
            submissions: finished.submissions,
        }
    }

    fn check_teardown(&self, last_word: Option<Time>, until: Time) {
        let Some(last_word) = last_word else { return };
        for (process, host) in self.inner.processes().iter().enumerate() {
            if !host.is_empty()
                && let Some(deadline) = host.next_policy_deadline()
                && deadline > last_word
                && deadline <= until
            {
                crate::fail(&format!(
                    "seed {}: process {process} requires policy deadline {} ns after the last word\n{}",
                    self.inner.seed(),
                    deadline.as_nanos(),
                    self.inner.tail()
                ));
            }
        }
    }
}

struct Observing<'a, R, F> {
    referee: &'a mut R,
    faults: &'a mut F,
}

impl<P, R: Referee<P>, F> Turns<P> for Observing<'_, R, F>
where
    F: FnMut(usize, Time, &Queue<Submit>) -> Option<Faults>,
{
    fn faults(&mut self, at: usize, now: Time, batch: &Queue<Submit>) -> Option<Faults> {
        (self.faults)(at, now, batch)
    }
    fn turned(&mut self, now: Time, processes: &[P]) {
        self.referee.observe(now, processes);
    }
}
