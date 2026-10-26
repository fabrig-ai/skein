use super::{Host, Machine, World};
use skein_sim::Pid;

/// The state digests a run ended with, for replay (testing.md, 5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StateDigests {
    /// Each surviving process's, in admission order, as `Finished::procs`.
    pub procs: Vec<Option<[u8; 32]>>,
    /// The fake machine's.
    pub machine: Option<[u8; 32]>,
}

/// Whether a world records state digests beside its boundary records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Digests {
    /// Records alone, the default.
    Untraced,
    /// Each process before its submit, then the machine after every iteration.
    Traced,
}

impl<P: Host, M: Machine, H: crate::Meter> World<P, M, H> {
    /// Sets the digest option before running the world.
    pub fn digests(&mut self, digests: Digests) {
        self.digests = digests;
    }

    pub(super) fn trace_machine_digest(&mut self) {
        if self.digests == Digests::Traced
            && let Some(digest) = self.machine.state_digest()
        {
            self.sim.trace_digest(Pid::MACHINE, digest);
        }
    }
}
