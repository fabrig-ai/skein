//! Bounded exchanges, identified independently of the application's ids.
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
pub mod web;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    pub id: u32,
    pub path: Box<[u8]>,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Box<[u8]>,
    pub bound: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Report {
    Answered { exchange: u32, status: u16, body: Box<[u8]> },
    Failed { exchange: u32 },
    StreamOpened,
    StreamRefused { status: u16 },
    StreamPiece { bytes: Box<[u8]> },
    StreamEnded,
}
struct Stream {
    ticket: u64,
    bound: usize,
    held: Box<[u8]>,
    at: usize,
}
struct Flight {
    id: u32,
    bound: u32,
    body: Vec<u8>,
}
pub struct Fetch {
    flights: BTreeMap<u64, Flight>,
    capacity: usize,
    serial: u64,
    stream: Option<Stream>,
}
impl Fetch {
    #[must_use]
    pub fn new(capacity: u32) -> Self {
        Self {
            flights: BTreeMap::new(),
            capacity: usize::try_from(capacity).expect("capacity fits"),
            serial: 0,
            stream: None,
        }
    }
    /// A ticket remains unique after the application abandons and reuses an id.
    pub fn begin(&mut self, id: u32, bound: u32) -> Option<u64> {
        if self.flights.len() == self.capacity || self.flights.values().any(|flight| flight.id == id) {
            return None;
        }
        self.serial = self.serial.checked_add(1).expect("exchange tickets fit");
        self.flights.insert(self.serial, Flight { id, bound, body: Vec::new() });
        Some(self.serial)
    }
    pub fn piece(&mut self, ticket: u64, piece: &[u8]) -> Option<Report> {
        let flight = self.flights.get_mut(&ticket)?;
        if piece.len() > usize::try_from(flight.bound).expect("bound fits").saturating_sub(flight.body.len()) {
            return self.fail(ticket);
        }
        flight.body.extend_from_slice(piece);
        None
    }
    #[must_use]
    pub fn remaining(&self, ticket: u64) -> Option<u32> {
        let flight = self.flights.get(&ticket)?;
        flight.bound.checked_sub(u32::try_from(flight.body.len()).expect("body within bound"))
    }
    pub fn finish(&mut self, ticket: u64, status: u16) -> Option<Report> {
        self.flights.remove(&ticket).map(|flight| Report::Answered {
            exchange: flight.id,
            status,
            body: flight.body.into_boxed_slice(),
        })
    }
    pub fn fail(&mut self, ticket: u64) -> Option<Report> {
        self.flights.remove(&ticket).map(|flight| Report::Failed { exchange: flight.id })
    }
    pub fn abandon(&mut self, id: u32) -> Option<u64> {
        let ticket = self.flights.iter().find_map(|(ticket, flight)| (flight.id == id).then_some(*ticket))?;
        self.flights.remove(&ticket);
        Some(ticket)
    }
    pub fn open_stream(&mut self, bound: u32) -> u64 {
        assert!(bound > 0, "a stream piece holds a byte");
        self.serial = self.serial.checked_add(1).expect("stream tickets fit");
        self.stream = Some(Stream {
            ticket: self.serial,
            bound: usize::try_from(bound).expect("bound fits"),
            held: Box::from([]),
            at: 0,
        });
        self.serial
    }
    #[must_use]
    pub fn stream_ticket(&self) -> Option<u64> {
        self.stream.as_ref().map(|stream| stream.ticket)
    }
    pub fn stream_piece(&mut self, ticket: u64, bytes: Box<[u8]>) -> Option<Report> {
        let stream = self.stream.as_mut().filter(|stream| stream.ticket == ticket)?;
        assert!(stream.at == stream.held.len(), "read on after the last piece");
        stream.held = bytes;
        stream.at = 0;
        self.read_on()
    }
    pub fn read_on(&mut self) -> Option<Report> {
        let stream = self.stream.as_mut()?;
        if stream.at == stream.held.len() {
            return None;
        }
        let end = stream.at.saturating_add(stream.bound).min(stream.held.len());
        let bytes = stream.held.get(stream.at..end).expect("part within held piece").into();
        stream.at = end;
        if end == stream.held.len() {
            stream.held = Box::from([]);
            stream.at = 0;
        }
        Some(Report::StreamPiece { bytes })
    }
    pub fn end_stream(&mut self, ticket: u64) -> Option<Report> {
        if self.stream_ticket() != Some(ticket) {
            return None;
        }
        self.stream = None;
        Some(Report::StreamEnded)
    }
    /// The stream's answer came with a status other than 200: the stream is closed.
    pub fn refuse_stream(&mut self, ticket: u64, status: u16) -> Option<Report> {
        self.end_stream(ticket).map(|_| Report::StreamRefused { status })
    }
}
