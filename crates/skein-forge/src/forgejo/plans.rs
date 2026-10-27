//! Pure call plans and their exchange events.
use crate::forgejo::{decode, document, header};
use alloc::boxed::Box;
use skein_lib::{Duration, Wall};

/// The answer's fields the plans read beside its body.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Fields {
    /// `Date`: the time the answer was made.
    pub date: Option<Wall>,
    /// `Retry-After` of a `429`, from `Date` when it is a date.
    pub retry_after: Option<Duration>,
    /// `X-HasMore`.
    pub has_more: Option<bool>,
    /// `Content-Range` of a `206`.
    pub content_range: Option<header::ContentRange>,
    /// `Connection: close`: Closed after this answer, not Idle.
    pub close: bool,
}

/// What the body was.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Body {
    /// A JSON answer, whole; its rows went up before it.
    Document(document::Document),
    /// A raw answer within its cap; `more` when bytes followed a head or preceded a tail.
    Raw { bytes: Box<[u8]>, more: bool },
    /// No body, or one not read (a failure's status needs no message).
    Empty,
    /// Not the document its request gets: the connection closes.
    Malformed(decode::Malformed),
}

/// Why an exchange failed (forge.md, 3.4,
/// "Unavailable is provable only before the first byte").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Failure {
    /// The server never saw the request: no byte written (a refused connect, a failed
    /// handshake, the deadline while waiting), or skein-http's `Closed`: `Unavailable`.
    Unseen,
    /// It may have been read: a reset, the stream's end, `Truncated`, `Stream`, the
    /// deadline in flight, an answer HTTP refuses: `Timeout`.
    Seen,
}

/// What a connection tells the call it serves, in order: `Head`, `Row`s, `Done`; or `Failed`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Heard {
    Head { status: u16, fields: Fields },
    Row(document::Row),
    Done(Body),
    Failed(Failure),
}

pub mod actions;
pub mod contents;
pub mod failure;
pub mod hint;
pub mod plan;
pub mod reading;
pub mod reads;
pub mod start;
pub mod tables;
pub mod writes;

#[cfg(test)]
mod tests;

mod turn;

mod commits;
