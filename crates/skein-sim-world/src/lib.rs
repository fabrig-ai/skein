//! A simulated world run a step at a time by its caller (world.md).
extern crate alloc;

mod meter;
mod printer;
/// Registry support shared with the native harness.
#[doc(hidden)]
pub mod program;
mod world;
mod writer_pipe;

pub use meter::{Bare, Meter};
pub use printer::{PRINTED_BYTES, Printed, Printer, Stream};
pub use program::{
    HostedProgram, Inherited, Machine, NoMachine, StartupAppend, StartupAppends, StartupRoot, StartupRoots,
};
pub use skein_shell::Host;
pub use world::{Cut, Digests, Killed, StateDigests, World};
pub use writer_pipe::{PipeCapture, PipeHost, PipeWriter};

/// Whether a step turned a process or dropped a killed child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub ran: bool,
}

/// What a loop does inside a step beside the world's operations (world.md, 3).
pub trait Turns<P> {
    /// Faults from this batch onward, or `None` to keep the simulator's.
    fn faults(
        &mut self,
        _at: usize,
        _now: skein_lib::Time,
        _batch: &skein_lib::Queue<skein_io::kernel::Submit>,
    ) -> Option<skein_sim::Faults> {
        None
    }
    /// After each process's turn, and after a killed child is dropped.
    fn turned(&mut self, _now: skein_lib::Time, _processes: &[P]) {}
}

impl<P> Turns<P> for () {}

/// The settled world handed back to its loop.
#[derive(Debug)]
pub struct Finished<P, M = NoMachine> {
    pub machine: M,
    pub killed: Vec<Killed>,
    pub seed: u64,
    pub trace: Vec<skein_sim::Entry>,
    pub procs: Vec<P>,
    pub end: skein_lib::Time,
    pub steps: u64,
    pub submissions: Vec<u32>,
}

/// Fails on a broken world contract.
#[expect(clippy::panic, reason = "a world fails on a broken contract, as the simulator does")]
pub(crate) fn fail(what: &str) -> ! {
    panic!("{what}")
}
