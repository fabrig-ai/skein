//! A world stepped in slices paced by the browser's clock (playground.md, 3).
use crate::visitor::Visitor;
use skein_lib::{Duration, Time};
use skein_sim_world::{Machine, Stream, World};
use skein_ui_web::{
    bridge::{Duty, Line, Output, Said, Stamp, ToPage, ToWorld},
    platform::worker::{Clock, Speed},
};
use std::collections::VecDeque;

/// The world's process and machine types, and where its visitor is.
pub trait Composition {
    type Process: skein_shell::Host;
    type Machine: Machine;
    /// The visitor, if `process` is it.
    fn visitor(process: &mut Self::Process) -> Option<&mut Visitor>;
}

/// A world run in slices, with inputs waiting between them.
pub struct Slices<C: Composition> {
    world: World<C::Process, C::Machine>,
    visitor: usize,
    budget: u32,
    speed: Speed,
    last: Time,
    banked: Duration,
    duties: VecDeque<Duty>,
    outgoing: VecDeque<ToPage>,
    stopped: bool,
    memory: u64,
    duration: Duration,
}

impl<C: Composition> Slices<C> {
    /// Exactly one process must be the visitor; the budget must be positive.
    #[must_use]
    pub fn new(mut world: World<C::Process, C::Machine>, budget: u32, speed: Speed, at: Time) -> Self {
        assert!(budget > 0, "a slice has a positive step budget");
        let mut visitor = None;
        for index in 0..world.processes().len() {
            if C::visitor(world.process(index)).is_some() {
                assert!(visitor.is_none(), "exactly one visitor");
                visitor = Some(index);
            }
        }
        Self {
            world,
            visitor: visitor.expect("exactly one visitor"),
            budget,
            speed,
            last: at,
            banked: Duration::ZERO,
            duties: VecDeque::new(),
            outgoing: VecDeque::new(),
            stopped: false,
            memory: 0,
            duration: Duration::ZERO,
        }
    }

    /// A message from the page. Start belongs to the world's constructor.
    pub fn heard(&mut self, message: ToWorld, at: Time) {
        assert!(at >= self.last, "browser time never moves backwards");
        match message {
            ToWorld::Start(_) => panic!("start belongs to the world's constructor"),
            ToWorld::Duty(duty) => {
                if !self.stopped {
                    self.duties.push_back(duty);
                }
            }
            ToWorld::Pace(speed) => {
                self.banked = self.elapsed(at);
                self.last = at;
                self.speed = speed;
            }
        }
    }

    /// Enter inputs, step toward this slice's target, then post its outputs.
    pub fn slice(&mut self, at: Time) {
        assert!(at >= self.last, "browser time never moves backwards");
        if self.stopped {
            return;
        }
        let target = self.world.now().saturating_add(self.elapsed(at));
        self.last = at;
        self.banked = Duration::ZERO;
        let mut said = Vec::new();
        let stamp = Stamp { step: self.world.steps(), instant: self.world.now() };
        let visitor = C::visitor(self.world.process(self.visitor)).expect("the visitor's process");
        for _ in 0..visitor.room() {
            let Some(duty) = self.duties.pop_front() else { break };
            visitor.duty(duty);
            said.push(Said::Stamp(stamp));
        }
        for _ in 0..self.budget {
            if self.world.settled() {
                break;
            }
            if self.world.busy() {
                self.world.step();
            } else if let Some(due) = self.world.next_due().filter(|due| *due <= target) {
                self.world.advance(due);
                self.world.step();
            } else {
                self.world.advance(target);
                break;
            }
        }
        let visitor = C::visitor(self.world.process(self.visitor)).expect("the visitor's process");
        while let Some(report) = visitor.report() {
            said.push(Said::Report(report));
        }
        for printed in self.world.printed() {
            said.push(Said::Line(Line {
                process: printed.process,
                stream: match printed.stream {
                    Stream::Output => Output::Standard,
                    Stream::Error => Output::Error,
                },
                line: printed.line,
            }));
        }
        if self.world.settled() {
            self.stopped = true;
            self.duties.clear();
            said.push(Said::Stopped);
        }
        if said.is_empty() {
            said.push(Said::Slice);
        }
        let clock = self.clock();
        self.outgoing.extend(said.into_iter().map(|said| ToPage { clock, said }));
    }

    /// When the next slice is due, on the browser's clock.
    #[must_use]
    pub fn wake(&self, at: Time) -> Option<Time> {
        if self.stopped {
            return None;
        }
        if !self.duties.is_empty() || self.world.busy() || self.world.settled() {
            return Some(at);
        }
        let due = self.world.next_due()?;
        let distance = due.saturating_since(self.world.now()).as_nanos();
        let banked = self.banked.as_nanos();
        if distance <= banked {
            return Some(at);
        }
        if self.speed.0 == 0 {
            return None;
        }
        let elapsed = distance.saturating_sub(banked).div_ceil(u64::from(self.speed.0));
        self.last.checked_add(Duration::from_nanos(elapsed)).map(|due| due.max(at))
    }

    /// The browser's figures for the next messages, including queued slice
    /// outputs. The worker supplies them after measuring the slice; native
    /// callers leave them at zero.
    pub fn set_figures(&mut self, memory: u64, slice: Duration) {
        self.memory = memory;
        self.duration = slice;
    }

    pub fn outgoing(&mut self) -> Option<ToPage> {
        let mut message = self.outgoing.pop_front()?;
        message.clock.memory = self.memory;
        message.clock.slice = self.duration;
        Some(message)
    }

    #[must_use]
    pub fn world(&self) -> &World<C::Process, C::Machine> {
        &self.world
    }

    #[must_use]
    pub fn into_world(self) -> World<C::Process, C::Machine> {
        self.world
    }

    fn elapsed(&self, at: Time) -> Duration {
        Duration::from_nanos(
            self.banked
                .as_nanos()
                .saturating_add(at.saturating_since(self.last).saturating_mul(u64::from(self.speed.0)).as_nanos()),
        )
    }

    pub(crate) fn clock(&self) -> Clock {
        Clock {
            step: self.world.steps(),
            now: self.world.now(),
            wall: self.world.wall(),
            idle: !self.world.busy(),
            due: self.world.next_due(),
            memory: self.memory,
            slice: self.duration,
        }
    }
}
