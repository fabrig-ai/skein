//! The TLS client (tls.md): a step machine that wraps rustls's unbuffered
//! client connection between two streams (lib.md, 7).
//!
//! # Its two sides
//!
//! Below, the ciphertext: a `lib::stream` to the server, a socket's or a
//! pipe's. The client reads it a record at a time, its header (a fill of 5)
//! then its body (a fill of the length the header gives), and hands each to
//! rustls, which deciphers it in place. It writes within room it asks for:
//! TLS's own output (a flight of the handshake, `close_notify`) alone, and
//! what the side above sends, encrypted, in one `Send` a grant.
//!
//! Above, the service's protocol layer ([`Request`] down, [`Event`] up), and
//! the plaintext: a `lib::stream` of which the client is the side below,
//! and which the machine above cannot tell from a socket.
//!
//! - **`Handshake` starts the connection,** first and once. `Ready`
//!   answers it, with what was agreed, or `Failed`. The plaintext stream
//!   may be demanded before `Ready`: its demands wait for the handshake.
//! - **The plaintext stream** keeps the contract of a stream (lib.md, 7):
//!   each demand answered at most once, exactly; `End` once the server's
//!   `close_notify` is read and nothing held meets a demand; `Finish` sends
//!   `close_notify`, then finishes the stream below.
//! - **`Failed` says why the connection failed,** once, after the
//!   plaintext stream was told it failed, `Failed(fault)`: a record that
//!   fails to decrypt, or any other error of the peer's data, is
//!   `Fault::Invalid`. A stream below that ends before the server's
//!   `close_notify` is a truncation, which an attacker can cause (RFC 8446,
//!   6.1): what the client deciphered before it still meets demands, and
//!   then the stream fails, never ends.
//! - **`Close` ends the client in any state.** Once the handshake is done
//!   it sends `close_notify` first, unless it was sent, and answers `Closed`
//!   once it went below; otherwise at once. It withdraws what it demanded
//!   below, and does not close the stream below, which its owner closes.
//!
//! # Bounds
//!
//! The plaintext the side above has not demanded is held in an intake of
//! [`Limits::read`] and one record's plaintext beside it; the ciphertext
//! rustls has not discarded, within [`Limits::records`]; TLS's own output,
//! within [`FLIGHT`]. The client reads a record only when the intake has
//! room for its plaintext, so rustls never holds plaintext of its own
//! between steps. [`worst_case`] is what a client holds, rustls's heap
//! included; [`LARGEST_READ`] and [`largest_room`] are what whoever stacks
//! it checks against the caps of the stream below at startup.

/// The client with independent output on both sides (tls.md, 3.6; lib.md, 7.1).
pub mod native;

mod above;
mod below;
mod close;
mod error;
mod limits;
pub use error::{Certificate, Error};
#[cfg_attr(not(test), expect(unused_imports, reason = "The crate's step tests use this client path"))]
pub(crate) use limits::MAX_HANDSHAKE;
pub use limits::{
    FLIGHT, HEADER, LARGEST_READ, Limits, MAX_BODY, MAX_PLAINTEXT, MAX_RECORD, largest_room,
    room_for, valid, worst_case,
};

use above::{Above, Answer, Reading, Wanted, answer, stream};
use below::{Below, Demand, Record, Room, failed_up, open_up, wanted};
use close::{Closing, close, closing_up, fail};
use core::mem;
use limits::intake_cap;

use alloc::boxed::Box;

use skein_lib::stream::{Down, Fault, Up};
use skein_lib::{Env, Intake, Queue, Wall};

use crate::held::Held;
use crate::session::{self, Connection};
use crate::{Config, Name};

/// From the side above.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Request {
    /// Starts the handshake, first and once. `Ready` or `Failed` answers it.
    Handshake,
    /// The plaintext stream (lib.md, 7), once the handshake is asked for: a
    /// demand of at most [`Limits::read`] and [`Limits::send`], waiting for
    /// the handshake if it is not done; a `Send` within the room granted,
    /// one a grant, empty or not; `Finish`, with no demand outstanding,
    /// which sends `close_notify`. A demand of room comes only once the last
    /// grant was sent within, and none after `Finish`. A read that crosses
    /// `End` stays outstanding until it is withdrawn, as io's does.
    Stream(Down),
    /// Closes the client, in any state. `Closed` answers it.
    Close,
}

/// To the side above.
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum Event {
    /// For `Handshake`: the handshake is done, and this was agreed.
    Ready(Agreed),
    /// The plaintext stream: `Bytes`, `Room`, `End` once the server's
    /// `close_notify` was read, or `Failed`.
    Stream(Up),
    /// The connection failed: for `Handshake`, or after `Ready`. The
    /// plaintext stream was told first, unless its demand was withdrawn.
    /// Nothing follows but `Closed`.
    Failed(Error),
    /// For `Close`: the client is closed. Terminal.
    Closed,
}

/// What a handshake agreed.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Agreed {
    pub version: Version,
    /// The protocol the server chose of those offered by ALPN, if it chose
    /// one (RFC 7301).
    pub alpn: Option<Box<[u8]>>,
}

/// The version of TLS a handshake agreed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Version {
    Tls12,
    Tls13,
}

/// What the client is waiting for. Machines keep no timers
/// (programming-model.md, 4): each says what it waits for, and the
/// connection arms the deadlines.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Waiting {
    /// For the side above to start the handshake.
    Handshake,
    /// For the handshake: the server's records, or room for TLS's own. The
    /// connection arms its handshake deadline.
    Handshaking,
    /// For room below: the peer is not reading what the client sends, or,
    /// closing, its `close_notify`.
    Room,
    /// For the server's records, for a demand of the side above's.
    Bytes,
    /// For the side above: to demand, send, finish or close.
    Above,
    /// For the side above to close it: it failed; on the native face, also
    /// while a close waits for the owner's `Closed`, and after `TransportClosing`.
    Close,
    /// For nothing: it is closed.
    Nothing,
}

/// A TLS client connection: its state for this machine.
#[derive(Debug)]
pub struct Client {
    state: State,
    buffers: Buffers,
}

/// What a client holds whatever its state, allocated with it.
#[derive(Debug)]
struct Buffers {
    /// The plaintext the side above has not demanded yet (lib.md, 7).
    intake: Intake,
    /// The ciphertext received, which rustls has not discarded.
    records: Held,
    /// What TLS owes the stream below: its flights, `close_notify`.
    owed: Held,
}

impl Buffers {
    /// The intake, the records held and what TLS owes, for `limits`.
    pub(in crate::client) fn new(limits: &Limits) -> Option<Buffers> {
        Some(Buffers {
            intake: Intake::with_capacity(intake_cap(limits)?),
            records: Held::with_capacity(limits.records),
            owed: Held::with_capacity(FLIGHT),
        })
    }
}

/// rustls's connection, or why the stream below already failed it.
pub(in crate::client) fn connect(fresh: Fresh, wall: Wall) -> Result<Connection, Error> {
    match fresh.line {
        Line::Open => session::connect(&fresh.config, fresh.name, wall),
        Line::Ended => Err(Error::Truncated),
        Line::Failed(fault) => Err(Error::Stream(fault)),
    }
}

/// `Some` once: when the step just finished the handshake.
pub(in crate::client) fn ready(tls: &Connection, handshake: &mut Handshake) -> Option<Agreed> {
    if *handshake == Handshake::Running && !session::handshaking(tls) {
        *handshake = Handshake::Done;
        Some(session::agreed(tls))
    } else {
        None
    }
}

impl Client {
    /// A client of the server `name`, with `config`, under `limits`, the
    /// limits its steps will be given. It demands nothing until the
    /// handshake is asked for.
    #[must_use]
    pub fn new(config: &Config, name: Name, limits: &Limits) -> Client {
        assert!(valid(limits), "the limits are honoured: a read and room of a byte, a record held");
        let buffers = Buffers::new(limits).expect("checked by valid");
        Client { state: State::Fresh(Fresh { config: config.clone(), name, line: Line::Open }), buffers }
    }

    /// What it is waiting for: a function of its state alone.
    #[must_use]
    pub fn waiting(&self) -> Waiting {
        match &self.state {
            State::Fresh(_) => Waiting::Handshake,
            State::Open(open) => match open.handshake {
                Handshake::Running => Waiting::Handshaking,
                Handshake::Done => match open.below.demand {
                    Some(demand) if demand.room.bytes() > 0 => Waiting::Room,
                    Some(_) => Waiting::Bytes,
                    None => Waiting::Above,
                },
            },
            State::Closing(_) => Waiting::Room,
            State::Failed(_) => Waiting::Close,
            State::Closed => Waiting::Nothing,
        }
    }
}

/// An event from the stream below. It has
/// the shape of every entry point, though nothing below reads `env`: the
/// wall time was read when the handshake started.
pub fn up(client: &mut Client, _env: &Env<Limits>, ev: Up, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    let buffers = &mut client.buffers;
    let state = mem::replace(&mut client.state, State::Closed);
    client.state = match state {
        State::Fresh(fresh) => State::Fresh(fresh_up(fresh, ev)),
        State::Open(open) => open_up(open, buffers, ev, above, below),
        State::Failed(demand) => State::Failed(failed_up(demand, ev)),
        State::Closing(closing) => closing_up(closing, &mut buffers.owed, ev, above, below),
        // An answer to the demand the close withdrew, on its way (lib.md, 7);
        // an end or a failure, which change nothing now.
        State::Closed => State::Closed,
    };
}

/// A request from the side above.
pub fn down(client: &mut Client, env: &Env<Limits>, rq: Request, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    let limits = &env.limits;
    let buffers = &mut client.buffers;
    let state = mem::replace(&mut client.state, State::Closed);
    client.state = match rq {
        Request::Handshake => match state {
            State::Fresh(fresh) => start(fresh, buffers, env, above, below),
            State::Open(_) | State::Failed(_) => unreachable!("one Handshake"),
            State::Closing(_) | State::Closed => unreachable!("a Handshake after Close"),
        },
        Request::Stream(down) => match state {
            State::Open(open) => stream(open, down, buffers, limits, above, below),
            // On its way when the failure went up: dropped.
            State::Failed(demand) => State::Failed(demand),
            State::Fresh(_) => unreachable!("the plaintext stream before the handshake was asked for"),
            State::Closing(_) | State::Closed => unreachable!("the plaintext stream after Close"),
        },
        Request::Close => close(state, buffers, above, below),
    };
}

/// What the client is doing.
#[derive(Debug)]
enum State {
    /// Made: the handshake not asked for yet.
    Fresh(Fresh),
    /// The handshake asked for, and the connection after it.
    Open(Open),
    /// The connection failed, and the side above was told: waiting for its
    /// close, with the demand outstanding below, if any, which the close
    /// withdraws.
    Failed(Option<Demand>),
    /// Closed by the side above: what TLS owes, `close_notify` last, goes
    /// below, then `Closed`.
    Closing(Closing),
    /// Closed: terminal, and the placeholder of every transition.
    Closed,
}

/// What a client is made with, until its handshake, and the stream below as
/// it knows it meanwhile.
#[derive(Debug)]
struct Fresh {
    config: Config,
    name: Name,
    line: Line,
}

/// The stream below, before the handshake demanded anything of it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Line {
    Open,
    /// It ended: the handshake can never be answered.
    Ended,
    Failed(Fault),
}

/// A connection, from its handshake on.
#[derive(Debug)]
struct Open {
    tls: Connection,
    handshake: Handshake,
    /// What the server's records came to.
    peer: Peer,
    /// The stream written: `close_notify`, then the stream below finished.
    writing: Writing,
    below: Below,
    above: Above,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Handshake {
    Running,
    /// Done, and `Ready` went up.
    Done,
}

/// How the server's side of the connection stands.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Peer {
    Open,
    /// Its `close_notify` was read: what the intake holds, then `End`.
    Closed,
    /// The stream below ended without it: what the intake holds, then the
    /// connection fails, `Truncated`.
    Cut,
}

/// How the side above's writing stands.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Writing {
    Open,
    /// The side above finished: `close_notify` is owed once the handshake is
    /// done.
    Finishing,
    /// `close_notify` is written, and owed below with the rest.
    Flushing,
    /// It went, and the stream below was finished.
    Finished,
}

/// `Handshake`: rustls's connection, with its `ClientHello` owed.
fn start(
    fresh: Fresh,
    buffers: &mut Buffers,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) -> State {
    let connected = connect(fresh, env.wall);
    let tls = match connected {
        Ok(tls) => tls,
        Err(error) => {
            above.push(Event::Stream(Up::Failed(error.fault())));
            above.push(Event::Failed(error));
            return State::Failed(None);
        }
    };
    let open = Open {
        tls,
        handshake: Handshake::Running,
        peer: Peer::Open,
        writing: Writing::Open,
        below: Below { demand: None, granted: 0, record: Record::Header, ended: false },
        above: Above { demand: None, grant: None, reading: Reading::Open },
    };
    processed(open, buffers, above, below)
}

/// An event below before the handshake: only the stream's end or failure,
/// as nothing is demanded. A failure after the end says only that the
/// stream cannot send (lib.md, 7), which fails the handshake all the same.
fn fresh_up(mut fresh: Fresh, ev: Up) -> Fresh {
    fresh.line = match ev {
        Up::End => match fresh.line {
            Line::Open => Line::Ended,
            Line::Ended | Line::Failed(_) => fresh.line,
        },
        Up::Failed(fault) => match fresh.line {
            Line::Open | Line::Ended => Line::Failed(fault),
            Line::Failed(_) => fresh.line,
        },
        Up::Bytes(_) | Up::Room => unreachable!("an answer before the handshake demanded anything"),
    };
    fresh
}

/// rustls handed what the records hold; then the handshake's end, if this
/// was it, and where the connection goes.
fn processed(mut open: Open, buffers: &mut Buffers, above: &mut Queue<Event>, below: &mut Queue<Down>) -> State {
    match session::process(&mut open.tls, &mut buffers.records, &mut buffers.intake, &mut buffers.owed) {
        Ok(true) => open.peer = Peer::Closed,
        Ok(false) => {}
        Err(error) => return fail(open, error, above),
    }
    if let Some(agreed) = ready(&open.tls, &mut open.handshake) {
        above.push(Event::Ready(agreed));
    }
    settle(open, buffers, above, below)
}

/// Where an open connection goes after every transition, in one place
/// (programming-model.md, 5.4): `close_notify` written once it is owed; the
/// side above's demand met from the intake, or the end or truncation told;
/// what TLS owes sent within room it holds; the stream below finished once
/// `close_notify` went; and the next demand below, if none is outstanding
/// and the state wants one.
fn settle(mut open: Open, buffers: &mut Buffers, above: &mut Queue<Event>, below: &mut Queue<Down>) -> State {
    if open.writing == Writing::Finishing && open.handshake == Handshake::Done {
        if let Err(error) = session::close_notify(&mut open.tls, &mut buffers.records, &mut buffers.owed) {
            return fail(open, error, above);
        }
        open.writing = Writing::Flushing;
    }
    if open.handshake == Handshake::Done {
        let read = match open.above.demand {
            Some(wanted) => wanted.read,
            None => skein_lib::stream::Read::Nothing,
        };
        match answer(read, &mut open.above.reading, open.peer, &mut buffers.intake) {
            Answer::Wait => {}
            Answer::Bytes(bytes) => {
                open.above.demand = None;
                above.push(Event::Stream(Up::Bytes(bytes)));
            }
            Answer::End => above.push(Event::Stream(Up::End)),
            Answer::Truncated => return fail(open, Error::Truncated, above),
        }
    }
    // Room the side above gave up, or held when it withdrew: what TLS owes
    // goes within it. While the side above holds it, what TLS owes goes
    // with the side above's `Send`.
    if !buffers.owed.is_empty() && open.below.granted > 0 && open.above.grant.is_none() {
        let room = mem::replace(&mut open.below.granted, 0);
        below.push(Down::Send(buffers.owed.take(room)));
    }
    if open.writing == Writing::Flushing && buffers.owed.is_empty() {
        below.push(Down::Finish);
        open.writing = Writing::Finished;
    }
    if open.below.demand.is_none() {
        match wanted(&open, buffers) {
            Ok(Some(demand)) => {
                below.push(Down::Demand { read: demand.read, room: demand.room.bytes() });
                open.below.demand = Some(demand);
            }
            Ok(None) => {}
            Err(error) => return fail(open, error, above),
        }
    }
    State::Open(open)
}
