//! Native lowest-tier ownership controls (tls.md, 3.6; testing-strategy.md, 2.1).

use super::{Client, Event, LowerEvent, LowerOutput, LowerRequest, Request, State, UpperOutput};
use crate::Name;
use crate::client::{self, Limits, Waiting};
use crate::tests::{LIMITS, VALID, WITHDRAW, config, native_peer};
use alloc::boxed::Box;
use skein_lib::stream::{Down, Fault, OutputDown, OutputOutcome, OutputUp, Read, Up};
use skein_lib::{Env, Queue, Time, Token, Wall};

fn lower(output: u32) -> super::LowerLimits {
    super::LowerLimits { read: client::LARGEST_READ, output, sends: 1 }
}

struct Machine {
    client: Client,
    env: Env<Limits>,
    above: Queue<Event>,
    below: Queue<LowerRequest>,
}

impl Machine {
    fn new() -> Self {
        Self {
            client: Client::new(
                &config(),
                Name::new("skein.test").expect("name"),
                &LIMITS,
                &lower(super::largest_room(&LIMITS).expect("cap")),
            )
            .expect("compatible"),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS },
            above: Queue::uncapped(),
            below: Queue::uncapped(),
        }
    }

    fn down(&mut self, request: Request) {
        assert!(self.above.is_empty() && self.below.is_empty(), "previous step fully observed");
        super::down(&mut self.client, &self.env, request, &mut self.above, &mut self.below);

    }

    fn up(&mut self, event: LowerEvent) {
        assert!(self.above.is_empty() && self.below.is_empty(), "previous step fully observed");
        super::up(&mut self.client, &self.env, event, &mut self.above, &mut self.below);

    }

    fn started(&mut self) -> Token {
        self.down(Request::Client(client::Request::Handshake));
        assert!(self.above.is_empty(), "real ClientHello does not complete handshake");
        let right = match self.below.pop().expect("ClientHello output right") {
            LowerRequest::Output(OutputDown::Room { right, bytes }) => {
                assert!(bytes > 0 && bytes <= client::FLIGHT, "real bounded ClientHello");
                right
            }
            LowerRequest::Output(OutputDown::Cancel { .. } | OutputDown::Send { .. } | OutputDown::Release { .. })
            | LowerRequest::Stream(_) => unreachable!("first request reserves real flight"),
        };
        assert_eq!(
            self.below.pop(),
            Some(LowerRequest::Stream(Down::Demand { read: Read::Fill(client::HEADER), room: 0 })),
            "separate real ciphertext read"
        );
        assert!(self.below.is_empty(), "exact initial two lower cells");
        right
    }

    fn sent_hello(&mut self, right: Token) {
        self.up(LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Granted }));
        assert!(self.above.is_empty(), "handshake still unanswered");
        match self.below.pop().expect("hello Send") {
            LowerRequest::Output(OutputDown::Send { right: sent, bytes }) => {
                assert_eq!(sent, right, "exact grant consumed");
                assert!(!bytes.is_empty(), "real rustls ClientHello");
            }
            LowerRequest::Output(OutputDown::Room { .. } | OutputDown::Cancel { .. } | OutputDown::Release { .. })
            | LowerRequest::Stream(_) => unreachable!("granted flight sends exactly once"),
        }
        assert!(self.below.is_empty(), "read remains outstanding independently");
    }
}

fn wanted(machine: &mut Machine, right: Token) {
    machine.down(Request::Output(OutputDown::Room { right, bytes: 1 }));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "upper Wanted waits for real Ready");
}

#[test]
fn a_fresh_native_client_waits_for_its_handshake() {
    let machine = Machine::new();
    assert_eq!(machine.client.waiting(), Waiting::Handshake);
}

#[test]
fn a_transport_closing_client_waits_for_its_close() {
    let mut machine = Machine::new();
    let right = machine.started();
    machine.up(LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Cancelled }));
    assert_eq!(machine.client.waiting(), Waiting::Close);
}

#[test]
fn native_room_counts_records_and_the_owed_prefix() {
    // 29 per record, 54 slack, and 2048 owed bytes (tls.md, 3.5, 3.6).
    for (plaintext, room) in [(1, 2_132), (16, 2_147), (16_384, 18_515), (16_385, 18_545), (40_000, 42_189)] {
        assert_eq!(super::room_for(plaintext), Some(room));
    }
}

#[test]
fn native_room_overflow_is_refused() {
    assert_eq!(super::room_for(u32::MAX), None);
}

#[test]
fn short_output_caps_are_refused() {
    assert!(Client::new(&config(), Name::new("skein.test").unwrap(), &LIMITS, &lower(2_146)).is_none());
}

#[test]
fn sufficient_output_caps_are_admitted() {
    assert!(Client::new(&config(), Name::new("skein.test").unwrap(), &LIMITS, &lower(2_147)).is_some());
}

#[test]
fn short_read_caps_are_refused() {
    let caps = super::LowerLimits { read: 18_431, output: 2_147, sends: 1 };
    assert!(Client::new(&config(), Name::new("skein.test").unwrap(), &LIMITS, &caps).is_none());
}

#[test]
fn missing_send_slots_are_refused() {
    let caps = super::LowerLimits { read: 18_432, output: 2_147, sends: 0 };
    assert!(Client::new(&config(), Name::new("skein.test").unwrap(), &LIMITS, &caps).is_none());
}

#[test]
fn cancel_before_ready_settles_only_its_named_right() {
    let mut machine = Machine::new();
    let lower = machine.started();
    wanted(&mut machine, Token::new(70));
    machine.down(Request::Output(OutputDown::Cancel { right: Token::new(71) }));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "stale cancel is inert");
    machine.down(Request::Output(OutputDown::Cancel { right: Token::new(70) }));
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: Token::new(70), outcome: OutputOutcome::Cancelled })),
        "upper owns its cancellation"
    );
    assert!(machine.above.is_empty() && machine.below.is_empty(), "TLS flight/read are still unanswered");
    machine.sent_hello(lower);
    machine.down(Request::Client(client::Request::Close));
    assert!(machine.above.is_empty(), "local close awaits physical Closed");
    assert_eq!(machine.below.pop(), Some(LowerRequest::Stream(WITHDRAW)), "read withdrawn only on close");
    machine.up(LowerEvent::Closed);
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Closed)), "local terminal after closure");
}

#[test]
fn owner_closure_cancels_output_before_ready() {
    let mut machine = Machine::new();
    let lower = machine.started();
    machine.sent_hello(lower);
    wanted(&mut machine, Token::new(8));
    machine.up(LowerEvent::Closed);
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: Token::new(8), outcome: OutputOutcome::Cancelled })),
        "own upper debt settles before lifecycle"
    );
    assert_eq!(machine.above.pop(), Some(Event::TransportClosed), "physical close ends Handshake/read");
    assert!(machine.below.is_empty(), "no fabricated lower Failed or cancellation");
    machine.down(Request::Client(client::Request::Close));
    assert!(
        machine.above.is_empty() && machine.below.is_empty(),
        "crossed native Close after TransportClosed is inert"
    );
}

#[test]
fn close_after_cancellation_waits_for_the_owner() {
    let mut machine = Machine::new();
    let lower = machine.started();
    wanted(&mut machine, Token::new(8));
    machine.up(LowerEvent::Output(OutputUp::Settled { right: lower, outcome: OutputOutcome::Cancelled }));
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: Token::new(8), outcome: OutputOutcome::Cancelled })),
        "upper debt cancellation"
    );
    assert_eq!(machine.above.pop(), Some(Event::TransportClosing), "real unsolicited cancellation lifecycle");
    assert_eq!(machine.below.pop(), Some(LowerRequest::Stream(WITHDRAW)), "closing read withdrawal");
    machine.down(Request::Client(client::Request::Close));
    assert!(machine.above.is_empty(), "local Close awaits real physical closure");
    machine.up(LowerEvent::Closed);
    assert_eq!(
        machine.above.pop(),
        Some(Event::Client(client::Event::Closed)),
        "local Close answers once after Closed"
    );
    assert!(machine.above.is_empty() && machine.below.is_empty(), "no invented second Closed");
}

#[test]
fn owner_closure_makes_a_later_close_inert() {
    let mut machine = Machine::new();
    let lower = machine.started();
    wanted(&mut machine, Token::new(8));
    machine.up(LowerEvent::Output(OutputUp::Settled { right: lower, outcome: OutputOutcome::Cancelled }));
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: Token::new(8), outcome: OutputOutcome::Cancelled })),
        "upper debt cancellation"
    );
    assert_eq!(machine.above.pop(), Some(Event::TransportClosing), "real unsolicited cancellation lifecycle");
    assert_eq!(machine.below.pop(), Some(LowerRequest::Stream(WITHDRAW)), "closing read withdrawal");
    machine.up(LowerEvent::Closed);
    assert_eq!(machine.above.pop(), Some(Event::TransportClosed), "physical lifecycle wins before queued Close");
    machine.down(Request::Client(client::Request::Close));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "no invented second Closed");
}

#[test]
fn a_grant_crossing_close_is_released() {
    let mut machine = Machine::new();
    let lower = machine.started();
    machine.down(Request::Client(client::Request::Close));
    assert!(machine.above.is_empty(), "Close waits for admitted lower terminal");
    assert_eq!(machine.below.pop(), Some(LowerRequest::Stream(WITHDRAW)), "close withdraws real read");
    assert_eq!(
        machine.below.pop(),
        Some(LowerRequest::Output(OutputDown::Cancel { right: lower })),
        "self-cancel retains lower identity"
    );
    machine.up(LowerEvent::Output(OutputUp::Settled { right: lower, outcome: OutputOutcome::Granted }));
    assert_eq!(
        machine.below.pop(),
        Some(LowerRequest::Output(OutputDown::Release { right: lower })),
        "queued grant remains real winner"
    );
    assert!(
        machine.above.is_empty() && machine.below.is_empty(),
        "lower grant settles without inventing physical closure"
    );
    machine.up(LowerEvent::Closed);
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Closed)), "no abandoned lower debt at closure");
    assert!(machine.above.is_empty() && machine.below.is_empty(), "no cancellation terminal fabricated");
}

#[test]
fn self_cancel_preserves_output_until_owner_closure() {
    let mut machine = Machine::new();
    let lower = machine.started();
    // An cancellation of this TLS-owned flight is injected at the
    // lower cell tier, retaining precisely its issued identity and witness.
    super::cancel_lower(&mut machine.client, &mut machine.below);
    assert_eq!(
        machine.below.pop(),
        Some(LowerRequest::Output(OutputDown::Cancel { right: lower })),
        "real pending lower self-cancel"
    );
    wanted(&mut machine, Token::new(88));
    machine.up(LowerEvent::Output(OutputUp::Settled { right: lower, outcome: OutputOutcome::Granted }));
    // Grant wins across the cancel: the owed ClientHello is sent,
    // rather than falsely claiming cancellation. No new upper grant pre-Ready.
    match machine.below.pop().expect("flight consumes winning grant") {
        LowerRequest::Output(OutputDown::Send { right, .. }) => assert_eq!(right, lower, "same lower right"),
        LowerRequest::Output(OutputDown::Room { .. } | OutputDown::Cancel { .. } | OutputDown::Release { .. })
        | LowerRequest::Stream(_) => unreachable!("owed flight sent in queued grant"),
    }
    assert!(machine.above.is_empty() && machine.below.is_empty(), "new upper right remains Wanted");
    machine.up(LowerEvent::Closed);
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: Token::new(88), outcome: OutputOutcome::Cancelled })),
        "new debt cancelled by Closed"
    );
    assert_eq!(machine.above.pop(), Some(Event::TransportClosed), "one physical lifecycle");
}

#[test]
fn the_last_token_is_used_once_without_wrapping() {
    let mut machine = Machine::new();
    machine.client.next_lower = Some(u64::MAX);
    let lower = machine.started();
    assert_eq!(lower, Token::new(u64::MAX), "last distinct lower token may be used once");
    assert_eq!(machine.client.next_lower, None, "never wrap or reuse");
    machine.sent_hello(lower);
    wanted(&mut machine, Token::new(4));
    super::request_room(
        &mut machine.client,
        1,
        super::Purpose::Upper { right: Token::new(4), bytes: 1 },
        &mut machine.above,
        &mut machine.below,
    );
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: Token::new(4), outcome: OutputOutcome::Failed(Fault::Other) })),
        "pending upper settles first"
    );
    assert_eq!(
        machine.above.pop(),
        Some(Event::Client(client::Event::Stream(Up::Failed(Fault::Other)))),
        "stream failure next"
    );
    assert_eq!(
        machine.above.pop(),
        Some(Event::Client(client::Event::Failed(client::Error::Other))),
        "local checked exhaustion cause"
    );
    assert_eq!(
        machine.below.pop(),
        Some(LowerRequest::Stream(WITHDRAW)),
        "independent ciphertext read is withdrawn on local failure"
    );
    assert!(machine.below.is_empty(), "no Room, encryption or transfer after exhaustion");
}

#[test]
fn stale_output_cannot_touch_another_grant() {
    let mut machine = Machine::new();
    let lower = machine.started();
    machine.sent_hello(lower);
    machine.client.upper_output = UpperOutput::Granted { right: Token::new(5), bytes: 1 };
    machine.client.lower_output =
        LowerOutput::Granted { right: Token::new(6), bytes: super::room_for(1).expect("cap") };
    machine.down(Request::Output(OutputDown::Send { right: Token::new(4), bytes: Box::from(&b"oversized"[..]) }));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "stale identity precedes payload size");
    machine.down(Request::Output(OutputDown::Release { right: Token::new(4) }));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "another grant remains affine");
    assert_eq!(
        machine.client.upper_output,
        UpperOutput::Granted { right: Token::new(5), bytes: 1 },
        "upper right unchanged"
    );
}

#[test]
#[should_panic(expected = "matching native Send within grant")]
fn matching_oversized_send_is_a_bug() {
    let mut machine = Machine::new();
    let lower = machine.started();
    machine.sent_hello(lower);
    machine.client.upper_output = UpperOutput::Granted { right: Token::new(5), bytes: 1 };
    machine.down(Request::Output(OutputDown::Send { right: Token::new(5), bytes: Box::from(&b"large"[..]) }));
}

#[test]
#[should_panic(expected = "native Finish after Ready once")]
fn finish_before_ready_is_a_bug() {
    let mut machine = Machine::new();
    machine.started();
    machine.down(Request::Client(client::Request::Stream(Down::Finish)));
}

#[test]
#[should_panic(expected = "native plaintext Demand is read-only")]
fn classic_room_is_a_bug_on_the_native_face() {
    let mut machine = Machine::new();
    machine.started();
    machine.down(Request::Client(client::Request::Stream(Down::Demand { read: Read::Nothing, room: 1 })));
}

#[test]
#[should_panic(expected = "lower terminal precedes physical Closed")]
fn closure_requires_every_output_terminal_below() {
    let mut machine = Machine::new();
    machine.started();
    machine.up(LowerEvent::Closed);
}

#[test]
fn duplicate_closure_preserves_the_first_terminal() {
    let mut machine = Machine::new();
    let lower = machine.started();
    machine.sent_hello(lower);
    machine.up(LowerEvent::Closed);
    assert_eq!(machine.above.pop(), Some(Event::TransportClosed), "lifecycle once");
    machine.up(LowerEvent::Closed);
    assert!(machine.above.is_empty() && machine.below.is_empty(), "duplicate lifecycle inert");
    match machine.client.state {
        State::TransportClosed => {}
        State::Fresh(_) | State::Open(_) | State::Failed | State::Closing | State::TransportClosing | State::Closed => {
            unreachable!("physical terminal persists")
        }
    }
}

/// A rustls peer that writes the handshake records.
struct Peer {
    tls: rustls::server::UnbufferedServerConnection,
    input: crate::held::Held,
    output: crate::held::Held,
}

impl Peer {
    fn process(&mut self) {
        use rustls::unbuffered::{ConnectionState, UnbufferedStatus};
        for _ in 0..64_u32 {
            let UnbufferedStatus { discard, state } = self.tls.process_tls_records(self.input.filled_mut());
            let state = state.expect("real valid client fixture handshake");
            // rustls's states are non-exhaustive. The fixture reaches only
            // the handshake and resting states of 0.23.
            let resting = if let ConnectionState::EncodeTlsData(mut encode) = state {
                let written = encode.encode(self.output.spare_mut()).expect("bounded server flight");
                self.output.wrote(written);
                false
            } else if let ConnectionState::TransmitTlsData(transmit) = state {
                transmit.done();
                false
            } else if let ConnectionState::BlockedHandshake
            | ConnectionState::WriteTraffic(_)
            | ConnectionState::Closed
            | ConnectionState::PeerClosed = state
            {
                true
            } else {
                unreachable!("rustls 0.23 positive handshake states; no peer application data")
            };
            self.input.discard(discard);
            if resting {
                return;
            }
        }
        unreachable!("bounded server processing rests");
    }
}

struct Handshake {
    machine: Machine,
    peer: Peer,
    read: Option<Read>,
    room: Option<(Token, u32)>,
    grant: Option<(Token, u32)>,
}

impl Handshake {
    fn new() -> Handshake {
        let (config, tls) = native_peer();
        let mut machine = Machine::new();
        machine.env.wall = VALID;
        machine.client = Client::new(
            &config,
            Name::new("skein.test").expect("certificate name"),
            &LIMITS,
            &lower(super::largest_room(&LIMITS).expect("cap")),
        )
        .expect("compatible");
        let capacity = client::MAX_HANDSHAKE.checked_add(client::MAX_RECORD).expect("maximum peer flight");
        let peer = Peer {
            tls,
            input: crate::held::Held::with_capacity(capacity),
            output: crate::held::Held::with_capacity(capacity),
        };
        machine.down(Request::Client(client::Request::Handshake));
        Handshake { machine, peer, read: None, room: None, grant: None }
    }

    fn take(&mut self) {
        for _ in 0..self.machine.below.len() {
            let Some(request) = self.machine.below.pop() else { break };
            match request {
                LowerRequest::Stream(Down::Demand { read, room: 0 }) => self.read = Some(read),
                LowerRequest::Output(OutputDown::Room { right, bytes }) => {
                    assert!(self.room.replace((right, bytes)).is_none(), "one fixture demand");
                }
                LowerRequest::Output(OutputDown::Send { right, bytes }) => {
                    assert!(self.room.is_none(), "grant observed before its Send");
                    let (expected, cap) = self.grant.take().expect("one fixture grant");
                    assert_eq!(right, expected, "below's token");
                    assert!(bytes.len() <= usize::try_from(cap).expect("finite capacity"), "within fixture grant");
                    self.peer.input.append(&bytes).expect("bounded peer input");
                    self.peer.process();
                }
                LowerRequest::Stream(Down::Demand { .. } | Down::Send(_) | Down::Finish)
                | LowerRequest::Output(OutputDown::Cancel { .. } | OutputDown::Release { .. }) => {
                    unreachable!("positive handshake fixture requests")
                }
            }
        }
        assert!(self.machine.below.is_empty(), "all output below observed");
    }

    fn receive(&mut self) {
        if let Some((right, bytes)) = self.room.take() {
            assert!(self.grant.replace((right, bytes)).is_none(), "one fixture grant");
            self.machine.up(LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Granted }));
        } else if let Some(requested) = self.read {
            let count = match requested {
                Read::Fill(count) => count,
                Read::Nothing | Read::Line { .. } | Read::Scan { .. } | Read::Arrived { .. } => {
                    unreachable!("exact record Fill")
                }
            };
            assert!(self.peer.output.len() >= count, "server supplies pending record");
            self.read = None;
            self.machine.up(LowerEvent::Stream(Up::Bytes(self.peer.output.take(count))));
        } else {
            unreachable!("handshake retains work below");
        }
    }
}

impl Machine {
    fn ready() -> Machine {
        let mut handshake = Handshake::new();
        for _ in 0..512_u32 {
            handshake.take();
            handshake.receive();
            if !handshake.machine.above.is_empty() {
                assert_eq!(handshake.machine.above.len(), 1, "one Ready without exhaustion");
                assert_eq!(
                    handshake.machine.above.pop(),
                    Some(Event::Client(client::Event::Ready(client::Agreed {
                        version: client::Version::Tls12,
                        alpn: None,
                    })))
                );
                assert!(handshake.machine.above.is_empty() && handshake.machine.below.is_empty());
                return handshake.machine;
            }
        }
        unreachable!("bounded handshake reaches Ready");
    }
}

#[test]
fn output_failure_settles_before_owner_closure() {
    let mut machine = Machine::new();
    let right = machine.started();
    let upper = Token::new(100);
    wanted(&mut machine, upper);
    let fault = Fault::Reset;
    machine.up(LowerEvent::Output(OutputUp::Settled { right, outcome: OutputOutcome::Failed(fault) }));
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: upper, outcome: OutputOutcome::Failed(fault) }))
    );
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Stream(Up::Failed(fault)))));
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Failed(client::Error::Stream(fault)))));
    assert_eq!(machine.below.pop(), Some(LowerRequest::Stream(WITHDRAW)));
    assert!(machine.above.is_empty() && machine.below.is_empty());
    machine.up(LowerEvent::Stream(Up::Failed(fault)));
    assert!(
        machine.above.is_empty() && machine.below.is_empty(),
        "the lower's following classic Failed cannot duplicate real native settlement"
    );
    machine.down(Request::Client(client::Request::Close));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "Close alone is not physical closure");
    machine.up(LowerEvent::Closed);
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Closed)));
    assert!(machine.above.is_empty() && machine.below.is_empty());
}

#[test]
fn token_exhaustion_on_room_fails_three_events_down() {
    let mut machine = Machine::ready();
    let upper = Token::new(101);
    machine.client.next_lower = None;
    machine.down(Request::Output(OutputDown::Room { right: upper, bytes: 1 }));
    assert_eq!(machine.above.len(), 3, "maximum down upper co-emission");
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: upper, outcome: OutputOutcome::Failed(Fault::Other) }))
    );
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Stream(Up::Failed(Fault::Other)))));
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Failed(client::Error::Other))));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "exhaustion before lower right or read effects");
}

#[test]
fn ready_survives_token_exhaustion_in_the_same_step() {
    let mut handshake = Handshake::new();
    handshake.take();
    let upper = Token::new(900);
    wanted(&mut handshake.machine, upper);
    for _ in 0..512_u32 {
        handshake.take();
        if !handshake.peer.tls.is_handshaking() {
            handshake.machine.client.next_lower = None;
        }
        handshake.receive();
        if !handshake.machine.above.is_empty() {
            break;
        }
    }
    let machine = &mut handshake.machine;
    assert_eq!(machine.above.len(), 4, "Ready and all exhaustion events");
    assert_eq!(
        machine.above.pop(),
        Some(Event::Client(client::Event::Ready(client::Agreed { version: client::Version::Tls12, alpn: None })))
    );
    assert_eq!(
        machine.above.pop(),
        Some(Event::Output(OutputUp::Settled { right: upper, outcome: OutputOutcome::Failed(Fault::Other) }))
    );
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Stream(Up::Failed(Fault::Other)))));
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Failed(client::Error::Other))));
    assert!(machine.above.is_empty() && machine.below.is_empty(), "no new right below after exhaustion");
}

#[test]
#[should_panic(expected = "ciphertext Fill is answered exactly")]
fn ciphertext_must_answer_the_fill_exactly() {
    let mut machine = Machine::new();
    let right = machine.started();
    machine.sent_hello(right);
    machine.up(LowerEvent::Stream(Up::Bytes(Box::from([0; 4]))));
}

#[test]
fn a_read_of_what_has_arrived_takes_one_records_plaintext() {
    use rustls::unbuffered::ConnectionState;
    let mut handshake = Handshake::new();
    for _ in 0..512_u32 {
        handshake.take();
        handshake.receive();
        if !handshake.machine.above.is_empty() {
            assert_eq!(
                handshake.machine.above.pop(),
                Some(Event::Client(client::Event::Ready(client::Agreed {
                    version: client::Version::Tls12,
                    alpn: None
                })))
            );
            break;
        }
    }
    let mut ciphertext = [0; 128];
    let status = handshake.peer.tls.process_tls_records(&mut []);
    let ConnectionState::WriteTraffic(mut traffic) = status.state.unwrap() else { panic!("server ready") };
    let count = traffic.encrypt(b"hello", &mut ciphertext).unwrap();
    let machine = &mut handshake.machine;
    machine.down(Request::Client(client::Request::Stream(Down::Demand { read: Read::Arrived { max: 64 }, room: 0 })));
    assert_eq!(
        machine.below.pop(),
        Some(LowerRequest::Stream(Down::Demand { read: Read::Fill(client::HEADER), room: 0 }))
    );
    machine.up(LowerEvent::Stream(Up::Bytes(Box::from(&ciphertext[..5]))));
    assert_eq!(
        machine.below.pop(),
        Some(LowerRequest::Stream(Down::Demand { read: Read::Fill(u32::try_from(count - 5).unwrap()), room: 0 }))
    );
    machine.up(LowerEvent::Stream(Up::Bytes(Box::from(&ciphertext[5..count]))));
    assert_eq!(machine.above.pop(), Some(Event::Client(client::Event::Stream(Up::Bytes(Box::from(&b"hello"[..]))))));
    assert!(machine.above.is_empty() && machine.below.is_empty());
}
