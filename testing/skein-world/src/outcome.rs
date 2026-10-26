//! The harness's settled run and its final ownership check.
use crate::{Host, Killed, Machine, NoMachine, StateDigests};
use alloc::{format, vec::Vec};
use skein_heap::Span;
use skein_lib::Time;
use skein_sim::Entry;
use std::thread;

/// What a run left, for a test to look at by reference.
///
/// When memory was checked, dropping it is the last check of the run: each
/// process, dropped in turn, must free exactly what was metered as its own,
/// or something it allocated was never accounted, or never freed. A test
/// that panicked skips it.
#[derive(Debug)]
pub struct Outcome<P, M = NoMachine> {
    /// The fake machine after every non-hosted call has been answered.
    pub machine: M,
    /// Terminated children already dropped under their own memory spans.
    pub killed: Vec<Killed>,
    pub seed: u64,
    /// Every submission and completion, and every fault drawn: what the same
    /// seed replays to.
    pub trace: Vec<Entry>,
    /// Surviving processes in admission order, settled; killed children are
    /// reported separately in `killed` after their metered drop.
    pub procs: Vec<P>,
    pub iterations: u32,
    /// When the world settled.
    pub end: Time,
    /// The most heap each process held at once, and its worst case, by
    /// index, when memory was checked.
    pub heap: Option<Vec<(u64, u64)>>,
    /// What each process held of its own once settled, by index, when memory
    /// was checked: what dropping it must free.
    pub held: Option<Vec<i64>>,
    /// Submissions over each surviving process's life, across restarts.
    pub submissions: Vec<u32>,
}

impl<P, M> Drop for Outcome<P, M> {
    fn drop(&mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        if thread::panicking() {
            return;
        }
        for (at, (proc, held)) in self.procs.drain(..).zip(held).enumerate() {
            let span = Span::start();
            drop(proc);
            let freed = span.end().net.checked_neg().expect("a heap within an i64");
            if freed != held {
                crate::fail(&format!(
                    "seed {}: process {at}, dropped once settled, freed {freed} bytes, not the {held} it held of \
                     its own: a leak, or heap made or freed outside its calls",
                    self.seed
                ));
            }
        }
    }
}

impl<P: Host, M: Machine> Outcome<P, M> {
    /// Reads each surviving process's digest, then the machine's.
    #[must_use]
    pub fn state_digests(&self) -> StateDigests {
        StateDigests {
            procs: self.procs.iter().map(Host::state_digest).collect(),
            machine: self.machine.state_digest(),
        }
    }
}
