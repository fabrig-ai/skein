//! The loop's measurement around each process's own calls (world.md, 2).

/// What a loop measures: making a process, its iterate and drain, and dropping it.
/// The harness meters heap; a playground's loop measures nothing.
pub trait Meter {
    fn admit<T>(&mut self, make: impl FnOnce() -> T, worst_case: fn(&T) -> u64) -> T;
    fn around<T>(&mut self, at: usize, call: impl FnOnce() -> T) -> T;
    fn release<T>(&mut self, at: usize, proc: T) -> Option<(u64, u64)>;
    fn restart<T>(&mut self, at: usize, proc: T, make: impl FnOnce() -> T, worst_case: fn(&T) -> u64) -> T;
}

/// Measures nothing: each call runs bare.
#[derive(Debug)]
pub struct Bare;

impl Meter for Bare {
    fn admit<T>(&mut self, make: impl FnOnce() -> T, _worst_case: fn(&T) -> u64) -> T {
        make()
    }
    fn around<T>(&mut self, _at: usize, call: impl FnOnce() -> T) -> T {
        call()
    }
    fn release<T>(&mut self, _at: usize, proc: T) -> Option<(u64, u64)> {
        drop(proc);
        None
    }
    fn restart<T>(&mut self, _at: usize, proc: T, make: impl FnOnce() -> T, _worst_case: fn(&T) -> u64) -> T {
        drop(proc);
        make()
    }
}
