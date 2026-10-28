//! The owner's plans, answers and retained items (forge-setup.md, 6.3).
use crate::{
    github::manifest::Target,
    item::Item,
    reconcile::{Failure, Outcome},
};
use alloc::boxed::Box;
use core::num::{NonZeroU32, NonZeroU64};
use skein_lib::{Secret, Token};
/// An owner-provided file placed only when absent (forge-setup.md, 4.9).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    pub content: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Plan {
    /// The browser page for creating a GitHub App (5.1).
    Manifest { forge: u16, target: Target },
    /// Converts a manifest callback, retaining its items before Done (5.2).
    Convert { forge: u16, code: Secret, state: Box<[u8]> },
    /// Connects or replaces a pasted App while preserving identity (5.8).
    Paste { forge: u16, app: NonZeroU64 },
    /// The App install link with an Accept-purpose state (5.4 and 5.7).
    Install { forge: u16 },
    /// Verifies and retains an installation (5.4): admitted by the state its
    /// Install drew (5.7), or, with none, by the owner, for its person's own
    /// act under their sign-in.
    Accept { forge: u16, installation: NonZeroU64, state: Option<Box<[u8]>> },
    /// Reconcile either kind; GitHub requires an empty adopted set (5.6).
    Reconcile { forge: u16, adopted: Box<[u64]> },
    /// A connected Forgejo's two tokens, checked (4.6).
    Check { forge: u16 },
    /// The handed admin token, held only until the run ends (4.7).
    TakeOver { forge: u16, admin_token: Secret },
    /// The bot's access to the repository whose id the forge gave (4.5).
    Adopt { forge: u16, repository: NonZeroU64 },
    /// Makes a first repository, its access and missing files (4.9).
    Create { forge: u16, org: Box<[u8]>, repository: Box<[u8]>, files: Box<[File]> },
    /// One picker page, from 1 (4.8). Forgejo takes no installation.
    /// Reads only: the host forwards no Write for it.
    Reachable { forge: u16, installation: Option<u64>, page: NonZeroU32 },
}
impl Plan {
    pub(super) fn forge(&self) -> u16 {
        match self {
            Self::Reconcile { forge, .. }
            | Self::Check { forge }
            | Self::TakeOver { forge, .. }
            | Self::Adopt { forge, .. }
            | Self::Create { forge, .. }
            | Self::Reachable { forge, .. }
            | Self::Manifest { forge, .. }
            | Self::Convert { forge, .. }
            | Self::Paste { forge, .. }
            | Self::Install { forge }
            | Self::Accept { forge, .. } => *forge,
        }
    }
    /// Whether the plan may change the forge or the owner's items.
    pub(super) fn writes(&self) -> bool {
        match self {
            Self::Reconcile { .. }
            | Self::Check { .. }
            | Self::TakeOver { .. }
            | Self::Adopt { .. }
            | Self::Create { .. }
            | Self::Manifest { .. }
            | Self::Convert { .. }
            | Self::Paste { .. }
            | Self::Install { .. }
            | Self::Accept { .. } => true,
            Self::Reachable { .. } => false,
        }
    }
    /// The plan's name, used in a failure the host reports at a GitHub App.
    pub(super) fn name(&self) -> &'static [u8] {
        match self {
            Self::Manifest { .. } => b"manifest",
            Self::Convert { .. } => b"convert",
            Self::Paste { .. } => b"paste",
            Self::Install { .. } => b"install",
            Self::Accept { .. } => b"accept",
            Self::Reconcile { .. } => b"reconcile",
            Self::Check { .. } => b"check",
            Self::TakeOver { .. } => b"take-over",
            Self::Adopt { .. } => b"adopt",
            Self::Create { .. } => b"create",
            Self::Reachable { .. } => b"reachable",
        }
    }
    /// The dialects the plan is for.
    pub(super) fn dialect(&self) -> Dialect {
        match self {
            Self::Manifest { .. }
            | Self::Convert { .. }
            | Self::Paste { .. }
            | Self::Install { .. }
            | Self::Accept { .. } => Dialect::GitHub,
            Self::Reconcile { .. } | Self::Reachable { .. } => Dialect::Both,
            Self::Check { .. } | Self::TakeOver { .. } | Self::Adopt { .. } | Self::Create { .. } => Dialect::Forgejo,
        }
    }
}
pub(super) enum Dialect {
    Forgejo,
    GitHub,
    Both,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Request {
    Run { run: Token, plan: Plan },
    Answer { run: Token, answer: Answer },
    Close,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Answer {
    Read(Option<Secret>),
    Written,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Need {
    Read(Item),
    Write { item: Item, bytes: Secret },
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    Need { run: Token, need: Need },
    Done { run: Token, outcome: Outcome },
    Stop { run: Token, failure: Failure },
    Closed,
}
