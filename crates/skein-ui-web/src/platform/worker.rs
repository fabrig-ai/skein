//! The worker platform's portable bridge, clock, store and navigation (ui-web.md, 18).
use skein_lib::{Duration, Time, Wall};
use std::collections::BTreeMap;

mod bridge;
pub use bridge::{Bridge, Heard, Limits};
#[cfg(target_arch = "wasm32")]
pub mod web;

/// The world's clock with each bridge message and after each slice (playground.md, 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// The world's step count.
    pub step: u64,
    /// Its monotonic instant.
    pub now: Time,
    /// Its wall.
    pub wall: Wall,
    /// Whether it waits idle: no process has work, nothing to deliver.
    pub idle: bool,
    /// Its next due instant, if any.
    pub due: Option<Time>,
    /// The world's memory in bytes: the worker's WebAssembly memory, which
    /// only grows (0 natively).
    pub memory: u64,
    /// The last slice's duration in browser time (zero natively).
    pub slice: Duration,
}

/// The world's figures as the page has seen them (playground.md, 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Figures {
    /// The most memory any message's clock named.
    pub memory: u64,
    /// The longest slice any message's clock named.
    pub slice: Duration,
}

impl Default for Figures {
    fn default() -> Self {
        Self { memory: 0, slice: Duration::ZERO }
    }
}

/// World nanoseconds per browser nanosecond; zero pauses the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speed(pub u32);

/// The world's clock as the page's platform gives it (ui-web.md, 18).
pub struct WorldClock {
    clock: Clock,
    at: Time,
    reference: Time,
    speed: Speed,
    last: Time,
}

impl WorldClock {
    /// A visit's first clock, its browser instant and starting pace.
    #[must_use]
    pub fn new(clock: Clock, at: Time, speed: Speed) -> Self {
        Self { clock, at, reference: clock.now, speed, last: clock.now }
    }

    /// A clock the world reported, at browser instant `at`.
    pub fn report(&mut self, clock: Clock, at: Time) {
        self.clock = clock;
        self.at = at;
        self.reference = clock.now;
    }

    /// Change pace without changing the instant given at `at`.
    pub fn pace(&mut self, speed: Speed, at: Time) {
        self.reference = self.now(at).0;
        self.at = at;
        self.speed = speed;
    }

    /// The instant and wall the application's step reads at browser instant `at`.
    pub fn now(&mut self, at: Time) -> (Time, Wall) {
        let projected = if self.clock.idle {
            let elapsed = at.saturating_since(self.at).saturating_mul(u64::from(self.speed.0));
            let instant = self.reference.saturating_add(elapsed);
            self.clock.due.map_or(instant, |due| instant.min(due))
        } else {
            self.clock.now
        };
        // A lagging report waits for the world to pass the last instant given.
        self.last = self.last.max(projected);
        let distance = self.last.saturating_since(self.clock.now).as_nanos();
        let wall = Wall::from_nanos(self.clock.wall.as_nanos().saturating_add(distance));
        (self.last, wall)
    }

    /// The least browser instant reaching a world deadline, from the reference.
    /// Paused, stepping, unreachable or unrepresentable deadlines have no timer.
    #[must_use]
    pub fn timer(&self, deadline: Time) -> Option<Time> {
        if self.speed.0 == 0 || !self.clock.idle {
            return None;
        }
        if deadline <= self.reference.max(self.last) {
            return Some(self.at);
        }
        if self.clock.due.is_some_and(|due| deadline > due) {
            return None;
        }
        let distance = deadline.saturating_since(self.reference).as_nanos();
        // Round up: a browser nanosecond before this one has not reached the deadline.
        let elapsed = distance.div_ceil(u64::from(self.speed.0));
        self.at.checked_add(Duration::from_nanos(elapsed))
    }
}

/// Records in the page's memory; the enclosing `Store` enforces its bound.
#[derive(Default)]
pub struct Memory {
    records: BTreeMap<Box<[u8]>, Box<[u8]>>,
}

impl Memory {
    #[must_use]
    pub fn read_all(&self) -> Vec<crate::store::Record> {
        self.records
            .iter()
            .map(|(key, value)| crate::store::Record { key: key.clone(), value: value.clone() })
            .collect()
    }

    /// Always taken: the bound is the `Store`'s.
    pub fn write(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.records.insert(key.into(), value.into());
        true
    }

    pub fn remove(&mut self, key: &[u8]) {
        self.records.remove(key);
    }
}

/// Only a path on the page's own origin is followed (ui-web.md, 18 and 10).
#[must_use]
pub fn navigable(to: &[u8]) -> bool {
    to.starts_with(b"/")
        && !to.starts_with(b"//")
        && !to.contains(&b'\\')
        && to.iter().all(|byte| (0x21..=0x7e).contains(byte))
}
