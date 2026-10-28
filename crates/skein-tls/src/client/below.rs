//! The side below and its records (tls.md, 3.2).
use super::close::fail;
use super::{
    Buffers, Error, Event, HEADER, Handshake, MAX_PLAINTEXT, Open, Peer, Reading, State, Wanted, Writing, processed,
    room_for, settle,
};
use skein_lib::Queue;
use skein_lib::stream::{Down, Read, Up};
/// The stream below, as the client knows it.
#[derive(Debug)]
pub(super) struct Below {
    /// The demand outstanding, if any: stated only when none is, and
    /// answered at most once (lib.md, 7).
    pub(super) demand: Option<Demand>,
    /// Room granted for the side above, which it has not sent within yet;
    /// or, once it withdrew or finished, which TLS may send within.
    pub(super) granted: u32,
    /// The part of the next record to read.
    pub(super) record: Record,
    /// The stream ended: it reads nothing more.
    pub(super) ended: bool,
}

/// A demand stated below: what it reads, and what its room is for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) struct Demand {
    pub(super) read: Read,
    pub(super) room: Room,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Room {
    None,
    /// For TLS's own output: this much of what it owes.
    Owed(u32),
    /// For the side above's demand of `plaintext` bytes of room: what they
    /// take in records.
    Above {
        ciphertext: u32,
        plaintext: u32,
    },
}

impl Room {
    pub(super) fn bytes(self) -> u32 {
        match self {
            Room::None => 0,
            Room::Owed(bytes) | Room::Above { ciphertext: bytes, .. } => bytes,
        }
    }
}

/// The part of a record read next.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Record {
    Header,
    /// Its body, of this many bytes, at least one.
    Body(u32),
}

/// An event below while open.
pub(super) fn open_up(
    mut open: Open,
    buffers: &mut Buffers,
    ev: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) -> State {
    match ev {
        Up::Bytes(bytes) => {
            let demand = open.below.demand.take().expect("bytes answer a demand");
            assert!(answers(demand.read, &bytes), "bytes answer a fill, exactly");
            open.below.record = received(open.below.record, &bytes, buffers);
            // Copied into the records, it is dropped before rustls deciphers
            // them and copies the plaintext out.
            drop(bytes);
            // rustls judges a header as it comes: its type, its version, and
            // a length within what TLS allows.
            processed(open, buffers, above, below)
        }
        Up::Room => {
            let demand = open.below.demand.take().expect("room answers a demand");
            match demand.room {
                Room::Owed(room) => below.push(Down::Send(buffers.owed.take(room))),
                Room::Above { ciphertext, plaintext } => {
                    open.below.granted = ciphertext;
                    // Unless the side above withdrew meanwhile: the room is
                    // then TLS's, for close_notify.
                    if let Some(wanted) = open.above.demand.take() {
                        assert!(wanted.room == plaintext, "room answers the side above's demand");
                        open.above.grant = Some(plaintext);
                        above.push(Event::Stream(Up::Room));
                    }
                }
                Room::None => unreachable!("room answers a demand for room"),
            }
            settle(open, buffers, above, below)
        }
        Up::End => {
            open.below.ended = true;
            open.below.demand = ended(open.below.demand, below);
            match open.handshake {
                Handshake::Running => return fail(open, Error::Truncated, above),
                Handshake::Done => {}
            }
            if open.peer == Peer::Open {
                open.peer = Peer::Cut;
            }
            settle(open, buffers, above, below)
        }
        // Nothing follows a failure: what was outstanding is over.
        Up::Failed(fault) => {
            open.below.demand = None;
            fail(open, Error::Stream(fault), above)
        }
    }
}

/// What the client demands below, when nothing is outstanding: the next
/// piece of a record, while the handshake runs, or for the side above's
/// read the intake does not meet; with room for what TLS owes, or else for
/// what the side above demands, once the handshake is done. No room while
/// room granted is not sent within, nor once the stream below is finished.
pub(super) fn wanted(open: &Open, buffers: &Buffers) -> Result<Option<Demand>, Error> {
    let reads = !open.below.ended
        && open.peer == Peer::Open
        && match open.handshake {
            Handshake::Running => true,
            Handshake::Done => {
                open.above.reading == Reading::Open
                    && match open.above.demand {
                        Some(wanted) => wanted.read != Read::Nothing,
                        None => false,
                    }
            }
        };
    let read = if reads { Read::Fill(next_fill(open.below.record, &buffers.records)?) } else { Read::Nothing };
    let room = if open.below.granted > 0 || open.above.grant.is_some() || open.writing == Writing::Finished {
        Room::None
    } else if !buffers.owed.is_empty() {
        Room::Owed(buffers.owed.len())
    } else if open.handshake == Handshake::Done && open.writing == Writing::Open {
        match open.above.demand {
            Some(Wanted { room: 0, .. }) | None => Room::None,
            Some(Wanted { room, .. }) => {
                Room::Above { ciphertext: room_for(room).expect("checked by valid"), plaintext: room }
            }
        }
    } else {
        Room::None
    };
    if read == Read::Nothing && room == Room::None {
        return Ok(None);
    }
    Ok(Some(Demand { read, room }))
}

/// The demand outstanding below once the stream ended. A read outstanding
/// crosses the end and is never met, but stays outstanding, as io keeps it
/// (lib.md, 7; io.md, 3.3): TLS reads no more after the end, so it
/// withdraws a demand that only reads, which leaves it free to demand room.
/// One with room keeps it, as room may still be granted.
pub(super) fn ended(demand: Option<Demand>, below: &mut Queue<Down>) -> Option<Demand> {
    match demand {
        Some(Demand { room: Room::None, .. }) => {
            below.push(Down::Demand { read: Read::Nothing, room: 0 });
            None
        }
        Some(Demand { room, .. }) => Some(Demand { read: Read::Nothing, room }),
        None => None,
    }
}

/// An event below after the connection failed: an answer to the demand
/// outstanding, which ends it, or the stream's end or failure. A read that
/// crosses the end stays outstanding, for the close to withdraw.
pub(super) fn failed_up(demand: Option<Demand>, ev: Up) -> Option<Demand> {
    match ev {
        Up::Bytes(_) | Up::Room | Up::Failed(_) => None,
        Up::End => demand,
    }
}

/// The length a record's header gives.
pub(super) fn length(header: &[u8]) -> u32 {
    let [_, _, _, high, low] = *header else { unreachable!("a header is five bytes: a fill of HEADER") };
    u32::from(u16::from_be_bytes([high, low]))
}

/// The fill `bytes` answer.
pub(super) fn fill(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).expect("a delivery within LARGEST_READ")
}

/// A piece of a record read: kept for rustls; the part to read next.
pub(in crate::client) fn received(record: Record, bytes: &[u8], buffers: &mut Buffers) -> Record {
    buffers.records.append(bytes).expect("a piece of a record is read only when the records have room");
    match record {
        Record::Header => match length(bytes) {
            0 => Record::Header,
            length => Record::Body(length),
        },
        Record::Body(length) => {
            assert!(buffers.intake.room() >= length.min(MAX_PLAINTEXT), "a record is read with room for its plaintext");
            Record::Header
        }
    }
}

/// The fill of the next piece, or `TooLong`.
pub(in crate::client) fn next_fill(record: Record, records: &crate::held::Held) -> Result<u32, Error> {
    let piece = match record {
        Record::Header => HEADER,
        Record::Body(length) => length,
    };
    if records.room() < piece {
        return Err(Error::TooLong);
    }
    Ok(piece)
}

/// Whether `bytes` answer `read`, a fill of their length.
pub(in crate::client) fn answers(read: Read, bytes: &[u8]) -> bool {
    read == Read::Fill(fill(bytes))
}
