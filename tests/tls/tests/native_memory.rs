//! Native TLS heap steps under scenario ceilings (tls.md, 3.6 and 5;
//! Testing-strategy.md, 5). The real peer runs between steps. All emitted
//! boxes are copied into reserved harness buffers and dropped before checking;
//! caller plaintext is charged separately. A complete sealed box dropped by
//! an encryption error remains a step allocation, never a handed-out box.

use std::collections::VecDeque;
use std::sync::Arc;

use skein_heap::{Counting, Span};
use skein_lib::stream::{Down, OutputDown, OutputOutcome, OutputUp, Read, Up};
use skein_lib::{Env, Queue, Time, Token};
use skein_tls::client::{self, Error, Limits, native};
use skein_tls_world::native_pair::{Output, Pair, Peer};
use skein_tls_world::native_peer::Tls12;
use skein_tls_world::pki::{self, Chain, Versions};
use skein_tls_world::server::Server;
use skein_tls_world::{
    meter::{LARGE, SMALL, Steps},
    records,
};

#[global_allocator]
static HEAP: Counting = Counting;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plain {
    Idle,
    Wanted(Token),
    Granted(Token),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Handshake,
    Exchange,
    Closing,
    Closed,
}

#[derive(Debug)]
struct Measured {
    most: u64,
    sealed: usize,
    plaintext: usize,
    ciphertext: usize,
    failed: Option<Error>,
}

/// Controls admitted by pinned rustls, never predicted reply counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Idle,
    Keys(u32),
    HelloRequest,
}

/// The server's and harness's heap is independently measured between native
/// steps; no peer allocation is silently counted as TLS memory.
struct Harness {
    metered: Steps,
    sealed: usize,
    plaintext: usize,
    ciphertext: usize,
    client: Option<native::Client>,
    env: Env<Limits>,
    above: Queue<native::Event>,
    below: Queue<native::LowerRequest>,
    peer: Peer,
    server_config: Arc<rustls::server::ServerConfig>,
    wire: VecDeque<u8>,
    outgoing: Vec<u8>,
    request: Vec<u8>,
    response: Vec<u8>,
    received: Vec<u8>,
    sent: usize,
    sequence: u64,
    name_bytes: usize,
    read: Option<Read>,
    output: Output,
    plain: Plain,
    phase: Phase,
    upper_read: bool,
    responded: bool,
    corrupt: bool,
    control: Control,
    failed: Option<Error>,
}

impl Harness {
    fn down(&mut self, request: native::Request) {
        let extra = match &request {
            native::Request::Output(OutputDown::Send { bytes, .. }) => bytes.len() as u64,
            native::Request::Client(_) | native::Request::Output(_) => 0,
        };
        self.metered.start();
        native::down(self.client.as_mut().expect("made"), &self.env, request, &mut self.above, &mut self.below);
        let step = self.metered.end();
        self.take(native::DOWN_MAX_OUT);
        self.check(step, extra);
    }

    fn up(&mut self, event: native::LowerEvent) {
        if let native::LowerEvent::Stream(Up::Bytes(bytes)) = &event {
            self.ciphertext = self.ciphertext.max(bytes.len());
        }
        self.metered.start();
        native::up(self.client.as_mut().expect("made"), &self.env, event, &mut self.above, &mut self.below);
        let step = self.metered.end();
        self.take(native::UP_MAX_OUT);
        self.check(step, 0);
    }

    fn check(&mut self, step: skein_heap::Measured, extra: u64) {
        self.metered.check(step, extra, &self.phase);
    }

    fn take(&mut self, maximum: skein_tls::MaxOut) {
        assert!(self.above.len() <= maximum.above && self.below.len() <= maximum.below);
        while let Some(event) = self.above.pop() {
            match event {
                native::Event::Client(client::Event::Ready(_)) => self.phase = Phase::Exchange,
                native::Event::Client(client::Event::Stream(Up::Bytes(bytes))) => {
                    assert!(self.upper_read && self.received.len() + bytes.len() <= self.received.capacity());
                    self.upper_read = false;
                    self.plaintext = self.plaintext.max(bytes.len());
                    self.received.extend_from_slice(&bytes);
                    drop(bytes);
                }
                native::Event::Client(client::Event::Stream(Up::Failed(_))) => self.upper_read = false,
                native::Event::Client(client::Event::Failed(error)) => self.failed = Some(error),
                native::Event::Client(client::Event::Closed) => self.phase = Phase::Closed,
                native::Event::Output(OutputUp::Settled { right, outcome }) => {
                    assert_eq!(self.plain, Plain::Wanted(right));
                    self.plain = match outcome {
                        OutputOutcome::Granted => Plain::Granted(right),
                        OutputOutcome::Cancelled | OutputOutcome::Failed(_) => Plain::Idle,
                    };
                }
                native::Event::Client(client::Event::Stream(Up::End | Up::Room))
                | native::Event::TransportClosing
                | native::Event::TransportClosed => panic!("meter proxy retains exact native lifecycle"),
            }
        }
        while let Some(request) = self.below.pop() {
            self.take_lower(request);
        }
    }

    fn take_lower(&mut self, request: native::LowerRequest) {
        match request {
            native::LowerRequest::Stream(Down::Demand { read: Read::Nothing, room: 0 }) => self.read = None,
            native::LowerRequest::Stream(Down::Demand { read, room: 0 }) => {
                assert!(self.read.replace(read).is_none(), "one independent ciphertext read");
            }
            native::LowerRequest::Output(OutputDown::Room { right, bytes }) => {
                assert_eq!(self.output, Output::Idle);
                assert!(bytes <= native::largest_room(&self.env.limits).expect("compatible"));
                self.output = Output::Wanted { right, bytes };
            }
            native::LowerRequest::Output(OutputDown::Send { right, bytes }) => {
                let Output::Granted { right: granted, bytes: cap } = self.output else { panic!("real proxy grant") };
                assert_eq!(right, granted);
                assert!(bytes.len() <= usize::try_from(cap).expect("finite"));
                assert!(self.outgoing.len() + bytes.len() <= self.outgoing.capacity());
                self.sealed = self.sealed.max(bytes.len());
                self.outgoing.extend_from_slice(&bytes);
                drop(bytes);
                self.output = Output::Idle;
            }
            native::LowerRequest::Output(OutputDown::Release { right }) => {
                assert!(matches!(self.output, Output::Granted { right: granted, .. } if granted == right));
                self.output = Output::Idle;
            }
            native::LowerRequest::Output(OutputDown::Cancel { right }) => match self.output {
                Output::Wanted { right: pending, .. } if pending == right => self.output = Output::Cancelled(right),
                Output::Idle | Output::Wanted { .. } | Output::Granted { .. } | Output::Cancelled(_) => {}
            },
            native::LowerRequest::Stream(Down::Demand { .. } | Down::Send(_) | Down::Finish) => {
                panic!("read-only native lower proxy")
            }
        }
    }

    fn peer_received(&self) -> &[u8] {
        match &self.peer {
            Peer::Rustls(peer) => &peer.received,
            Peer::Extracted(peer) => &peer.received,
        }
    }

    fn pull(&mut self) {
        match &mut self.peer {
            Peer::Rustls(peer) => self.wire.extend(peer.transmit()),
            Peer::Extracted(peer) => self.wire.extend(peer.transmit()),
        }
    }

    fn control(&mut self) {
        if self.sent != 0 || !matches!(self.plain, Plain::Granted(_)) {
            return;
        }
        match std::mem::replace(&mut self.control, Control::Idle) {
            Control::Idle => {}
            Control::Keys(count) => {
                for _ in 0..count {
                    let peer = self.peer.rustls_mut();
                    peer.key_update();
                    let requesting_record = peer.transmit();
                    assert_eq!(
                        requesting_record.len(),
                        pki::KEY_UPDATE_RECORD,
                        "each of 32 requesting key update records is emitted"
                    );
                    self.wire.extend(requesting_record);
                }
            }
            Control::HelloRequest => {
                let placeholder = Server::new(Arc::clone(&self.server_config));
                let Peer::Rustls(peer) = std::mem::replace(&mut self.peer, Peer::Rustls(Box::new(placeholder))) else {
                    panic!("one extraction")
                };
                let mut peer = Tls12::new(*peer);
                peer.hello_request();
                self.peer = Peer::Extracted(peer);
                self.pull();
            }
        }
    }

    fn between(&mut self) {
        let span = Span::start();
        if !self.outgoing.is_empty() {
            match &mut self.peer {
                Peer::Rustls(peer) => peer.receive(&self.outgoing),
                Peer::Extracted(peer) => peer.receive(&self.outgoing),
            }
            self.outgoing.clear();
        }
        self.control();
        if !self.responded && self.peer_received().len() == self.request.len() {
            assert_eq!(self.peer_received(), self.request, "authenticated maximal caller payload");
            self.responded = true;
            if let Peer::Rustls(peer) = &mut self.peer {
                peer.write(&self.response);
            } else {
                let peer = self.peer.extracted_mut();
                assert_eq!(peer.alerts, [[1, 100]], "one authenticated refusal before full plaintext");
                assert_eq!(peer.records.first(), Some(&rustls::ContentType::Alert), "owed prefix first");
                peer.write(&self.response);
            }
            let start = self.wire.len();
            self.pull();
            if self.corrupt {
                records::corrupt_first(&mut self.wire.make_contiguous()[start..]);
            }
        }
        self.pull();
        self.metered.outside(span.end().net);
    }

    fn answer(&mut self) -> bool {
        if let Some(Read::Fill(count)) = self.read {
            let count = usize::try_from(count).expect("finite");
            if self.wire.len() >= count {
                self.read = None;
                // Delivery made outside the step, but charged to TLS work:
                // it is not added to outside. It is dropped before processing.
                let bytes: Vec<u8> = self.wire.drain(..count).collect();
                self.up(native::LowerEvent::Stream(Up::Bytes(bytes.into())));
                return true;
            }
        }
        match self.output {
            Output::Wanted { right, bytes } => {
                self.output = Output::Granted { right, bytes };
                self.up(native::LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Granted }));
                true
            }
            Output::Cancelled(right) => {
                self.output = Output::Idle;
                self.up(native::LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Cancelled }));
                true
            }
            Output::Idle | Output::Granted { .. } => false,
        }
    }

    fn next(&mut self) {
        if self.answer() {
            return;
        }
        if self.phase == Phase::Closing {
            assert_eq!(self.output, Output::Idle, "all output obligations settled before Closed from below");
            self.up(native::LowerEvent::Closed);
            return;
        }
        if self.failed.is_some() || self.received.len() == self.response.len() {
            if self.failed.is_none() {
                assert_eq!(self.received, self.response, "complete maximal intake payload");
            }
            self.phase = Phase::Closing;
            self.down(native::Request::Client(client::Request::Close));
            return;
        }
        assert_eq!(self.phase, Phase::Exchange, "handshake must retain lower work");
        if !self.upper_read {
            self.ask_read();
            return;
        }
        match self.plain {
            Plain::Granted(right) => {
                let count = usize::try_from(self.env.limits.send).expect("finite").min(self.request.len() - self.sent);
                let bytes: Box<[u8]> = self.request[self.sent..self.sent + count].into();
                self.sent += count;
                self.plain = Plain::Idle;
                self.down(native::Request::Output(OutputDown::Send { right, bytes }));
            }
            Plain::Idle if self.sent < self.request.len() => {
                let right = Token::new(self.sequence);
                self.sequence = self.sequence.checked_add(1).expect("bounded case");
                self.plain = Plain::Wanted(right);
                self.down(native::Request::Output(OutputDown::Room { right, bytes: self.env.limits.send }));
            }
            Plain::Idle | Plain::Wanted(_) => panic!("independent metered TLS rights retain work"),
        }
    }

    fn ask_read(&mut self) {
        self.upper_read = true;
        let count =
            usize::try_from(self.env.limits.read).expect("finite").min(self.response.len() - self.received.len());
        self.down(native::Request::Client(client::Request::Stream(Down::Demand {
            read: Read::Fill(u32::try_from(count).expect("finite")),
            room: 0,
        })));
    }

    fn finish(mut self) -> Measured {
        assert!(self.above.is_empty() && self.below.is_empty());
        // These output buffers grow inside the client steps and release with it.
        let owned = (self.client.take().expect("made"), self.above, self.below);
        let most = self.metered.finish(owned, self.name_bytes, &self.phase);
        Measured {
            most,
            sealed: self.sealed,
            plaintext: self.plaintext,
            ciphertext: self.ciphertext,
            failed: self.failed,
        }
    }
}

fn measure(limits: Limits, versions: Versions, chain: Chain, corrupt: bool, ceiling: u64) -> Measured {
    let server = pki::Server { versions, chain, extractable: versions == Versions::Tls12, ..pki::Server::plain() };
    let config = pki::client(&[]);
    let text = if chain == Chain::Big { "big.skein.test" } else { "skein.test" };
    let name = skein_tls::Name::new(text).expect("name");
    let cap = native::largest_room(&limits).expect("checked envelope");
    let server_config = server.config();
    let mut harness = Harness {
        metered: Steps::new(ceiling),
        sealed: 0,
        plaintext: 0,
        ciphertext: 0,
        client: None,
        env: Env { now: Time::ZERO, wall: pki::VALID, limits },
        above: Queue::with_capacity(native::UP_MAX_OUT.above),
        below: Queue::with_capacity(native::UP_MAX_OUT.below),
        peer: Peer::Rustls(Box::new(Server::new(Arc::clone(&server_config)))),
        server_config,
        wire: VecDeque::with_capacity(4 * 65_536),
        outgoing: Vec::with_capacity(1 << 20),
        request: vec![b'p'; if limits.send == 1 { 300 } else { 100_000 }],
        response: vec![b'r'; if limits.read == 1 { 2_000 } else { 100_000 }],
        received: Vec::with_capacity(100_001),
        sent: 0,
        sequence: 0,
        name_bytes: text.len(),
        read: None,
        output: Output::Idle,
        plain: Plain::Idle,
        phase: Phase::Handshake,
        upper_read: false,
        responded: false,
        corrupt,
        control: if versions == Versions::Tls13 && limits == LARGE {
            Control::Keys(32)
        } else if versions == Versions::Tls12 {
            Control::HelloRequest
        } else {
            Control::Idle
        },
        failed: None,
    };
    harness.metered.reset();
    harness.metered.start();
    let client = native::Client::new(
        &config,
        name,
        &limits,
        &native::LowerLimits { read: client::LARGEST_READ, output: cap, sends: 1 },
    )
    .expect("compatible lower configuration");
    let step = harness.metered.end();
    harness.check(step, 0);
    harness.client = Some(client);
    harness.down(native::Request::Client(client::Request::Handshake));
    for _ in 0..100_000 {
        if harness.phase == Phase::Closed {
            return harness.finish();
        }
        harness.between();
        harness.next();
    }
    panic!("metered native conversation settles");
}

// Bounds the largest native peak across both TLS versions and both payload sizes.
const TLS_PAYLOAD_PEAK_CEILING: u64 = 170_000;

#[test]
fn payload_intake_and_ciphertext_stay_under_the_peak_ceiling() {
    for versions in [Versions::Tls12, Versions::Tls13] {
        for limits in [SMALL, LARGE] {
            let measured = measure(limits, versions, Chain::Leaf, false, TLS_PAYLOAD_PEAK_CEILING);
            assert_eq!(measured.failed, None, "{versions:?} {limits:?} {measured:?}");
            assert_eq!(measured.plaintext, usize::try_from(limits.read).expect("read size fits"));
            assert!(
                measured.ciphertext > 0
                    && measured.ciphertext <= usize::try_from(client::LARGEST_READ).expect("read fits")
            );
            assert!(measured.most <= TLS_PAYLOAD_PEAK_CEILING, "{measured:?}");
            if limits == LARGE {
                assert!(measured.sealed >= 40_000, "maximal encrypted caller payload: {measured:?}");
                assert_eq!(
                    measured.sealed,
                    match versions {
                        Versions::Tls13 => 40_000 + 3 * pki::TLS13_OVERHEAD + pki::KEY_UPDATE_RECORD,
                        Versions::Tls12 => pki::TLS12_ALERT_RECORD + 40_000 + 3 * pki::TLS12_OVERHEAD,
                        Versions::Both => unreachable!("explicit version cases"),
                    },
                    "complete requesting-control response plus maximal plaintext: {measured:?}"
                );
            }
        }
    }
}

// Bounds the largest native peak across big chains and decrypt errors in both TLS versions.
const TLS_CHAIN_AND_ERROR_PEAK_CEILING: u64 = 250_000;

#[test]
fn big_chain_and_decrypt_error_release_the_clients_heap() {
    for versions in [Versions::Tls12, Versions::Tls13] {
        let measured = measure(LARGE, versions, Chain::Big, false, TLS_CHAIN_AND_ERROR_PEAK_CEILING);
        assert_eq!(measured.failed, None, "{measured:?}");
        assert!(measured.most > 189_743, "double held decoded chain: {measured:?}");
        assert_eq!(
            measure(LARGE, versions, Chain::Leaf, true, TLS_CHAIN_AND_ERROR_PEAK_CEILING).failed,
            Some(Error::Decrypt)
        );
    }
}

#[test]
fn queued_header_drops_after_withdrawal_without_allocation() {
    for versions in [Versions::Tls12, Versions::Tls13] {
        let (mut pair, winner, plaintext) = Pair::maximal_winner(versions, false);
        let native::LowerEvent::Stream(Up::Bytes(bytes)) = &winner else { panic!("owning queued Fill box") };
        let count = i64::try_from(bytes.len()).expect("bounded Fill");
        assert!(count > 0 && count <= i64::from(client::LARGEST_READ));

        assert!(pair.read.is_none(), "box ownership moved out of Fill");
        assert!(pair.withdraw().is_empty());

        let span = Span::start();
        assert!(pair.up(winner).is_empty(), "disposed winner emits no observation");
        let step = span.end();
        assert_eq!(step.net, -count, "only the caller-owned queued ciphertext box is released");
        assert_eq!(step.peak, 0, "withdrawn winner is dropped without copying or processing");
        assert!(pair.read.is_none());
        assert_eq!(plaintext.len(), 40_000, "caller-owned payload retained throughout disposal");
        drop(plaintext);
    }
}

#[test]
fn queued_header_drops_after_finish_without_allocation() {
    for versions in [Versions::Tls12, Versions::Tls13] {
        let (mut pair, winner, plaintext) = Pair::maximal_winner(versions, false);
        let native::LowerEvent::Stream(Up::Bytes(bytes)) = &winner else { panic!("owning queued Fill box") };
        let count = i64::try_from(bytes.len()).expect("bounded Fill");
        assert!(count > 0 && count <= i64::from(client::LARGEST_READ));

        assert!(pair.read.is_none(), "box ownership moved out of Fill");
        assert!(pair.withdraw().is_empty());
        assert!(pair.finish().is_empty());
        let span = Span::start();
        assert!(pair.up(winner).is_empty(), "disposed winner emits no observation");
        let step = span.end();
        assert_eq!(step.net, -count, "only the caller-owned queued ciphertext box is released");
        assert_eq!(step.peak, 0, "withdrawn winner is dropped without copying or processing");
        assert!(pair.read.is_none());
        assert_eq!(plaintext.len(), 40_000, "caller-owned payload retained throughout disposal");
        drop(plaintext);
    }
}

#[test]
fn queued_body_drops_after_withdrawal_without_allocation() {
    for versions in [Versions::Tls12, Versions::Tls13] {
        let (mut pair, winner, plaintext) = Pair::maximal_winner(versions, true);
        let native::LowerEvent::Stream(Up::Bytes(bytes)) = &winner else { panic!("owning queued Fill box") };
        let count = i64::try_from(bytes.len()).expect("bounded Fill");
        assert!(count > 0 && count <= i64::from(client::LARGEST_READ));
        assert!(count >= i64::from(client::MAX_PLAINTEXT), "maximum record body pressure: {versions:?}, count={count}");
        assert!(pair.read.is_none(), "box ownership moved out of Fill");
        assert!(pair.withdraw().is_empty());

        let span = Span::start();
        assert!(pair.up(winner).is_empty(), "disposed winner emits no observation");
        let step = span.end();
        assert_eq!(step.net, -count, "only the caller-owned queued ciphertext box is released");
        assert_eq!(step.peak, 0, "withdrawn winner is dropped without copying or processing");
        assert!(pair.read.is_none());
        assert_eq!(plaintext.len(), 40_000, "caller-owned payload retained throughout disposal");
        drop(plaintext);
    }
}

#[test]
fn queued_body_drops_after_finish_without_allocation() {
    for versions in [Versions::Tls12, Versions::Tls13] {
        let (mut pair, winner, plaintext) = Pair::maximal_winner(versions, true);
        let native::LowerEvent::Stream(Up::Bytes(bytes)) = &winner else { panic!("owning queued Fill box") };
        let count = i64::try_from(bytes.len()).expect("bounded Fill");
        assert!(count > 0 && count <= i64::from(client::LARGEST_READ));
        assert!(count >= i64::from(client::MAX_PLAINTEXT), "maximum record body pressure: {versions:?}, count={count}");
        assert!(pair.read.is_none(), "box ownership moved out of Fill");
        assert!(pair.withdraw().is_empty());
        assert!(pair.finish().is_empty());
        let span = Span::start();
        assert!(pair.up(winner).is_empty(), "disposed winner emits no observation");
        let step = span.end();
        assert_eq!(step.net, -count, "only the caller-owned queued ciphertext box is released");
        assert_eq!(step.peak, 0, "withdrawn winner is dropped without copying or processing");
        assert!(pair.read.is_none());
        assert_eq!(plaintext.len(), 40_000, "caller-owned payload retained throughout disposal");
        drop(plaintext);
    }
}
