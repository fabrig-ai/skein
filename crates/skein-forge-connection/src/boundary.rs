//! Forge component boundary (forge-connection.md, sections 2, 3.5, 7 and 8).
use crate::Access;
use alloc::boxed::Box;
use skein_forge::api;
use skein_lib::{Secret, Token, Wall};

/// An installation account at the connection boundary (forge-connection.md, 2.1).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Account {
    pub id: u64,
    pub login: Box<[u8]>,
}

/// An installation lifecycle change (forge-connection.md, 4.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    Created,
    Deleted,
    Suspended,
    Unsuspended,
    Renamed,
    /// A retained installation's repository selection changed (4.4).
    Repositories,
}
/// A diagnostic for the owner's log (forge-connection.md, 4.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Fact {
    /// An unconfigured job log origin, once per remembered origin per connect.
    Origin {
        origin: skein_forge::github::redirect::Origin,
    },
    Unkept {
        installation: u64,
    },
    /// The delivery log ended at its age or page bound before the kept guid.
    Gap,
}

/// One call's terminal, the forge client's `Event::Answered` field for field. The
/// component makes it; the routing hands it to the root as `Event::Forge`, and hands
/// back one the root did not take (`Request::Refused`, `Request::Returned`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Answered {
    /// The forge client's call token, echoed.
    pub call: Token,
    /// The requests the call wrote, cache fills included; 0 for a call that wrote none.
    pub cost: u32,
    /// How many of the call's `cost` requests the forge answered `304`;
    /// its rate does not count them. Zero until a call's read takes a validator.
    pub free: u32,
    /// The latest window reported by an answer of the call, per installation on
    /// GitHub. None when no answer reported one (forge-connection.md, 4.1).
    pub rate: Option<api::Rate>,
    pub result: Result<api::Answer, api::Error>,
}

/// From the routing.
#[derive(Debug)]
pub enum Request {
    /// Hands or replaces a forge's access (forge-connection.md, 3.5): at most one
    /// Connected and at most one Stopped per generation, including refusal after Ready.
    /// A replacement suppresses the old generation's remaining events.
    /// A forge not configured, or a Request after Close, is ignored.
    Connect { forge: u16, access: Access },
    /// Replaces the bounded set of accepted installations (forge-connection.md, 4.4).
    Installations { forge: u16, accepted: Box<[u64]> },
    /// The root's `Request::Forge`: ended by exactly one `Event::Answered`.
    Call { call: Token, repository: api::Repository, op: api::Op },
    /// An answer the root's admission did not take (`try_step`'s `Err`): held, offered
    /// again at the next `Offer`; at most one per call.
    Refused(Answered),
    /// A verified hint refused by root admission, held until the next offer.
    RefusedHint(crate::hooks::Hinted),
    /// An answer the root returned undecided in its drain (`Request::Busy`): kept until
    /// `Close`, never offered again (forge-connection.md, 2).
    Returned(Answered),
    /// The root admits again: held answers awaiting an offer go up again.
    Offer,
    /// After the root's drain: calls in flight end with their connections and no answer
    /// goes up; `Event::Closed` answers it.
    Close,
}

/// To the routing; io's requests go in a queue of their own.
#[derive(Debug)]
pub enum Event {
    /// A call's one terminal.
    Answered(Answered),
    /// A retained installation's lifecycle, with its current or last known account.
    Installation { forge: u16, installation: u64, account: Option<Account>, change: Change },
    /// A bounded diagnostic for the owner's log.
    Fact { forge: u16, fact: Fact },
    /// Git's Contents token, handed to the owner once (forge-connection.md, 5.3).
    GitToken { forge: u16, installation: u64, account: Account, token: Secret, expires: Wall },
    /// A verified webhook after its response, or a delivery read from the log.
    Hint(crate::hooks::Hinted),
    /// The webhooks listener bound, with its actual address, once per bind.
    Listening { addr: skein_io::Addr },
    /// The webhooks listener failed to bind or was lost.
    ListenerLost,
    /// A forge's start is done: at most once per configured-access or connect generation.
    Connected { forge: u16 },
    /// A forge's access refused: at most once per generation, possibly after Connected.
    /// The forge holds no access until the next connect.
    Stopped { forge: u16, why: Stopped },
    /// The terminal of `Close`: every connection closed.
    Closed,
}

/// Why a forge's access was refused (forge-connection.md, 3.5 and 8.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stopped {
    /// A connect's access refused before any io, naming the value.
    Invalid { value: Value },
    /// The App JWT was refused.
    KeyRefused,
    /// An installation mint was refused permanently.
    TokenRefused { installation: u64, status: u16 },
    /// The identity read answered with another user's id.
    OtherUser { user: u64 },
    /// A `4xx` other than `429` to one of the start's reads.
    Refused { read: StartRead, status: u16 },
}

/// The start's reads.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StartRead {
    /// `GET /api/v1/user`.
    User,
    /// `GET /api/v1/settings/api`.
    ApiSettings,
    /// GitHub's `GET /app/installations`.
    Installations,
    /// GitHub's installation token mint.
    Mint,
    /// GitHub's GraphQL viewer read.
    Viewer,
}

/// The value of an access a connect's check refused (forge-connection.md, 3.5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Value {
    /// A credential of the other dialect's kind: an App's on Forgejo, a token on GitHub.
    Credential,
    /// A token empty, past `token_bytes`, or with a byte a request head refuses.
    Token,
    /// An App's key past `key_bytes`, or one that will not sign (a GitHub forge's, 5.2).
    Key,
    /// A user of 0, or another than the access the forge holds.
    User,
    /// A webhook secret on a GitHub forge, with no listener, empty, or past `hook_secret_bytes`.
    Secret,
}
