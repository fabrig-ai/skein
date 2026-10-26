//! Terminal accounting beside the story, independent of the visitor's bookkeeping.
use skein_ui_web::{bridge::Duty, fetch::Report, transport::StreamDuty};
use skein_world::domain::Ledger;
use std::collections::VecDeque;
#[derive(Clone, Copy)]
pub(super) enum Input {
    Exchange(u32),
    Abandon(u32),
    Open,
    Close,
    ReadOn,
}
impl Input {
    pub(super) fn of(duty: &Duty) -> Self {
        match duty {
            Duty::Exchange(exchange) => Self::Exchange(exchange.id),
            Duty::Abandon { exchange } => Self::Abandon(*exchange),
            Duty::Stream(StreamDuty::Open { .. }) => Self::Open,
            Duty::Stream(StreamDuty::Close) => Self::Close,
            Duty::ReadOn => Self::ReadOn,
        }
    }
}
pub(super) struct Contracts {
    exchanges: Ledger<u32, ()>,
    streams: VecDeque<bool>,
}
impl Contracts {
    pub(super) fn new() -> Self {
        Self { exchanges: Ledger::new("visitor exchange"), streams: VecDeque::new() }
    }
    pub(super) fn input(&mut self, input: Input) {
        match input {
            Input::Exchange(id) => self.exchanges.open(id, ()),
            Input::Abandon(id) => {
                self.exchanges.take(id);
            }
            Input::Open => self.streams.push_back(false),
            Input::Close | Input::ReadOn => {}
        }
    }
    pub(super) fn report(&mut self, report: &Report) {
        match report {
            Report::Answered { exchange, .. } | Report::Failed { exchange } => self.exchanges.end(*exchange),
            Report::StreamOpened => {
                let opened = self.streams.front_mut().expect("a stream opens only for an Open");
                assert!(!*opened, "a stream opens once");
                *opened = true;
            }
            Report::StreamEnded | Report::StreamRefused { .. } => {
                self.streams.pop_front().expect("one stream terminal per Open, in order");
            }
            Report::StreamPiece { .. } => assert_eq!(self.streams.front(), Some(&true), "pieces follow StreamOpened"),
        }
    }
    pub(super) fn settled(&self) {
        self.exchanges.assert_settled();
        assert!(self.streams.is_empty(), "every Open has one terminal");
    }
}
