//! Neutral forge calls and answers (forge.md, section 3).
//!
//! These types describe bounded requests and answers; they retain no state.
//! They never name tasks or authorization policy. The adapter maps a `Read`
//! or `Write` to provider calls and returns one typed terminal. A job-log
//! read names one run, job, attempt, and head, and returns at most its byte cap.
use alloc::boxed::Box;
use skein_lib::{Duration, Time, Wall};

/// The largest printable read validator retained from a forge.
pub const VALIDATOR_BYTES: usize = 128;
/// The display reason's byte bound; decoders truncate at a UTF-8 boundary instead of refusing the read.
pub const DEQUEUED_BYTES: usize = 256;

/// A forge's request window, so the client can use its allowance and reset time.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rate {
    /// The requests the window allows.
    pub limit: u32,
    /// The requests left in the window.
    pub remaining: u32,
    /// When the window resets, as an epoch wall time.
    pub reset_at: Wall,
}
/// A bounded printable read validator, retained for conditional requests.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Validator(Box<[u8]>);
impl Validator {
    /// Checks the non-empty printable ASCII bound before copying.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > VALIDATOR_BYTES {
            return None;
        }
        for byte in bytes {
            if !(0x20..=0x7e).contains(byte) {
                return None;
            }
        }
        Some(Self(skein_lib::bytes::copy_of(bytes)))
    }
    /// The validator's HTTP spelling, for a conditional request.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A forge commit identifier in the adapter’s fixed-width form.
pub type Commit = [u8; 32];
/// A repository named within one configured forge.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Repository {
    pub forge: u16,
    pub repository: u32,
}
/// A provider hint that may advance a live-resource read.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Hint {
    pub repository: Repository,
    pub change: Change,
    pub key: Option<Box<[u8]>>,
}
/// The resource named by a provider hint.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    /// A hint about an item.
    Item(u64),
    /// A hint about a branch.
    Branch(Box<[u8]>),
    /// An authenticated push webhook. The hint advances a fresh branch read;
    /// its actor is used only if that read still observes this head.
    BranchMoved { branch: Box<[u8]>, head: Commit, actor: u64 },
    /// A hint about a commit.
    Commit(Commit),
}
/// A typed call the protocol adapter makes to the forge.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    /// A read operation.
    Read(Read),
    /// A write operation.
    Write(Write),
}
/// A typed forge read, bounded by the client’s limits.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// Locate the forge's id by owner and name, addressed to repository 0.
    Locate { owner: Box<[u8]>, name: Box<[u8]> },
    /// Inclusive forge time, least recently updated first, pages from 1.
    Items {
        /// Inclusive at the provider's time resolution. Saved positions come
        /// from observed forge timestamps; arbitrary caller times may overlap
        /// the preceding coarse quantum. Page validation conservatively
        /// tolerates that overlap rather than claiming adapter normalization.
        since: Time,
        page: u32,
        kind: Option<Kind>,
    },
    /// Read item.
    Item { number: u64, after: u64 },
    /// Read pull.
    Pull { number: u64 },
    /// The open matching pull, else the oldest.
    PullFor { head: Box<[u8]>, base: Box<[u8]> },
    /// Read reviews.
    Reviews { number: u64, page: u32 },
    /// Read statuses.
    Statuses { commit: Commit, page: u32 },
    /// Read remarks.
    Remarks { number: u64, review: u64, page: u32 },
    /// Read branch.
    Branch { branch: Box<[u8]> },
    /// Names starting with the nonempty branch prefix, bounded like a branch name.
    /// Only matching names count toward the adapter's answer limit.
    Branches { prefix: Box<[u8]> },
    /// Read pull files.
    PullFiles { number: u64, head: Commit, page: u32 },
    /// Whole bounded comparison; `TooLarge` means unknown overlap.
    Compare { before: Commit, after: Commit },
    /// Provider descriptions and links, with a failing job's attempt.
    Checks { commit: Commit },
    /// Bounded plaintext log for one failed job attempt at its head.
    Job { attempt: JobAttempt, max_bytes: u32 },
    /// One file at an immutable head, cut to the requested byte budget.
    File { head: Commit, path: Box<[u8]>, max_bytes: u32 },
    /// Requires admin, including an absent rule.
    Protection { branch: Box<[u8]> },
    /// Read settings.
    Settings,
    /// Read collaborators.
    Collaborators { page: u32 },
    /// Read permission.
    Permission { user: u64 },
}
/// A typed forge write handed down from a committed outbox entry.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    /// Write create issue.
    CreateIssue { key: Box<[u8]>, title: Box<[u8]>, body: Box<[u8]> },
    /// Write open pull.
    OpenPull { title: Box<[u8]>, body: Box<[u8]>, head: Box<[u8]>, base: Box<[u8]> },
    /// Write post.
    Post { number: u64, key: Box<[u8]>, body: Box<[u8]> },
    /// Write review.
    Review { number: u64, key: Box<[u8]>, verdict: Verdict, body: Box<[u8]> },
    /// Write edit.
    Edit { number: u64, title: Option<Box<[u8]>>, body: Option<Box<[u8]>> },
    /// Write set reviewers.
    SetReviewers { number: u64, reviewers: Box<[u64]> },
    /// Write close.
    Close { number: u64 },
    /// Write reopen.
    Reopen { number: u64 },
    /// Checked at exact head. The outbox effect's intended base is a
    /// client-side precondition, checked by reading the pull afresh.
    Merge { number: u64, head: Commit },
    /// Forge-side merge update, with two parents and fresh CI.
    Update { number: u64 },
    /// Write status.
    Status { commit: Commit, context: Box<[u8]>, check: Check },
    /// Write create branch.
    CreateBranch { branch: Box<[u8]>, commit: Commit },
    /// Write delete branch.
    DeleteBranch { branch: Box<[u8]> },
}
/// A decoded terminal value for one forge call.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    /// The located repository's nonzero id on its forge.
    Located { repository: u32 },
    /// Return items.
    Items { items: Box<[Summary]>, more: bool, now: Time },
    /// Return item.
    Item { item: Summary, comments: Box<[Comment]>, more: bool },
    /// Return pull.
    Pull(Pull),
    /// Return reviews.
    Reviews { reviews: Box<[Review]>, more: bool },
    /// Return statuses.
    Statuses { ci: Ci, statuses: Box<[Status]>, more: bool },
    /// Return remarks.
    Remarks { remarks: Box<[Remark]>, more: bool },
    /// Return commit.
    Commit(Commit),
    /// Return branch names for adoption's deployment-prefix check.
    Branches(Box<[Box<[u8]>]>),
    /// The observed head must equal the request's head.
    PullFiles { head: Commit, files: Box<[File]>, more: bool },
    /// Return compare.
    Compare { before: Commit, after: Commit, contains_before: bool, files: Box<[File]>, commits: Box<[Commit]> },
    /// Return checks.
    Checks(Box<[Status]>),
    /// Bytes from one pinned job attempt, cut to the requested byte budget.
    Job { attempt: JobAttempt, step: Option<Step>, log: Box<[u8]>, truncated: bool },
    /// One bounded file from the requested immutable head.
    File { head: Commit, path: Box<[u8]>, bytes: Box<[u8]>, truncated: bool },
    /// Return protection.
    Protection(Option<Protection>),
    /// Return settings.
    Settings(Settings),
    /// Return collaborators.
    Collaborators { collaborators: Box<[Collaborator]>, more: bool },
    /// Return permission.
    Permission(Permission),
    /// Return created.
    Created(u64),
    /// Return commented.
    Commented(u64),
    /// Return reviewed.
    Reviewed(u64),
    /// Return merged.
    Merged(Commit),
    /// The pull request was put in the merge queue at the head named by `Write::Merge`, final for its call.
    Queued,
    /// Return branch.
    Branch(BranchCreation),
    /// Return done.
    Done,
}
/// A typed refusal or failure of one forge call.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The forge could not be reached.
    Unavailable,
    /// May have reached the forge: a write is uncertain.
    Timeout,
    /// The forge refused calls until its reset.
    RateLimited { after: Duration },
    /// The writer lacks permission.
    Forbidden,
    /// The requested object is absent.
    Missing,
    /// The requested job attempt is absent.
    MissingJob,
    /// The request or answer exceeds a configured bound.
    TooLarge,
    /// The requested operation has no content, including an empty branch prefix.
    Empty,
    /// The provider has no room for this operation.
    Full,
    /// The object already exists.
    Exists,
    /// The pull request has nothing to merge.
    NothingToMerge,
    /// The target is closed.
    Closed,
    /// The required head changed.
    Stale,
    /// The change conflicts with its base.
    Conflict,
    /// Protection refused the write.
    Protected,
    /// Repository settings or transport policy refuses this call.
    Refused,
    /// A decoded answer does not match its operation.
    InvalidAnswer,
    /// Client admission refused, no API call was made.
    Busy,
    /// The forge is not connected, or stopped; no API call was made.
    NotConnected,
    /// No accepted App installation for the repository owner account.
    NotInstalled,
}
/// The kind of an item in the forge listing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    /// An issue item.
    Issue,
    /// A pull request item.
    Pull,
}
/// Whether a forge item is open or closed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum State {
    /// The item remains open.
    Open,
    /// The item is closed.
    Closed,
}
/// The combined CI state at a commit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ci {
    /// No CI contexts were reported.
    None,
    /// At least one required context is pending.
    Pending,
    /// The reported contexts passed.
    Passed,
    /// At least one reported context failed.
    Failed,
}
/// The state of one check context.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Check {
    /// The context has not reached a verdict.
    Pending,
    /// The context passed.
    Passed,
    /// The context failed.
    Failed,
}
/// A review verdict posted to a pull request.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    /// The review approves the change.
    Approve,
    /// The review requests changes.
    RequestChanges,
    /// The review leaves nonblocking remarks.
    Comment,
}
/// The authenticated writer’s repository permission.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Permission {
    /// The writer has no repository access.
    None,
    /// The writer may read the repository.
    Read,
    /// The writer may write the repository.
    Write,
    /// The writer may administer the repository.
    Admin,
}
/// Whether branch creation made a branch or found one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BranchCreation {
    /// The requested branch was created.
    Created,
    /// The requested branch already existed.
    Exists,
}
/// A bounded item row from a changed-items listing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Summary {
    pub labels: Box<[Box<[u8]>]>,
    pub number: u64,
    pub kind: Kind,
    pub state: State,
    pub author: u64,
    pub key: Option<Box<[u8]>>,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub updated: Time,
}
/// Evidence about the observable body/version, not merely its creator.
/// An adapter must use Unknown when its provider shape cannot establish
/// whether a historical creation was edited. Original is never a default
/// inferred from a missing editor field.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Provenance {
    /// The observed body is its original version.
    Original,
    /// The observed body was revised.
    Revised,
    /// The provider cannot establish its origin.
    Unknown,
}
/// A comment with its identity and observable version.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub provenance: Provenance,
    pub author: u64,
    pub created: Time,
    pub revision: u64,
    pub key: Option<Box<[u8]>>,
    pub body: Box<[u8]>,
}
/// A pull request and the facts needed by a change decision.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub reviewers: Box<[u64]>,
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: Commit,
    pub base_commit: Option<Commit>,
    pub merged: Option<Commit>,
    pub mergeable: bool,
    pub ci: Ci,
    /// The position in a merge queue when the pull request is in one.
    pub queued: Option<u32>,
    /// Why a queue last ejected it; decoders truncate to `DEQUEUED_BYTES` at a UTF-8 boundary for display.
    pub dequeued: Option<Box<[u8]>>,
}
/// A review with its identity, verdict and observed version.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub id: u64,
    pub provenance: Provenance,
    /// Changes when the observable review version changes.
    pub revision: u64,
    pub author: u64,
    pub verdict: Verdict,
    pub commit: Commit,
    pub key: Option<Box<[u8]>>,
    pub body: Box<[u8]>,
    pub at: Time,
    pub official: bool,
}
/// A comment attached to a review and a file position.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Remark {
    pub id: u64,
    pub author: u64,
    pub path: Box<[u8]>,
    pub line: u32,
    pub body: Box<[u8]>,
}
/// A check context with a description, link and optional job attempt.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Status {
    pub at: Time,
    pub context: Box<[u8]>,
    pub check: Check,
    pub description: Box<[u8]>,
    pub url: Box<[u8]>,
    pub job: Option<JobAttempt>,
}
/// A Forgejo Actions job attempt whose status belongs to `head`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct JobAttempt {
    pub head: Commit,
    pub run: u64,
    pub job: u64,
    pub attempt: u32,
}
/// The first failed step of a job attempt (forge.md, 3.6).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Step {
    pub number: u32,
    pub name: Box<[u8]>,
}
/// A changed file with content on either side when present.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    /// None means absent, preserving additions and deletions.
    pub before: Option<Box<[u8]>>,
    pub after: Option<Box<[u8]>>,
}
/// The App's own ability to bypass a ruleset (forge-github.md, 5.3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Bypass {
    Never,
    PullsOnly,
    Always,
}
/// A branch protection rule as the writer could observe it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "protection reports independent provider rules")]
pub struct Protection {
    pub queue: bool,
    pub signatures: bool,
    pub linear: bool,
    pub code_owners: bool,
    pub bypass: Bypass,
    pub branch: Box<[u8]>,
    pub contexts: Box<[Box<[u8]>]>,
    pub approvals: u32,
    pub dismiss_stale: bool,
}
/// Repository merge settings and its default branch.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Settings {
    pub default_branch: Box<[u8]>,
    pub merge: bool,
    pub squash: bool,
    pub rebase: bool,
}
/// A repository collaborator and permission.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Collaborator {
    pub user: u64,
    pub permission: Permission,
}
