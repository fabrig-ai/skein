//! Native TLS against a rustls server in memory (tls.md, 3.6).
//! The proxy tracks independent reads and output rights. io's socket capacity
//! and lifecycle are checked by native.rs.
use crate::{native_peer::Tls12, pki, records, server::Server};
use skein_lib::stream::{Down, OutputDown, OutputOutcome, OutputUp, Read, Up};
use skein_lib::{Env, Queue, Time, Token};
use skein_tls::client::{self, Limits, native};
use skein_tls::{Config, Name};
use std::collections::VecDeque;

/// Independent output ownership below in this bounded proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Idle,
    Wanted { right: Token, bytes: u32 },
    Granted { right: Token, bytes: u32 },
    Cancelled(Token),
}

pub const LIMITS: Limits = Limits { read: 16, send: 16, records: client::MAX_RECORD };

/// The side above's output right in most tests.
pub const UPPER: Token = Token::new(1);

#[must_use]
pub fn granted(right: Token) -> native::Event {
    native::Event::Output(OutputUp::Settled { right, outcome: OutputOutcome::Granted })
}

/// The server: rustls's own, or one whose keys were taken out.
pub enum Peer {
    Rustls(Box<Server>),
    Extracted(Tls12),
}

impl Peer {
    #[must_use]
    pub fn rustls(&self) -> &Server {
        match self {
            Self::Rustls(server) => server,
            Self::Extracted(_) => panic!("rustls peer required"),
        }
    }

    pub fn rustls_mut(&mut self) -> &mut Server {
        match self {
            Self::Rustls(server) => server,
            Self::Extracted(_) => panic!("rustls peer required"),
        }
    }

    #[must_use]
    pub fn extracted(&self) -> &Tls12 {
        match self {
            Self::Extracted(server) => server,
            Self::Rustls(_) => panic!("extracted peer required"),
        }
    }

    pub fn extracted_mut(&mut self) -> &mut Tls12 {
        match self {
            Self::Extracted(server) => server,
            Self::Rustls(_) => panic!("extracted peer required"),
        }
    }
}

/// Native TLS and rustls server, with one prompt backed proxy slot.
/// It does not claim IO/kernel capacity or deterministic ciphertext.
/// All public methods check native queue maxima.
pub struct Pair {
    /// Native client with four distinct obligation cells.
    pub client: native::Client,
    /// Immutable native read/send/record bounds.
    pub env: Env<Limits>,
    /// The cryptographic peer and its authenticated plaintext.
    pub peer: Peer,
    /// Ciphertext emitted by the server, waiting for exact Fill.
    pub wire: VecDeque<u8>,
    /// The exact unanswered ciphertext read; Room never clears it.
    pub read: Option<Read>,
    /// Below Finish count for Finish controls.
    pub finished: u32,
    queued_read: Option<u32>,
    output: Output,
    last_send: Option<(Token, u32)>,
    cap: u32,
    above: Queue<native::Event>,
    below: Queue<native::LowerRequest>,
}

impl Pair {
    /// Make one peer at fixture-valid wall time. The proxy below has one
    /// local Send slot and the full checked native cap.
    #[must_use]
    pub fn new(config: &Config, name: Name, limits: Limits, server: Server) -> Self {
        let cap = native::largest_room(&limits).expect("checked native envelope");
        Self {
            client: native::Client::new(
                config,
                name,
                &limits,
                &native::LowerLimits { read: client::LARGEST_READ, output: cap, sends: 1 },
            )
            .expect("compatible proxy"),
            env: Env { now: Time::ZERO, wall: pki::VALID, limits },
            peer: Peer::Rustls(Box::new(server)),
            wire: VecDeque::new(),
            read: None,
            finished: 0,
            queued_read: None,
            output: Output::Idle,
            last_send: None,
            cap,
            above: Queue::uncapped(),
            below: Queue::uncapped(),
        }
    }

    /// A pair past Ready, with the shared limits.
    #[must_use]
    pub fn ready(server: &pki::Server) -> Self {
        let mut pair = Self::new(&pki::client(&[]), pki::name(), LIMITS, Server::new(server.config()));
        let mut events = pair.down(native::Request::Client(client::Request::Handshake));
        events.extend(pair.settle());
        let version = match server.versions {
            pki::Versions::Tls12 => Some(client::Version::Tls12),
            pki::Versions::Tls13 => Some(client::Version::Tls13),
            pki::Versions::Both => None,
        };
        assert!(matches!(&events[..], [native::Event::Client(client::Event::Ready(agreed))]
            if version.is_none_or(|version| agreed.version == version)));
        assert!(!pair.peer.rustls().handshaking());
        pair
    }

    pub fn read(&mut self, bytes: u32) -> Vec<native::Event> {
        self.down(native::Request::Client(client::Request::Stream(Down::Demand { read: Read::Fill(bytes), room: 0 })))
    }

    pub fn withdraw(&mut self) -> Vec<native::Event> {
        self.down(native::Request::Client(client::Request::Stream(Down::Demand { read: Read::Nothing, room: 0 })))
    }

    pub fn finish(&mut self) -> Vec<native::Event> {
        self.down(native::Request::Client(client::Request::Stream(Down::Finish)))
    }

    pub fn close(&mut self) -> Vec<native::Event> {
        self.down(native::Request::Client(client::Request::Close))
    }

    /// Ask for an output right, then settle the pair.
    pub fn room(&mut self, right: Token, bytes: u32) -> Vec<native::Event> {
        let mut events = self.down(native::Request::Output(OutputDown::Room { right, bytes }));
        events.extend(self.settle());
        events
    }

    pub fn send(&mut self, right: Token, bytes: &[u8]) -> Vec<native::Event> {
        self.down(native::Request::Output(OutputDown::Send { right, bytes: bytes.into() }))
    }

    #[must_use]
    pub fn peer_closed(&self) -> bool {
        match &self.peer {
            Peer::Rustls(peer) => peer.closed,
            Peer::Extracted(peer) => peer.alerts.contains(&[1, 0]),
        }
    }

    #[must_use]
    pub fn ready_tls12() -> Self {
        let server = pki::Server { versions: pki::Versions::Tls12, extractable: true, ..pki::Server::plain() };
        let mut pair = Self::ready(&server);
        pair.extract_peer();
        pair
    }

    #[must_use]
    pub fn ready_version(version: client::Version) -> Self {
        let versions = match version {
            client::Version::Tls12 => pki::Versions::Tls12,
            client::Version::Tls13 => pki::Versions::Tls13,
        };
        Self::ready(&pki::Server { versions, ..pki::Server::plain() })
    }

    /// Emit one exact Header or Body Fill winner, retaining its owning box
    /// above TLS until the chosen withdrawal/Finish ordering.
    pub fn queued_ciphertext(&mut self, body: bool) -> native::LowerEvent {
        self.peer.rustls_mut().write(&[b'x'; 16]);
        self.pull();
        assert_eq!(self.read, Some(Read::Fill(client::HEADER)));
        let header = self.answer().expect("real emitted Header winner");
        let native::LowerEvent::Stream(Up::Bytes(bytes)) = &header else { panic!("Header box") };
        assert_eq!(bytes.len(), usize::try_from(client::HEADER).expect("Header"));
        let length = u32::try_from(records::length(bytes)).expect("bounded record");
        if body {
            assert!(self.up(header).is_empty(), "Header alone invents no upper answer");
            assert_eq!(self.read, Some(Read::Fill(length)));
            let winner = self.answer().expect("real emitted Body winner");
            let native::LowerEvent::Stream(Up::Bytes(bytes)) = &winner else { panic!("Body box") };
            assert_eq!(bytes.len(), usize::try_from(length).expect("bounded real Body"));
            assert!(self.read.is_none(), "Fill ownership moved to queued box");
            winner
        } else {
            assert!(self.read.is_none(), "Fill ownership moved to queued box");
            header
        }
    }

    /// Process every pre-data ticket record, then retain an maximal
    /// body winner. The real configured ticket count bounds the prefix; neither
    /// peer tickets nor TLS controls are suppressed.
    pub fn queued_maximal_body(&mut self, versions: pki::Versions, ticket_records: usize) -> native::LowerEvent {
        let records = ticket_records.checked_add(1).expect("configured ticket prefix plus first data record");
        for prefix in 0..records {
            let header = self.answer().expect("emitted Header before maximal Body");
            let native::LowerEvent::Stream(Up::Bytes(bytes)) = &header else { panic!("Header Fill box") };
            assert_eq!(bytes.len(), usize::try_from(client::HEADER).expect("Header"));
            let length = u32::try_from(records::length(bytes)).expect("bounded record");
            assert!(self.up(header).is_empty(), "Header cannot fabricate a plaintext or output answer");
            assert_eq!(self.read, Some(Read::Fill(length)), "exact next Body Fill");
            let winner = self.answer().expect("emitted Body Fill");
            let native::LowerEvent::Stream(Up::Bytes(bytes)) = &winner else { panic!("Body Fill box") };
            assert_eq!(bytes.len(), usize::try_from(length).expect("bounded Body"));
            if length >= client::MAX_PLAINTEXT {
                if versions == pki::Versions::Tls13 {
                    assert!(prefix > 0, "post-handshake ticket flight preceded the full peer data");
                }
                return winner;
            }
            assert_eq!(versions, pki::Versions::Tls13, "only the TLS13 ticket flight precedes maximal data");
            assert!(self.up(winner).is_empty(), "ticket record is processed, never discarded or a short Fill answer");
        }
        panic!("configured ticket flight followed by maximal40000 data must yield a maximum Body: {versions:?}");
    }

    #[must_use]
    pub fn maximal_winner(versions: pki::Versions, body: bool) -> (Self, native::LowerEvent, Vec<u8>) {
        let server = pki::Server { versions, ..pki::Server::plain() };
        let config = server.config();
        let tickets = config.send_tls13_tickets;
        let mut pair = Self::new(&pki::client(&[]), pki::name(), crate::meter::LARGE, Server::new(config));
        let mut events = pair.down(native::Request::Client(client::Request::Handshake));
        events.extend(pair.settle());
        assert!(matches!(&events[..], [native::Event::Client(client::Event::Ready(_))]));
        assert!(pair.read(crate::meter::LARGE.read).is_empty());
        let plaintext = vec![b'x'; 40_000];
        pair.peer.rustls_mut().write(&plaintext);
        pair.pull();
        let winner = if body {
            pair.queued_maximal_body(versions, tickets)
        } else {
            pair.answer().expect("emitted Header Fill")
        };
        (pair, winner, plaintext)
    }

    /// Send one native request from above, retaining every request below and
    /// returning all observations above.
    pub fn down(&mut self, request: native::Request) -> Vec<native::Event> {
        native::down(&mut self.client, &self.env, request, &mut self.above, &mut self.below);
        self.route()
    }

    /// Forward one below observation from the proxy. A queued Granted
    /// can cross an above cancellation; a queued Fill box must match the one
    /// emitted by `answer`, even across withdrawal.
    pub fn up(&mut self, event: native::LowerEvent) -> Vec<native::Event> {
        if let native::LowerEvent::Stream(Up::Bytes(bytes)) = &event {
            assert_eq!(
                Some(u32::try_from(bytes.len()).expect("bounded Fill winner")),
                self.queued_read.take(),
                "only the proxy's  emitted exact Fill box is delivered"
            );
        }
        native::up(&mut self.client, &self.env, event, &mut self.above, &mut self.below);
        self.route()
    }

    /// Poll the independent read/output proxy. Emitting a grant commits
    /// its one slot; emitting a Fill moves its box and retains one size witness
    /// until `up`, including after withdrawal.
    pub fn answer(&mut self) -> Option<native::LowerEvent> {
        if let Some(Read::Fill(bytes)) = self.read
            && self.wire.len() >= usize::try_from(bytes).expect("bounded record")
        {
            assert!(self.queued_read.replace(bytes).is_none(), "one queued Fill winner");
            self.read = None;
            let bytes: Vec<u8> = self.wire.drain(..usize::try_from(bytes).expect("bounded Fill")).collect();
            return Some(native::LowerEvent::Stream(Up::Bytes(bytes.into())));
        }
        match self.output {
            Output::Wanted { right, bytes } => {
                self.output = Output::Granted { right, bytes };
                Some(native::LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Granted }))
            }
            Output::Cancelled(right) => {
                self.output = Output::Idle;
                Some(native::LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Cancelled }))
            }
            Output::Idle | Output::Granted { .. } => None,
        }
    }

    /// Drive only proxy answers, at most 10,000 transitions. It may rest
    /// with a unanswered read.
    pub fn settle(&mut self) -> Vec<native::Event> {
        let mut events = Vec::new();
        for _ in 0..10_000 {
            let Some(event) = self.answer() else { return events };
            events.extend(self.up(event));
        }
        panic!("bounded prompt native TLS peer rests");
    }

    /// Peer records moved onto the wire; no synthetic plaintext answers
    /// or Ready observations.
    pub fn pull(&mut self) {
        match &mut self.peer {
            Peer::Rustls(server) => self.wire.extend(server.transmit()),
            Peer::Extracted(server) => self.wire.extend(server.transmit()),
        }
    }

    /// Consume the completed extractable peer; all future Sends are
    /// authenticated using its directional keys.
    pub fn extract_peer(&mut self) {
        let Peer::Rustls(server) = &mut self.peer else { panic!("one extraction") };
        let placeholder = Server::new(pki::Server::plain().config());
        let server = std::mem::replace(server, Box::new(placeholder));
        self.peer = Peer::Extracted(Tls12::new(*server));
    }

    /// Current proxy reservation size, independently of its read. Used
    /// to compare growing owed output with the older full grant.
    #[must_use]
    pub fn output_bytes(&self) -> Option<u32> {
        match self.output {
            Output::Wanted { bytes, .. } | Output::Granted { bytes, .. } => Some(bytes),
            Output::Idle | Output::Cancelled(_) => None,
        }
    }

    /// Only the last consumed ciphertext Send's identity and length;
    /// bounded observer state, not predicted or replayable bytes.
    #[must_use]
    pub fn last_send(&self) -> Option<(Token, u32)> {
        self.last_send
    }

    fn route(&mut self) -> Vec<native::Event> {

        while let Some(request) = self.below.pop() {
            match request {
                native::LowerRequest::Stream(Down::Demand { read: Read::Nothing, room: 0 }) => {
                    assert!(
                        self.read.take().is_some() || self.queued_read.is_some(),
                        "withdraw only a live Fill or its already emitted winner"
                    );
                }
                native::LowerRequest::Stream(Down::Demand { read, room: 0 }) => {
                    assert!(self.queued_read.is_none(), "queued winner is delivered before a new Fill");
                    assert!(self.read.replace(read).is_none());
                    assert!(matches!(read, Read::Fill(n) if n <= client::LARGEST_READ));
                }
                native::LowerRequest::Stream(Down::Demand { .. } | Down::Send(_)) => panic!("native lower reads only"),
                native::LowerRequest::Stream(Down::Finish) => self.finished += 1,
                native::LowerRequest::Output(OutputDown::Room { right, bytes }) => {
                    assert_eq!(self.output, Output::Idle);
                    assert!(bytes > 0 && bytes <= self.cap);
                    self.output = Output::Wanted { right, bytes };
                }
                native::LowerRequest::Output(OutputDown::Cancel { right }) => match self.output {
                    Output::Wanted { right: pending, .. } if pending == right => self.output = Output::Cancelled(right),
                    Output::Idle | Output::Wanted { .. } | Output::Granted { .. } | Output::Cancelled(_) => {}
                },
                native::LowerRequest::Output(OutputDown::Send { right, bytes }) => {
                    let Output::Granted { right: granted, bytes: cap } = self.output else {
                        panic!("backed native Send")
                    };
                    assert_eq!(right, granted);
                    assert!(bytes.len() <= usize::try_from(cap).expect("bounded ciphertext"));
                    self.output = Output::Idle;
                    self.last_send = Some((right, u32::try_from(bytes.len()).expect("bounded ciphertext")));
                    match &mut self.peer {
                        Peer::Rustls(server) => server.receive(&bytes),
                        Peer::Extracted(server) => server.receive(&bytes),
                    }
                    self.pull();
                }
                native::LowerRequest::Output(OutputDown::Release { right }) => {
                    assert!(matches!(self.output, Output::Granted { right: granted, .. } if granted == right));
                    self.output = Output::Idle;
                }
            }
        }
        let mut events = Vec::new();
        while let Some(event) = self.above.pop() {
            events.push(event);
        }
        events
    }
}
