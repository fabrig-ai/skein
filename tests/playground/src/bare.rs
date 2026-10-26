//! The same visitor and scripted peer in a bare product world, without a harness loop.
use crate::{SLOW, limits, server, settings};
use skein_io::kernel;
use skein_lib::{Queue, Time, Wall};
use skein_playground::{
    native::Native,
    slices::{Composition, Slices},
    visitor::Visitor,
};
use skein_shell::Host;
use skein_sim_world::{NoMachine, World};
use skein_ui_web::{
    bridge::{Duty, Said, Start, ToWorld},
    fetch::Exchange,
    platform::worker::{Heard, Speed},
};

pub const SEED: u64 = 73;
pub const END: Time = Time::from_nanos(1_000_000_000);

enum Kind {
    Visitor(Box<Visitor>),
    Server(Box<server::Server>),
}
pub struct Process {
    kind: Kind,
    end: Option<Time>,
    closing: bool,
}
pub struct Visit;
impl Composition for Visit {
    type Process = Process;
    type Machine = NoMachine;
    fn visitor(process: &mut Process) -> Option<&mut Visitor> {
        match &mut process.kind {
            Kind::Visitor(visitor) => Some(visitor),
            Kind::Server(_) => None,
        }
    }
}
macro_rules! host {
    ($self:ident, $name:ident $(, $arg:expr)*) => {
        match &mut $self.kind {
            Kind::Visitor(visitor) => visitor.$name($($arg),*),
            Kind::Server(server) => server.$name($($arg),*),
        }
    };
}
macro_rules! read_host {
    ($self:ident, $name:ident $(, $arg:expr)*) => {
        match &$self.kind {
            Kind::Visitor(visitor) => visitor.$name($($arg),*),
            Kind::Server(server) => server.$name($($arg),*),
        }
    };
}
impl Host for Process {
    fn iterate(&mut self, now: Time, wall: Wall) {
        if self.end.is_some_and(|end| now >= end) {
            self.closing = true;
            match &mut self.kind {
                Kind::Visitor(visitor) => visitor.close(),
                Kind::Server(server) => server.closing = true,
            }
        }
        host!(self, iterate, now, wall);
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        host!(self, completions)
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        host!(self, submissions)
    }
    fn work_pending(&self, now: Time) -> bool {
        (!self.closing && self.end.is_some_and(|end| now >= end)) || read_host!(self, work_pending, now)
    }
    fn next_deadline(&self) -> Option<Time> {
        read_host!(self, next_deadline).into_iter().chain(self.end.filter(|_| !self.closing)).min()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        self.next_deadline()
    }
    fn is_empty(&self) -> bool {
        read_host!(self, is_empty)
    }
    fn worst_case(&self) -> u64 {
        read_host!(self, worst_case)
    }
    fn operations(&self) -> u32 {
        read_host!(self, operations)
    }
}
#[must_use]
pub fn world(end: Option<Time>, duties: u32) -> World<Process> {
    let mut world = World::new(SEED, skein_sim::Config::calm());
    let printer = world.printer(b"scripted server", skein_sim_world::Stream::Output);
    world.spawn(|| {
        let mut server = server::Server::new(SLOW);
        server.printer = Some(printer);
        Process { kind: Kind::Server(Box::new(server)), end, closing: false }
    });
    world.spawn(|| {
        let mut limits = limits();
        limits.duties = duties;
        Process { kind: Kind::Visitor(Box::new(Visitor::new(settings(limits, server::ADDRESS)))), end, closing: false }
    });
    // Admit the listener before the page sends its first request.
    while world.busy() {
        world.step();
        assert!(world.steps() < 100, "listener starts without time passing");
    }
    world
}
#[must_use]
pub fn exchange(id: u32, path: &str) -> Duty {
    Duty::Exchange(Exchange {
        id,
        path: path.as_bytes().into(),
        method: "GET".into(),
        headers: Vec::new(),
        body: Box::from([]),
        bound: 256,
    })
}
#[must_use]
pub fn slices(budget: u32, speed: Speed, end: Option<Time>, duties: u32) -> Slices<Visit> {
    Slices::new(world(end, duties), budget, speed, Time::ZERO)
}
#[must_use]
pub fn native(speed: Speed) -> Native<Visit> {
    Native::new(
        world(Some(END), 4),
        Start { story: b"scripted".as_slice().into(), seed: SEED, start: skein_sim::Config::calm().wall },
        16,
        2,
        speed,
    )
}
pub fn give(slices: &mut Slices<Visit>, duty: Duty, at: Time) {
    slices.heard(ToWorld::Duty(duty), at);
}
#[must_use]
pub fn said(slices: &mut Slices<Visit>) -> Vec<Said> {
    std::iter::from_fn(|| slices.outgoing()).map(|message| message.said).collect()
}
#[must_use]
pub fn heard(native: &mut Native<Visit>) -> Vec<Heard> {
    std::iter::from_fn(|| native.heard()).collect()
}
/// Finish through the same slices, checking all product-world teardown invariants.
pub fn finish(slices: Slices<Visit>) {
    let mut world = slices.into_world();
    for at in 0..world.processes().len() {
        let process = world.process(at);
        process.end = Some(Time::ZERO);
    }
    let mut slices = Slices::<Visit>::new(world, 16, Speed(u32::MAX), Time::ZERO);
    for _ in 0..1000 {
        let Some(at) = slices.wake(Time::ZERO) else { break };
        slices.slice(at);
        let _messages = said(&mut slices);
    }
    assert!(slices.world().settled(), "bare world tears down");
    let _finished = slices.into_world().finish();
}
