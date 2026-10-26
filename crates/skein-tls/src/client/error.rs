//! Ends and failures (tls.md, 3.3).
#[cfg(doc)]
use super::{FLIGHT, Limits};
use skein_lib::stream::Fault;
/// Why a connection failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The server's certificate was refused.
    Certificate(Certificate),
    /// A record failed to decrypt: the peer is broken or hostile, or its
    /// bytes were changed on the way.
    Decrypt,
    /// The peer sent a fatal alert, of this description (RFC 8446, 6): it
    /// gave up on the connection.
    Alert(u8),
    /// The peer broke TLS: a message malformed or out of place, a version,
    /// suite or protocol it was not offered.
    Protocol,
    /// A handshake message longer than [`Limits::records`] holds.
    TooLong,
    /// What TLS would owe the stream below is longer than [`FLIGHT`]: a
    /// `HelloRetryRequest` sent a cookie too long to echo in the second
    /// `ClientHello`.
    FlightTooLong,
    /// The stream below ended before the server's `close_notify`: during the
    /// handshake, or after it, a truncation (RFC 8446, 6.1).
    Truncated,
    /// The stream below failed.
    Stream(Fault),
    /// rustls failed for a reason of its own: no entropy from the kernel,
    /// or its keys exhausted; on the native face, also when its tokens below
    /// run out. Also refuses the portable target, where a world runs in plaintext
    /// (testing.md, 10).
    Other,
}

impl Error {
    /// What the plaintext stream is told when the connection fails with
    /// this (lib.md, 7): the stream's own fault; a reset for the peer's
    /// alert, by which it gave up; invalid for anything wrong with the
    /// peer's data, a truncation among it, which a clean end would hide.
    #[must_use]
    pub fn fault(self) -> Fault {
        match self {
            Error::Certificate(_)
            | Error::Decrypt
            | Error::Protocol
            | Error::TooLong
            | Error::FlightTooLong
            | Error::Truncated => Fault::Invalid,
            Error::Alert(_) => Fault::Reset,
            Error::Stream(fault) => fault,
            Error::Other => Fault::Other,
        }
    }
}

/// Why the server's certificate was refused, at the wall time of the step
/// that started the handshake, for the server's name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Certificate {
    /// It expired.
    Expired,
    /// It is not valid yet.
    NotYetValid,
    /// It is not valid for the server's name.
    Name,
    /// It does not chain to a root the client trusts.
    Issuer,
    /// Anything else: a bad signature or encoding, a usage it does not
    /// allow.
    Invalid,
}
