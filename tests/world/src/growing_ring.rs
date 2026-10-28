//! A process whose completion storage grows only when the loop delivers.

use skein_heap::Span;
use skein_io::kernel::{Complete, Done, Op, Submit};
use skein_lib::{Queue, Time, Token, Wall};
use skein_world::Host;

/// Enough simultaneous completions to grow the ring through several capacities.
pub const OPERATIONS: u32 = 64;

/// The submissions are provisioned at construction; completions start empty.
#[derive(Debug)]
pub struct GrowingRing {
    completions: Queue<Complete>,
    submissions: Queue<Submit>,
    started: bool,
    /// What the process held before any completion storage was allocated.
    pub before_growth: u64,
    /// The most completions handed over before one iteration.
    pub largest_delivery: u32,
    /// Operations completed, all of which were submitted in the first turn.
    pub completed: u32,
}

impl GrowingRing {
    #[must_use]
    pub fn new() -> Self {
        let span = Span::start();
        let mut process = Self {
            completions: Queue::uncapped(),
            submissions: Queue::with_capacity(OPERATIONS),
            started: false,
            before_growth: 0,
            largest_delivery: 0,
            completed: 0,
        };
        process.before_growth = u64::try_from(span.end().net).expect("construction only allocates");
        process
    }
}

impl Default for GrowingRing {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for GrowingRing {
    fn iterate(&mut self, _now: Time, _wall: Wall) {
        self.largest_delivery = self.largest_delivery.max(self.completions.len());
        while let Some(complete) = self.completions.pop() {
            assert!(matches!(complete.kind, Op::Usage));
            assert!(matches!(complete.result, Ok(Done::Usage(_))));
            self.completed += 1;
        }
        if !self.started {
            for number in 0..OPERATIONS {
                self.submissions.push(Submit { op: Token::new(u64::from(number)), kind: Op::Usage });
            }
            self.started = true;
        }
    }

    fn completions(&mut self) -> &mut Queue<Complete> {
        &mut self.completions
    }

    fn submissions(&mut self) -> &mut Queue<Submit> {
        &mut self.submissions
    }

    fn work_pending(&self, _now: Time) -> bool {
        !self.started || !self.completions.is_empty()
    }

    fn next_deadline(&self) -> Option<Time> {
        None
    }

    fn next_policy_deadline(&self) -> Option<Time> {
        None
    }

    fn is_empty(&self) -> bool {
        self.completed == OPERATIONS && self.completions.is_empty() && self.submissions.is_empty()
    }

    fn worst_case(&self) -> u64 {
        // Bounds both rings, including transient completion-ring reallocations.
        1024 * 1024
    }

    fn operations(&self) -> u32 {
        OPERATIONS
    }
}
