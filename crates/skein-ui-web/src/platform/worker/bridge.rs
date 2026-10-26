//! The page's portable end of the bridge (ui-web.md, 18).
use super::{Clock, Memory, Speed, WorldClock};
use crate::{
    bridge::{Duty, Line, Malformed, Said, Stamp, Start, ToWorld, decode_to_page, encode_to_world, log},
    fetch::{Exchange, Fetch, Report},
    transport::StreamDuty,
};
use skein_lib::{Time, Wall};
use std::collections::VecDeque;

/// The page's end's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Exchanges in flight, the page's fetch bound.
    pub exchanges: u32,
    /// The most bytes the session log holds, including its terminal.
    pub log: u32,
}

/// The page's end of the bridge to a world in a worker (ui-web.md, 18).
/// The shell drains `outgoing` after each call, bounding the queued messages
/// to what that call made. Stream opens wait for their terminals in order.
pub struct Bridge {
    fetch: Fetch,
    log: log::Writer,
    opens: VecDeque<u64>,
    clock: WorldClock,
    memory: Memory,
    outgoing: VecDeque<ToWorld>,
    speed: Speed,
    hidden: bool,
}

/// What the page hears from the world, after the bridge's bookkeeping.
#[derive(Debug, PartialEq, Eq)]
pub enum Heard {
    /// A slice's clock, or a report the page already ended.
    Nothing,
    /// Fetch's report, with the application's id restored.
    Report(Report),
    Line(Line),
    Stamp(Stamp),
    Stopped,
    Failed {
        words: Box<[u8]>,
    },
}

impl Bridge {
    /// A visit's start at browser instant `at`: `Start` is first out,
    /// followed by the initial pace. Until the world's first message, its
    /// clock is stepping at step zero, instant zero and the start's wall.
    #[must_use]
    pub fn new(start: Start, build: &[u8], limits: Limits, speed: Speed, at: Time) -> Self {
        let clock = Clock { step: 0, now: Time::ZERO, wall: start.start, idle: false, due: None };
        Self {
            fetch: Fetch::new(limits.exchanges),
            log: log::Writer::new(build, &start, limits.log),
            opens: VecDeque::new(),
            clock: WorldClock::new(clock, at, speed),
            memory: Memory::default(),
            outgoing: VecDeque::from([ToWorld::Start(start), ToWorld::Pace(speed)]),
            speed,
            hidden: false,
        }
    }

    /// Send with its ticket as its id. Past the in-flight bound, fail at
    /// once, returning the report and sending nothing.
    pub fn send(&mut self, exchange: &Exchange) -> Option<Report> {
        let Some(ticket) = self.fetch.begin(exchange.id, exchange.bound) else {
            return Some(Report::Failed { exchange: exchange.id });
        };
        let mut exchange = exchange.clone();
        exchange.id = u32::try_from(ticket).expect("a visit's tickets fit u32");
        self.duty(Duty::Exchange(exchange));
        None
    }

    /// Abandon under the ticket; an answer that crosses it is ignored.
    pub fn abandon(&mut self, exchange: u32) {
        if let Some(ticket) = self.fetch.abandon(exchange) {
            let exchange = u32::try_from(ticket).expect("a visit's tickets fit u32");
            self.duty(Duty::Abandon { exchange });
        }
    }

    /// A piece the page still holds, else a `ReadOn` sent for its stream.
    /// A read-on after the stream ended is ignored.
    pub fn read_on(&mut self) -> Option<Report> {
        if let Some(report) = self.fetch.read_on() {
            return Some(report);
        }
        if self.fetch.stream_ticket().is_some() {
            self.duty(Duty::ReadOn);
        }
        None
    }

    /// `Close` ends the page's stream at once, returning `StreamEnded`.
    /// `Open` while one is open ends that one first in the same way.
    pub fn stream(&mut self, duty: StreamDuty) -> Option<Report> {
        let ended = self.fetch.stream_ticket().and_then(|ticket| self.fetch.end_stream(ticket));
        match &duty {
            StreamDuty::Open { bound, .. } => {
                let ticket = self.fetch.open_stream(*bound);
                self.opens.push_back(ticket);
            }
            StreamDuty::Close => {}
        }
        self.duty(Duty::Stream(duty));
        ended
    }

    pub fn store(&mut self) -> &mut Memory {
        &mut self.memory
    }

    /// The frame's pace from now on; sent to the world. While hidden,
    /// remember the selection for showing and keep the world paused.
    pub fn pace(&mut self, speed: Speed, at: Time) {
        self.speed = speed;
        self.set_speed(at);
    }

    /// A hidden tab pauses the world and keeps its stream; showing resumes
    /// the selected pace. Repeated visibility reports have no effect.
    pub fn visibility(&mut self, hidden: bool, at: Time) {
        if hidden != self.hidden {
            self.hidden = hidden;
            self.set_speed(at);
        }
    }

    /// The world's instant and wall, for the application's step.
    pub fn now(&mut self, at: Time) -> (Time, Wall) {
        self.clock.now(at)
    }

    /// The browser instant for the application's deadline, a world instant.
    #[must_use]
    pub fn timer(&self, deadline: Time) -> Option<Time> {
        self.clock.timer(deadline)
    }

    /// The next message for the world, encoded, in order. The shell drains
    /// this after each call so queued messages are bounded by that call.
    pub fn outgoing(&mut self) -> Option<Box<[u8]>> {
        self.outgoing.pop_front().map(|message| encode_to_world(&message))
    }

    /// A message from the world at browser instant `at`: take its clock,
    /// then give what the page hears. Malformed bytes change no state.
    pub fn incoming(&mut self, bytes: &[u8], at: Time) -> Result<Heard, Malformed> {
        let message = decode_to_page(bytes)?;
        self.log.said(&message.said);
        self.clock.report(message.clock, at);
        Ok(match message.said {
            Said::Slice => Heard::Nothing,
            Said::Report(report) => self.report(report).map_or(Heard::Nothing, Heard::Report),
            Said::Line(line) => Heard::Line(line),
            Said::Stamp(stamp) => Heard::Stamp(stamp),
            Said::Stopped => Heard::Stopped,
            Said::Failed { words } => Heard::Failed { words },
        })
    }

    /// The session log as a file, as far as it goes.
    #[must_use]
    pub fn log(&self) -> Box<[u8]> {
        self.log.file()
    }

    fn duty(&mut self, duty: Duty) {
        self.log.sent(&duty);
        self.outgoing.push_back(ToWorld::Duty(duty));
    }

    fn set_speed(&mut self, at: Time) {
        let speed = if self.hidden { Speed(0) } else { self.speed };
        self.clock.pace(speed, at);
        self.outgoing.push_back(ToWorld::Pace(speed));
    }

    fn report(&mut self, report: Report) -> Option<Report> {
        match report {
            Report::Answered { exchange, status, body } => {
                let ticket = u64::from(exchange);
                self.fetch.piece(ticket, &body).or_else(|| self.fetch.finish(ticket, status))
            }
            Report::Failed { exchange } => self.fetch.fail(u64::from(exchange)),
            Report::StreamOpened => {
                let ticket = *self.opens.front()?;
                (self.fetch.stream_ticket() == Some(ticket)).then_some(Report::StreamOpened)
            }
            Report::StreamRefused { status } => {
                let ticket = self.opens.pop_front()?;
                self.fetch.refuse_stream(ticket, status)
            }
            Report::StreamPiece { bytes } => {
                let ticket = *self.opens.front()?;
                self.fetch.stream_piece(ticket, bytes)
            }
            Report::StreamEnded => {
                let ticket = self.opens.pop_front()?;
                self.fetch.end_stream(ticket)
            }
        }
    }
}
