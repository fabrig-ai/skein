//! The client's machine world (testing-strategy.md, 2.4 and 4.4): one
//! client, from a seed, between its two neighbours, both played by the
//! world in one loop.
//!
//! - **The side below** is a rustls server's ciphertext: what it writes
//!   arrives in pieces cut at random, late, into an intake under its cap,
//!   and meets each fill exactly; room is granted late, one `Send` a grant,
//!   and what the client sends is the server's to read. The server answers
//!   the handshake, then, once it has the whole request (or at once, if it
//!   is eager), its response, a key update in the middle of it if the
//!   scenario asks; then it ends as the scenario says: `close_notify`, a
//!   stream that ends without it, a record corrupted on the way, or nothing.
//!   The stream may also fail, at a moment the settings draw.
//! - **The side above** starts the handshake, writes its request in pieces
//!   within the room it is granted, reads with fills and scans of every
//!   shape, slowly, stops for a while, finishes once its request is sent if
//!   the settings say so, and closes: once the stream ended or failed, or
//!   at a moment the settings draw, whatever the client is doing,
//!   withdrawing its demand first now and then.
//!
//! The world checks both streams as it goes (testing-strategy.md, 6):
//! Below, one demand at a time, fills only, none
//! past [`client::LARGEST_READ`] or the caps, none once the stream ended or
//! failed, a withdrawal only as the client closes, each `Send` within the
//! room granted, nothing after `Finish`; above, `Ready` once and before
//! anything on the stream, each answer for a demand and exactly what it
//! reads, `End` only after the server's `close_notify`, `End` and `Failed`
//! once, `Failed` after the stream heard it, `Closed` once and last; and
//! the client waiting for what its neighbours see. [`check`] then holds the
//! run to what the scenario implies.
//!
//! A run is not replayed: rustls's randoms come from the kernel, so a
//! record's length changes from run to run, and with it where the pieces
//! fall. Nothing asserted depends on it.

use std::collections::VecDeque;

use skein_lib::stream::{Delimiter, Down, Fault, Read, Up};
use skein_lib::{Env, Intake, Queue, Rng, Time};
use skein_tls::client::{self, Agreed, Client, Error, Event, Limits, Request, Version, Waiting};

use crate::pki;
use crate::server::Server;

mod check;
mod scenario;
pub use check::check;
pub use scenario::{Ending, Reads, Scenario, Settings, limits, pick, text};
use scenario::{draw, fault};

/// What a run came to, as the neighbours saw it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Run {
    pub agreed: Option<Agreed>,
    /// What the side above read.
    pub received: Vec<u8>,
    /// The demand `End` answered, or `Read::Nothing` if it came with none.
    pub ended: Option<Read>,
    /// The fault the plaintext stream failed with, if it did.
    pub stream_failed: Option<Fault>,
    pub failed: Option<Error>,
    /// What the server read of the request.
    pub served: Vec<u8>,
    /// Whether the server read the client's `close_notify`.
    pub notified: bool,
    /// Whether the side above sent its whole request, and finished.
    pub sent: bool,
    pub finished: bool,
    /// What the client waited for when the side above closed it.
    pub closed_while: Waiting,
    /// The fault the stream below failed with, if it did.
    pub below_failed: Option<Fault>,
    /// The server's error, if what the client sent broke TLS.
    pub server_failed: Option<String>,
    pub fell: Fell,
}

/// What fell in a run, of what its neighbours may inject: a sweep asserts
/// that each fell at least once (testing-strategy.md, 3).
#[expect(clippy::struct_excessive_bools, reason = "a record of what fell, a flag each")]
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Fell {
    /// Room came while bytes could have met the same demand.
    pub room_first: bool,
    /// The intake below filled while the client read nothing: backpressure.
    pub stalled_below: bool,
    /// A demand of the side above's waited for the handshake.
    pub early_demand: bool,
    /// The side above withdrew its demand as it closed.
    pub withdrew: bool,
    /// An answer came after the client withdrew its demand below.
    pub late_answer: bool,
    /// The stream ended with nothing demanded.
    pub idle_end: bool,
    /// A response that came before the request was all sent.
    pub early_response: bool,
    /// The side above withdrew the read that crossed the end, to write on.
    pub withdrew_after_end: bool,
    /// It withdrew a demand that asked room too, after the end crossed its
    /// read.
    pub withdrew_room_after_end: bool,
    /// The server updated its keys, and the client read records sealed
    /// with the new ones.
    pub updated_keys: bool,
}

struct World<'a> {
    rng: Rng,
    settings: &'a Settings,
    scenario: &'a Scenario,
    env: Env<Limits>,
    client: Client,
    events: Queue<Event>,
    requests: Queue<Down>,
    server: Side,
    below: Below,
    above: Above,
    fell: Fell,
    iteration: u64,
    /// The last iteration something moved.
    moved: u64,
}

/// The server, and what it did of the scenario.
struct Side {
    server: Server,
    responded: bool,
    ended: bool,
    /// How much of the response went before the server's key update, if it
    /// sent one.
    updated_at: Option<usize>,
}

/// The ciphertext stream.
struct Below {
    /// The server's bytes on their way.
    wire: VecDeque<u8>,
    /// The server closed its side: once the wire is empty, the stream ends.
    eof: bool,
    intake: Intake,
    /// The client's demand outstanding: its read, and its room.
    demand: Option<(Read, u32)>,
    /// Room granted and not yet sent in.
    granted: Option<u32>,
    life: Life,
    failed: Option<Fault>,
    /// The demand the client withdrew, which an answer on its way may still
    /// meet.
    withdrawn: Option<(Read, u32)>,
    finished: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Life {
    Open,
    /// The end arrived: room may still be granted, nothing read.
    Ended,
    Failed,
}

/// The side above.
#[expect(clippy::struct_excessive_bools, reason = "the side above's state, a flag each")]
struct Above {
    started: bool,
    face: Face,
    /// How much of the request went down.
    sent: usize,
    finished: bool,
    received: Vec<u8>,
    ended: Option<Read>,
    /// The plaintext stream failed, or ended: it reads no more.
    reading_over: bool,
    stream_failed: Option<Fault>,
    failed: Option<Error>,
    agreed: Option<Agreed>,
    closing: Option<Waiting>,
    closed: bool,
}

/// The side above's side of the plaintext stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Face {
    Idle,
    /// A demand outstanding: its read and its room.
    Demanded(Read, u32),
    /// Room granted: it may send this much, once.
    Granted(u32),
    /// The stream failed: nothing more on it.
    Over,
}

/// Runs the world for `scenario` with `settings`, from `seed`, until the
/// side above has closed the client.
#[must_use]
pub fn run(scenario: &Scenario, settings: &Settings, seed: u64) -> Run {
    let mut world = World::new(scenario, settings, seed);
    let budget = 64 * (scenario.request.len() + scenario.response.len()) as u64 + 200_000;
    while world.iteration < budget {
        world.server_acts();
        if world.rng.chance(500) {
            world.below_acts();
            world.above_acts();
        } else {
            world.above_acts();
            world.below_acts();
        }
        world.iteration += 1;
        if world.above.closed {
            // What failed before the close, not what fails after it.
            let below_failed = world.below.failed;
            world.late();
            world.fell.updated_keys = world.server.updated_at.is_some_and(|at| world.above.received.len() > at);
            let above = world.above;
            return Run {
                agreed: above.agreed,
                received: above.received,
                ended: above.ended,
                stream_failed: above.stream_failed,
                failed: above.failed,
                served: world.server.server.received,
                notified: world.server.server.closed,
                sent: above.sent == scenario.request.len(),
                finished: above.finished,
                closed_while: above.closing.expect("closed after a close"),
                below_failed,
                server_failed: world.server.server.failed.map(|error| format!("{error:?}")),
                fell: world.fell,
            };
        }
    }
    panic!("seed {seed}: the world settles within {budget} iterations: {settings:?}");
}

/// Whether `bytes` hold what `read` asks for, from their start.
fn meets(read: Read, bytes: &[u8]) -> bool {
    match read {
        Read::Nothing | Read::Arrived { .. } => !bytes.is_empty(),
        Read::Fill(n) => bytes.len() >= usize::try_from(n).expect("fits a usize"),
        Read::Scan { until, max } => {
            let max = usize::try_from(max).expect("fits a usize");
            let window = &bytes[..max.min(bytes.len())];
            bytes.len() >= max || window.windows(until.as_bytes().len()).any(|found| found == until.as_bytes())
        }
        Read::Line { max } => {
            let max = usize::try_from(max).expect("fits a usize");
            bytes.len() >= max || bytes[..max.min(bytes.len())].iter().any(|&byte| byte == b'\r' || byte == b'\n')
        }
    }
}

/// Whether `bytes` meet `read` exactly, or with what has arrived.
fn delivers(read: Read, bytes: &[u8]) -> bool {
    match read {
        Read::Nothing => false,
        Read::Fill(n) => bytes.len() == usize::try_from(n).expect("fits a usize"),
        Read::Arrived { max } => !bytes.is_empty() && bytes.len() <= usize::try_from(max).expect("fits a usize"),
        Read::Scan { until, max } => {
            let delimiter = until.as_bytes();
            match bytes.windows(delimiter.len()).position(|window| window == delimiter) {
                Some(at) => at + delimiter.len() == bytes.len(),
                None => bytes.len() == usize::try_from(max).expect("fits a usize"),
            }
        }
        Read::Line { max } => match bytes.iter().position(|&byte| byte == b'\r' || byte == b'\n') {
            Some(at) => at + 1 == bytes.len(),
            None => bytes.len() == usize::try_from(max).expect("fits a usize"),
        },
    }
}

impl<'a> World<'a> {
    fn new(scenario: &'a Scenario, settings: &'a Settings, seed: u64) -> Self {
        let limits = settings.limits;
        assert!(settings.cap >= client::LARGEST_READ, "the side below's cap holds the largest read");
        assert!(settings.output >= client::largest_room(&limits), "the side below's output holds the largest room");
        let alpn: Vec<&[u8]> = scenario.alpn.iter().map(Vec::as_slice).collect();
        let config = pki::client(&alpn);
        let name = skein_tls::Name::new(&scenario.name).expect("a name");
        Self {
            rng: Rng::new(seed),
            settings,
            scenario,
            env: Env { now: Time::ZERO, wall: scenario.wall, limits },
            client: Client::new(&config, name, &limits),
            events: Queue::uncapped(),
            requests: Queue::uncapped(),
            server: Side {
                server: Server::new(scenario.server.config()),
                responded: false,
                ended: false,
                updated_at: None,
            },
            below: Below {
                wire: VecDeque::new(),
                eof: false,
                intake: Intake::with_capacity(settings.cap),
                demand: None,
                granted: None,
                life: Life::Open,
                failed: None,
                withdrawn: None,
                finished: false,
            },
            above: Above {
                started: false,
                face: Face::Idle,
                sent: 0,
                finished: false,
                received: Vec::new(),
                ended: None,
                reading_over: false,
                stream_failed: None,
                failed: None,
                agreed: None,
                closing: None,
                closed: false,
            },
            fell: Fell::default(),
            iteration: 0,
            moved: 0,
        }
    }

    /// What the side below may still deliver once the client is closed: an
    /// answer on its way for the demand the close withdrew, then the end or
    /// a failure.
    fn late(&mut self) {
        if let Some((read, room)) = self.below.withdrawn.take()
            && self.rng.chance(500)
        {
            if let Some(bytes) = self.below.intake.meet(read) {
                self.fell.late_answer = true;
                self.up(Up::Bytes(bytes));
            } else if room > 0 {
                self.fell.late_answer = true;
                self.up(Up::Room);
            }
        }
        if self.below.life == Life::Open && self.rng.chance(500) {
            self.end();
        }
        if self.below.life != Life::Failed && self.rng.chance(300) {
            let fault = fault(&mut self.rng);
            self.fail(fault);
        }
    }

    /// The stream ends. A read outstanding is never met, and stays
    /// outstanding until the client withdraws it, as io keeps it (lib.md,
    /// 7); its room may still be granted.
    fn end(&mut self) {
        self.below.life = Life::Ended;
        self.up(Up::End);
    }

    fn fail(&mut self, fault: Fault) {
        self.below.life = Life::Failed;
        self.below.demand = None;
        self.below.granted = None;
        self.below.failed = Some(fault);
        self.up(Up::Failed(fault));
    }

    /// The server writes its response once it may, then ends as the
    /// scenario says.
    fn server_acts(&mut self) {
        let side = &mut self.server;
        if side.server.failed.is_some() || side.server.handshaking() || self.below.life == Life::Failed {
            return;
        }
        let scenario = self.scenario;
        if !side.responded && (scenario.eager || side.server.received.len() >= scenario.request.len()) {
            side.responded = true;
            self.fell.early_response |= self.above.sent < scenario.request.len();
            let half = scenario.response.len() / 2;
            side.server.write(&scenario.response[..half]);
            let tls13 = self.above.agreed.as_ref().is_some_and(|agreed| agreed.version == Version::Tls13);
            if scenario.key_update && tls13 {
                side.server.key_update();
                side.updated_at = Some(half);
            }
            side.server.write(&scenario.response[half..]);
            let mut records = side.server.transmit();
            if scenario.ending == Ending::Corrupt {
                corrupt(&mut records, &mut self.rng);
            }
            self.below.wire.extend(records);
            return;
        }
        if side.responded && !side.ended && self.rng.chance(300) {
            side.ended = true;
            match scenario.ending {
                Ending::CloseNotify => {
                    side.server.close_notify();
                    let records = side.server.transmit();
                    self.below.wire.extend(records);
                    self.below.eof = true;
                }
                Ending::Truncate | Ending::Corrupt => self.below.eof = true,
                Ending::Silent => {}
            }
        }
    }

    fn below_acts(&mut self) {
        if let Some((at, fault)) = self.settings.failure
            && self.iteration >= at
            && self.below.failed.is_none()
        {
            self.fail(fault);
            return;
        }
        let below = &mut self.below;
        if below.life == Life::Failed {
            return;
        }
        if !below.wire.is_empty() && self.rng.chance(self.settings.arrival) {
            if below.intake.room() == 0 {
                self.fell.stalled_below = true;
            } else {
                let piece = usize::try_from(self.rng.between(1, u64::from(self.settings.piece))).expect("fits a usize");
                let room = usize::try_from(below.intake.room()).expect("fits a usize");
                let piece: Vec<u8> = below.wire.drain(..piece.min(room).min(below.wire.len())).collect();
                below.intake.append(&piece).expect("within the room");
                self.moved = self.iteration;
            }
        }
        let ended = below.life == Life::Ended;
        let over = below.eof && below.wire.is_empty();
        match below.demand {
            Some((read, room)) => {
                let held = below.intake.len();
                let met = if ended { None } else { below.intake.meet(read) };
                if let Some(bytes) = met {
                    assert!(delivers(read, &bytes), "a delivery meets the demand");
                    assert!(bytes.len() <= usize::try_from(held).expect("fits a usize"), "no more than was held");
                    below.demand = None;
                    self.up(Up::Bytes(bytes));
                } else if room > 0 && self.rng.chance(self.settings.grant) {
                    if read != Read::Nothing && !below.intake.is_empty() {
                        self.fell.room_first = true;
                    }
                    below.demand = None;
                    below.granted = Some(room);
                    self.up(Up::Room);
                } else if !ended && over && read != Read::Nothing {
                    // It can never be met.
                    self.end();
                }
            }
            None => {
                if !ended && over && below.intake.is_empty() && self.rng.chance(200) {
                    self.fell.idle_end = true;
                    self.end();
                }
            }
        }
    }

    fn is_stalled(&self) -> bool {
        match self.settings.stall {
            Some((from, iterations)) => (from..from + iterations).contains(&self.iteration),
            None => false,
        }
    }

    fn above_acts(&mut self) {
        if !self.above.started {
            self.above.started = true;
            self.down(Request::Handshake);
            return;
        }
        if self.is_stalled() || self.above.closing.is_some() {
            return;
        }
        let closes_now = match self.settings.close {
            Some(at) => self.iteration >= at,
            None => false,
        };
        // Nothing more will come, or nothing has moved for long enough that
        // nothing will: a server that is silent, a demand no byte left meets.
        let sent = self.above.sent == self.scenario.request.len() && (self.above.finished || !self.settings.finish);
        let done =
            self.above.failed.is_some() || (self.above.reading_over && sent) || self.iteration > self.moved + 2_000;
        if closes_now || (done && self.rng.chance(self.settings.eagerness)) {
            self.close();
            return;
        }
        if !self.rng.chance(self.settings.eagerness) {
            return;
        }
        let left = self.scenario.request.len() - self.above.sent;
        match self.above.face {
            Face::Idle if left == 0 && self.settings.finish && !self.above.finished => {
                self.above.finished = true;
                self.down(Request::Stream(Down::Finish));
            }
            Face::Idle => self.demand_above(left),
            Face::Granted(room) => self.send_above(room, left),
            // A read that crossed the end stays outstanding, never met: the
            // side above withdraws it to write on, or to finish.
            Face::Demanded(_, 0)
                if self.above.ended.is_some() && (left > 0 || self.settings.finish && !self.above.finished) =>
            {
                self.fell.withdrew_after_end = true;
                self.above.face = Face::Idle;
                self.down(Request::Stream(Down::Demand { read: Read::Nothing, room: 0 }));
            }
            // Now and then even one that asked room too, which it asks for
            // again: room that comes below meanwhile is held for no one.
            Face::Demanded(read, room)
                if self.above.ended.is_some() && read != Read::Nothing && room > 0 && self.rng.chance(300) =>
            {
                self.fell.withdrew_room_after_end = true;
                self.above.face = Face::Idle;
                self.down(Request::Stream(Down::Demand { read: Read::Nothing, room: 0 }));
            }
            Face::Demanded(..) | Face::Over => {}
        }
    }

    fn demand_above(&mut self, left: usize) {
        let reads = !self.above.reading_over && (left == 0 || self.rng.chance(500));
        let read = if reads { self.draw_read() } else { Read::Nothing };
        // Now and then it reads before it writes on, from a server
        // that answers at once and ends the stream: a patient one, or
        // a silent one, would leave it waiting for ever.
        let ends = self.scenario.eager && self.scenario.ending != Ending::Silent;
        let writes = !reads || !ends || self.rng.chance(700);
        let room = if left > 0 && !self.above.finished && writes {
            let most = self.settings.limits.send.min(u32::try_from(left).expect("fits a u32"));
            draw(&mut self.rng, 1, most)
        } else {
            0
        };
        if read == Read::Nothing && room == 0 {
            return;
        }
        self.fell.early_demand |= self.above.agreed.is_none();
        self.above.face = Face::Demanded(read, room);
        self.down(Request::Stream(Down::Demand { read, room }));
    }

    fn send_above(&mut self, room: u32, left: usize) {
        let most = usize::try_from(room).expect("fits a usize").min(left);
        let len = usize::try_from(self.rng.between(0, most as u64)).expect("fits a usize");
        let piece = self.scenario.request[self.above.sent..self.above.sent + len].to_vec();
        self.above.sent += len;
        self.above.face = Face::Idle;
        self.down(Request::Stream(Down::Send(piece.into())));
    }

    /// The side above closes: now and then withdrawing its demand first, as
    /// a machine stacked above withdraws what it demanded as it closes.
    fn close(&mut self) {
        if let Face::Demanded(..) = self.above.face
            && self.rng.chance(500)
        {
            self.fell.withdrew = true;
            self.above.face = Face::Over;
            self.down(Request::Stream(Down::Demand { read: Read::Nothing, room: 0 }));
        }
        self.above.closing = Some(self.client.waiting());
        self.down(Request::Close);
    }

    fn draw_read(&mut self) -> Read {
        let most = self.settings.limits.read;
        match self.settings.reads {
            Reads::Bytes => Read::Fill(1),
            Reads::Any => {
                let n = draw(&mut self.rng, 1, most);
                match self.rng.below(6) {
                    0 => Read::Fill(n),
                    1 => Read::Scan { until: Delimiter::LF, max: n },
                    2 if n >= 2 => Read::Scan { until: Delimiter::CRLF, max: n },
                    3 => Read::Line { max: n },
                    4 => Read::Arrived {
                        max: if self.rng.below(16) == 0 { most.checked_add(1).expect("small read cap") } else { n },
                    },
                    _ => Read::Scan { until: Delimiter::new(b"\"").expect("one byte"), max: n },
                }
            }
        }
    }

    fn up(&mut self, ev: Up) {
        client::up(&mut self.client, &self.env, ev, &mut self.events, &mut self.requests);
        self.route(false);
    }

    fn down(&mut self, rq: Request) {
        let closing = rq == Request::Close;
        client::down(&mut self.client, &self.env, rq, &mut self.events, &mut self.requests);
        self.route(closing);
    }

    /// What one call emitted, to each side, checked.
    fn route(&mut self, closing: bool) {
        if !self.events.is_empty() || !self.requests.is_empty() {
            self.moved = self.iteration;
        }
        while let Some(event) = self.events.pop() {
            self.receive(event);
        }
        while let Some(request) = self.requests.pop() {
            self.send(request, closing);
        }
        self.check_waiting();
    }

    /// An event for the side above, checked against its contract.
    fn receive(&mut self, event: Event) {
        assert!(!self.above.closed, "nothing follows Closed: {event:?}");
        let above = &mut self.above;
        match event {
            Event::Ready(agreed) => {
                assert!(above.agreed.is_none() && above.failed.is_none(), "Ready once, before any failure");
                above.agreed = Some(agreed);
            }
            Event::Stream(Up::Bytes(bytes)) => {
                assert!(above.agreed.is_some(), "nothing on the stream before Ready");
                let Face::Demanded(read, _) = above.face else { panic!("bytes for a demand outstanding") };
                assert!(read != Read::Nothing, "bytes answer a read");
                assert!(delivers(read, &bytes), "what the demand reads: {read:?}, {}", bytes.escape_ascii());
                above.face = Face::Idle;
                above.received.extend_from_slice(&bytes);
            }
            Event::Stream(Up::Room) => {
                assert!(above.agreed.is_some(), "no room before Ready");
                let Face::Demanded(_, room) = above.face else { panic!("room for a demand outstanding") };
                assert!(room > 0, "room answers a demand for room");
                above.face = Face::Granted(room);
            }
            Event::Stream(Up::End) => {
                assert!(above.ended.is_none() && above.stream_failed.is_none(), "End once, before any failure");
                assert!(self.server.server.handshaking() || self.below.eof, "End once the server closed");
                // A read outstanding stays so, never met, until withdrawn.
                above.ended = Some(match above.face {
                    Face::Demanded(read, _) => read,
                    Face::Idle | Face::Granted(_) | Face::Over => Read::Nothing,
                });
                above.reading_over = true;
            }
            Event::Stream(Up::Failed(fault)) => {
                assert!(above.stream_failed.is_none() && above.failed.is_none(), "the stream fails once, first");
                above.stream_failed = Some(fault);
                above.face = Face::Over;
                above.reading_over = true;
            }
            Event::Failed(error) => {
                assert!(above.failed.is_none(), "Failed once");
                assert!(
                    above.stream_failed == Some(error.fault()) || self.fell.withdrew,
                    "the stream heard the failure first: {error:?}"
                );
                above.failed = Some(error);
                above.face = Face::Over;
                above.reading_over = true;
            }
            Event::Closed => {
                assert!(above.closing.is_some(), "Closed answers a Close");
                above.closed = true;
            }
        }
    }

    /// A request for the side below, checked against its contract.
    fn send(&mut self, request: Down, closing: bool) {
        let below = &mut self.below;
        match request {
            Down::Demand { read: Read::Nothing, room: 0 } => {
                assert!(
                    closing || below.life == Life::Ended,
                    "a withdrawal only as the client closes, or reads no more after the end"
                );
                let withdrawn = below.demand.take().expect("only a demand outstanding is withdrawn");
                below.withdrawn = Some(withdrawn);
            }
            Down::Demand { read, room } => {
                assert!(below.demand.is_none(), "one demand at a time: {read:?} over {:?}", below.demand);
                assert!(below.life != Life::Failed, "nothing demanded after a failure");
                match read {
                    Read::Nothing => {}
                    Read::Fill(n) => {
                        assert!(below.life == Life::Open, "nothing read after the end");
                        assert!(n <= client::LARGEST_READ && n <= below.intake.capacity(), "no read past the caps");
                    }
                    Read::Scan { .. } | Read::Line { .. } | Read::Arrived { .. } => {
                        panic!("the client reads records by fills")
                    }
                }
                assert!(room <= client::largest_room(&self.settings.limits), "no room past the largest declared");
                assert!(room <= self.settings.output, "no room past the output cap");
                assert!(room == 0 || !below.finished, "no room after Finish");
                // A grant not sent within (the side above sent nothing in it)
                // gives way to the next, as io's does (io.md, 3.3).
                if room > 0 {
                    below.granted = None;
                }
                below.demand = Some((read, room));
            }
            Down::Send(bytes) => {
                assert!(below.life != Life::Failed, "nothing sent after a failure");
                assert!(!below.finished, "nothing sent after Finish");
                let granted = below.granted.take().expect("a Send within room granted");
                assert!(bytes.len() <= usize::try_from(granted).expect("fits a usize"), "a Send within room granted");
                self.server.server.receive(&bytes);
                let records = self.server.server.transmit();
                self.below.wire.extend(records);
            }
            Down::Finish => {
                assert!(!below.finished, "one Finish");
                below.finished = true;
            }
        }
    }

    /// The client waits for exactly what its neighbours see.
    fn check_waiting(&self) {
        let waiting = self.client.waiting();
        let seen = if self.above.closed {
            Waiting::Nothing
        } else if !self.above.started {
            Waiting::Handshake
        } else if self.above.failed.is_some() {
            Waiting::Close
        } else if self.above.closing.is_some() {
            Waiting::Room
        } else if self.above.agreed.is_none() {
            Waiting::Handshaking
        } else {
            match self.below.demand {
                Some((_, room)) if room > 0 => Waiting::Room,
                Some(_) => Waiting::Bytes,
                None => Waiting::Above,
            }
        };
        assert_eq!(waiting, seen, "the client waits for what its neighbours see");
    }
}

/// Changes a byte in the body of one of `records`, at random: what was a
/// record of the response no longer decrypts.
fn corrupt(records: &mut [u8], rng: &mut Rng) {
    let mut bodies = Vec::new();
    let mut at = 0;
    while at + 5 <= records.len() {
        let length = crate::records::length(&records[at..]);
        bodies.push((at + 5, length));
        at += 5 + length;
    }
    assert!(!bodies.is_empty(), "a response of a byte at least, to corrupt");
    let (start, length) = bodies[usize::try_from(rng.below(bodies.len() as u64)).expect("fits a usize")];
    let offset = start + usize::try_from(rng.below(length as u64)).expect("fits a usize");
    records[offset] ^= u8::try_from(rng.between(1, 255)).expect("a byte");
}
