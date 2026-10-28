//! Limits and the worst case (tls.md, 3.5).
#[cfg(doc)]
use super::{Client, Error};
use skein_lib::Intake;

/// The most plaintext a record carries (RFC 8446, 5.1; RFC 5246, 6.2.1).
pub const MAX_PLAINTEXT: u32 = 16_384;

/// A record's header: its type, its version and its length.
pub const HEADER: u32 = 5;

/// The longest record body TLS allows, a TLS 1.2 ciphertext's (RFC 5246,
/// 6.2.3); TLS 1.3's is shorter.
pub const MAX_BODY: u32 = MAX_PLAINTEXT + 2_048;

/// A record of the longest body, with its header.
pub const MAX_RECORD: u32 = HEADER + MAX_BODY;

/// The most bytes the client demands below at once: a record's body.
/// Whoever stacks it checks at startup that the stream below's intake holds
/// it (lib.md, 7): a demand past that cap could never be met.
pub const LARGEST_READ: u32 = MAX_BODY;

/// The most of TLS's own output held at once: a flight of the handshake, or
/// an alert and `close_notify`. The longest flight is a `ClientHello` of a
/// server name of 253 bytes and protocols of [`ALPN`](crate::ALPN) bytes,
/// measured at 738 bytes, and 809 once a `HelloRetryRequest` asks for a
/// larger key share (tls.md, 5).
pub const FLIGHT: u32 = 2_048;

/// The most a record adds to the plaintext it carries, of ring's suites:
/// TLS 1.2's AES-GCM, a header, an explicit nonce and a tag (RFC 5288, 3);
/// TLS 1.3's adds 22.
const OVERHEAD: u32 = HEADER + 8 + 16;

/// What may go before the next data rustls encrypts, besides the records
/// of the data: in TLS 1.3, the key update rustls asks for as its keys
/// reach their limit, and a second, as the first's own record reaches it,
/// each a record of 27 bytes (RFC 8446, 4.6.3), while the one that answers
/// the peer's comes alone, as it starts the keys over; in TLS 1.2, a
/// refusal of a renegotiation the client owes, 31 bytes.
const SLACK: u32 = 2 * 27;

/// rustls's own heap besides the server's certificates and the record it
/// works on: its states, keys, transcript and configuration, measured at
/// 10 KB at most, with the longest ALPN list; and what a step that
/// deciphers or encrypts a record of 16 KB holds past that record, as
/// [`LARGEST_READ`] is counted for it, about 5 KB more (tls.md, 5).
const RUSTLS: u64 = 16 * 1_024;

/// The longest handshake message rustls reads: 64 KB of body (its
/// `MAX_HANDSHAKE_SIZE`, 0xffff) and the message's header.
pub(crate) const MAX_HANDSHAKE: u32 = 65_535 + 4;

/// What rustls may hold for each byte of a handshake message as it decodes
/// it, measured (tls.md, 5). A message of the shortest entries a list
/// allows, 3 bytes each in TLS 1.2 and 5 in TLS 1.3 (empty certificates,
/// names of a byte), decodes to an element of 24 or 48 bytes each, in a
/// list that grows by doubling and is copied once; a chain is kept for the
/// connection's life. A server sends such a message whole within the
/// records held, so they bound it: 18 to 20 bytes for each of its bytes
/// were measured past the delivery and rustls's own state.
const HANDSHAKE: u64 = 20;

/// The client's limits (programming-model.md, 7): the same for every step
/// and for [`Client::new`], which allocates by them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// The most the side above demands of the plaintext at once, a fill's
    /// count or a scan's maximum. The intake holds it, and one record's
    /// plaintext beside it. At least 1.
    pub read: u32,
    /// The most room the side above demands at once, in plaintext. At
    /// least 1.
    pub send: u32,
    /// The most ciphertext held at once: a record, and the records of a
    /// handshake message that spans several, the server's certificates
    /// among them. A longer message fails the handshake with
    /// [`Error::TooLong`]. At least [`MAX_RECORD`].
    pub records: u32,
}

/// The room the client demands below for the side above's demand of `send`
/// bytes: their records, each with its overhead, and rustls's slack.
#[must_use]
pub fn room_for(send: u32) -> Option<u32> {
    let records = send.checked_add(MAX_PLAINTEXT - 1)? / MAX_PLAINTEXT;
    send.checked_add(records.checked_mul(OVERHEAD)?)?.checked_add(SLACK)
}

/// The most room the client demands below at once: TLS's own output, or
/// what the side above's largest demand of room takes in records. Whoever
/// stacks the client checks it against the stream below's output cap at
/// startup.
#[must_use]
pub fn largest_room(limits: &Limits) -> u32 {
    match room_for(limits.send) {
        Some(room) => room.max(FLIGHT),
        None => u32::MAX,
    }
}

/// The intake's cap: the side above's largest read, and a record's
/// plaintext beside it, so that a demand the intake does not meet always
/// leaves room for the next record.
pub(super) fn intake_cap(limits: &Limits) -> Option<u32> {
    limits.read.checked_add(MAX_PLAINTEXT)
}

/// Whether these limits are representable: every edge bound they size
/// is finite and every capacity they derive fits (programming-model.md, 6.3).
/// Plaintext intake and ciphertext room fit u32, and one record can be held.
#[must_use]
pub fn valid(limits: &Limits) -> bool {
    limits.read > 0
        && limits.send > 0
        && limits.records >= MAX_RECORD
        && room_for(limits.send).is_some()
        && intake_cap(limits).is_some()
}

/// The most memory a client holds under `limits`, in bytes
/// (programming-model.md, 6.3), rustls's heap included, or `None` if it does
/// not fit a `u64` or the limits cannot be honoured: a read or room of
/// nothing, or records shorter than one.
///
/// Its own: the intake, the records held, the output it owes, all allocated
/// with the client, and the delivery it reads, at most [`LARGEST_READ`]: a
/// delivery is made to the client's demand, so it counts it (testing.md,
/// 5); a step that encrypts holds as much of rustls's instead. What it sends
/// up or down is handed out when it is emitted. rustls allocates as it
/// pleases, so its part is measured against the counting allocator, over
/// handshakes of either version, a retry, the longest ALPN list, the
/// longest chain the records hold, records of every size each way, and a
/// hostile server's messages of the shortest entries (tls.md, 5): its own
/// state (`RUSTLS`), and the decoded form of the longest handshake
/// message, `HANDSHAKE` bytes for each of the fewer of the records held
/// and `MAX_HANDSHAKE`.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !valid(limits) {
        return None;
    }
    let intake = Intake::worst_case(intake_cap(limits)?)?;
    let records = u64::from(limits.records);
    let decoded = u64::from(limits.records.min(MAX_HANDSHAKE)).checked_mul(HANDSHAKE)?;
    intake
        .checked_add(records)?
        .checked_add(u64::from(FLIGHT))?
        .checked_add(u64::from(LARGEST_READ))?
        .checked_add(RUSTLS)?
        .checked_add(decoded)
}
