//! Fields kept from GitHub's first documents (forge-github.md, section 6).
use crate::api;
use alloc::boxed::Box;
use skein_lib::{Secret, Wall};
/// The identity document sent on an installation token (forge-github.md, 4.3).
pub const VIEWER: &[u8] = b"query { viewer { login databaseId } }";
/// Pull identity, landed commit and merge queue facts (forge-github.md, 4.3).
pub const PULL: &[u8] = b"query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { id mergeCommit { oid } mergeQueueEntry { position } timelineItems(last: 1, itemTypes: [REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes { ... on RemovedFromMergeQueueEvent { reason } } } } } }";
/// Expected-head queue admission (forge-github.md, 4.3).
pub const ENQUEUE: &[u8] = b"mutation($id: ID!, $head: GitObjectID!) { enqueuePullRequest(input: {pullRequestId: $id, expectedHeadOid: $head}) { mergeQueueEntry { position } } }";
/// The document a decoder expects.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    ActionsPermissions,
    Workflows,
    RuleRows,
    Ruleset,
    ClassicProtection,
    LogRows,
    LogDelivery,
    /// The REST merge's landed commit.
    Merge,
    /// Queue admission, including every GraphQL error.
    Enqueue,
    Id,
    Number,
    /// A bounded object whose fields are discarded.
    Done,
    Installation,
    InstallationRows,
    Minted,
    ReachableRows,
    Viewer,
    App,
    HookConfig,
    Conversion,
    BotUser,
    Repository,
    Branch,
    BranchRows,
    IssueRows,
    Issue,
    CommentRows,
    /// One pull object.
    Pull,
    /// Pull objects streamed from a listing.
    PullRows,
    /// The shared GraphQL pull query.
    PullGraphql,
    /// Combined status envelope and streamed contexts.
    CombinedStatus,
    /// Check runs streamed from their envelope.
    CheckRuns,
    /// Actions job identity and attempt metadata.
    Job,
    /// A user resolved by numeric id.
    User,
    /// A repository permission.
    Permission,
    /// Streamed repository collaborators.
    CollaboratorRows,
    /// Streamed review comments.
    RemarkRows,
    /// Streamed submitted and pending reviews.
    ReviewRows,
    /// A comparison envelope with streamed commits and filenames.
    Compare,
    FileRows,
    Error,
}
/// A repository's routing, owner and settings row.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "GitHub reports the owner kind and three independent merge settings")]
pub struct Repository {
    pub id: u64,
    pub owner: Box<[u8]>,
    pub owner_id: u64,
    pub organization: bool,
    pub name: Box<[u8]>,
    pub default_branch: Box<[u8]>,
    pub html_url: Box<[u8]>,
    pub merge: bool,
    pub squash: bool,
    pub rebase: bool,
}
/// A named branch and its commit.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Branch {
    pub name: Box<[u8]>,
    pub commit: api::Commit,
}
/// A streamed listing row.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Row {
    Rule(Rule),
    Log(super::log::Row),
    Installation(Installation),
    Reachable(Reachable),
    Branch(Box<[u8]>),
    /// A branch name past `string_bytes`, counted toward the page without retention.
    SkippedBranch,
    Issue(Issue),
    Comment(Comment),
    /// One listed pull.
    Pull(Pull),
    /// One commit status context.
    Context(Context),
    /// One check run.
    Run(Run),
    Collaborator(Collaborator),
    Remark(Remark),
    Review(Review),
    Commit(api::Commit),
    File(File),
}
/// GitHub's message and bounded error messages.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Refusal {
    pub message: Box<[u8]>,
    pub errors: Box<[Box<[u8]>]>,
}
/// A whole document, or the end of a streamed listing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Document {
    ActionsPermissions(bool),
    Workflows {
        total: u64,
        count: u32,
    },
    Ruleset(api::Bypass),
    ClassicProtection(Option<Protection>),
    Delivery(super::log::Logged),
    /// The REST merge's landed commit.
    Merge(api::Commit),
    /// A nonnull queue entry with its checked position.
    Enqueued(u32),
    Id(u64),
    Number(u64),
    Done,
    Installation(Installation),
    Minted(Minted),
    Viewer(Viewer),
    Graphql(GraphqlRefusal),
    App(App),
    HookConfig(Hook),
    Conversion(Conversion),
    BotUser(u64),
    User(User),
    Permission(api::Permission),
    Compare(Compare),
    Repository(Repository),
    Branch(Branch),
    Issue(Issue),
    /// One whole pull.
    Pull(Pull),
    PullGraphql(PullGraphql),
    /// The combined status after its contexts.
    Combined(Combined),
    Job(Job),
    Rows,
    Error(Refusal),
}

/// An installation's account kind.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Account {
    User,
    Organization,
}

/// An installation's account, creation and suspension.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Installation {
    pub id: u64,
    pub login: Box<[u8]>,
    pub account: u64,
    pub kind: Account,
    pub created: Wall,
    pub suspended: bool,
}

/// Installation access levels kept from a full token mint (forge-github.md, 8.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Level {
    None,
    Read,
    Write,
}
impl Level {
    #[must_use]
    pub fn read(word: &[u8]) -> Self {
        match word {
            b"read" => Self::Read,
            b"write" => Self::Write,
            _ => Self::None,
        }
    }
    #[must_use]
    pub const fn text(self) -> &'static [u8] {
        match self {
            Self::None => b"none",
            Self::Read => b"read",
            Self::Write => b"write",
        }
    }
}
/// The grants needed for the App bot's repository permission.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Levels {
    pub contents: Level,
    pub pull_requests: Level,
}
impl Levels {
    pub const NONE: Self = Self { contents: Level::None, pull_requests: Level::None };
    #[must_use]
    pub const fn permission(self) -> api::Permission {
        match (self.contents, self.pull_requests) {
            (Level::Write, Level::Write) => api::Permission::Write,
            (Level::Read | Level::Write, Level::None | Level::Read) | (Level::Read, Level::Write) => {
                api::Permission::Read
            }
            (Level::None, Level::None | Level::Read | Level::Write) => api::Permission::None,
        }
    }
}
/// An installation token, its expiry and granted access levels.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Minted {
    pub permissions: Levels,
    pub token: Secret,
    pub expires: Wall,
}

/// A repository selectable through an installation.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Reachable {
    pub id: u64,
    pub owner: u64,
    pub login: Box<[u8]>,
    pub name: Box<[u8]>,
    pub private: bool,
}

/// The installation token's bot identity.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Viewer {
    pub id: u64,
    pub login: Box<[u8]>,
}

/// A GraphQL error's type and message; enqueue keeps a non-stale error when present.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct GraphqlRefusal {
    pub kind: Box<[u8]>,
    pub message: Box<[u8]>,
}

/// An App permission's access level.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Access {
    Read,
    Write,
    Admin,
}
impl Access {
    /// Reads GitHub's permission access vocabulary.
    #[must_use]
    pub fn read(text: &[u8]) -> Option<Self> {
        match text {
            b"read" => Some(Self::Read),
            b"write" => Some(Self::Write),
            b"admin" => Some(Self::Admin),
            _ => None,
        }
    }
    /// The access value in an App or manifest document.
    #[must_use]
    pub const fn text(self) -> &'static [u8] {
        match self {
            Self::Read => b"read",
            Self::Write => b"write",
            Self::Admin => b"admin",
        }
    }
    /// Whether this grant covers the wanted access.
    #[must_use]
    pub const fn satisfies(self, wanted: Self) -> bool {
        match self {
            Self::Read => match wanted {
                Self::Read => true,
                Self::Write | Self::Admin => false,
            },
            Self::Write => match wanted {
                Self::Read | Self::Write => true,
                Self::Admin => false,
            },
            Self::Admin => true,
        }
    }
}

/// A named App permission, shared with the manifest and fake.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Permission {
    pub name: Box<[u8]>,
    pub access: Access,
}

/// The App's identity, permissions and event subscriptions.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct App {
    pub id: u64,
    pub slug: Box<[u8]>,
    pub permissions: Box<[Permission]>,
    pub events: Box<[Box<[u8]>]>,
}

/// The App's webhook address.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Hook {
    pub url: Box<[u8]>,
}

/// The identity and secrets returned by a manifest conversion.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Conversion {
    pub id: u64,
    pub slug: Box<[u8]>,
    pub owner: Box<[u8]>,
    pub owner_id: u64,
    pub pem: Secret,
    pub webhook_secret: Secret,
}

/// An issue or pull from GitHub's issue routes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Issue {
    pub number: u64,
    pub pull: bool,
    pub merged: bool,
    pub state: api::State,
    pub author: u64,
    pub labels: Box<[Box<[u8]>]>,
    pub updated_at: Wall,
    pub title: Box<[u8]>,
    pub body: crate::forgejo::document::Body,
}
/// One issue comment; missing times leave its provenance unknown.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub author: u64,
    pub created_at: Option<Wall>,
    pub updated_at: Option<Wall>,
    pub body: crate::forgejo::document::Body,
}

/// A pull's branches, commits, merge state and requested reviewers.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Pull {
    pub number: u64,
    pub closed: bool,
    pub merged: bool,
    pub merge_commit: Option<api::Commit>,
    /// None while GitHub computes mergeability.
    pub mergeable: Option<bool>,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub head_commit: api::Commit,
    pub base_commit: api::Commit,
    pub reviewers: Box<[u64]>,
}
/// One context in a combined status page.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Context {
    pub updated: Option<Wall>,
    pub context: Box<[u8]>,
    pub check: api::Check,
    pub description: Box<[u8]>,
    pub url: Box<[u8]>,
}
/// The combined check and the number of contexts across all pages.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Combined {
    pub check: api::Check,
    pub total: u64,
}
/// A check and optional display and identity fields; Checks validates the identity.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    pub check: api::Check,
    pub id: Option<u64>,
    pub head: Option<api::Commit>,
    pub completed: Option<Wall>,
    pub started: Option<Wall>,
    pub name: Box<[u8]>,
    pub title: Box<[u8]>,
    pub url: Box<[u8]>,
    pub app: Option<Box<[u8]>>,
}
impl Run {
    /// A check-only row is sufficient for Pull's CI fold.
    #[must_use]
    pub fn new(check: api::Check) -> Self {
        Self {
            check,
            id: None,
            head: None,
            completed: None,
            started: None,
            name: Box::from([]),
            title: Box::from([]),
            url: Box::from([]),
            app: None,
        }
    }
}
/// Actions job metadata shared by Checks and the Job read.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Job {
    pub step: Option<api::Step>,
    pub id: u64,
    pub run: u64,
    pub attempt: u32,
    pub head: api::Commit,
}

/// A user id and bounded login for the users table.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct User {
    pub id: u64,
    pub login: Box<[u8]>,
}

/// A collaborator's role and optional bounded login for the users table.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Collaborator {
    pub id: u64,
    pub permission: api::Permission,
    pub login: Option<Box<[u8]>>,
}

/// A review comment with its selected line and bounded body.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Remark {
    pub id: u64,
    pub author: u64,
    pub path: Box<[u8]>,
    pub line: u32,
    pub body: crate::forgejo::document::Body,
}

/// A review before submission filtering and neutral provenance mapping.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub id: u64,
    pub author: u64,
    /// None is a pending review.
    pub verdict: Option<api::Verdict>,
    pub commit: api::Commit,
    pub body: crate::forgejo::document::Body,
    pub submitted: Option<Wall>,
    pub official: bool,
}

/// Comparison completeness and ancestry; merge base is shared with `PullFiles`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Compare {
    pub total_commits: u64,
    pub behind_by: u64,
    pub merge_base: Option<api::Commit>,
}
/// A filename awaiting its raw sides.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    pub status: FileStatus,
    pub previous: Option<Box<[u8]>>,
}
/// Statuses determine which raw sides a pull file can have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FileStatus {
    Added,
    Removed,
    Renamed,
    Modified,
    Changed,
    Copied,
    Unchanged,
}

/// Opaque pull identity and merge queue facts shared by reads and merging.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PullGraphql {
    pub id: Box<[u8]>,
    pub merge_commit: Option<api::Commit>,
    pub queued: Option<u32>,
    pub dequeued: Option<Box<[u8]>>,
}

/// Rules kept from either GitHub protection source (forge-github.md, 5.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent protection facts")]
pub struct Protection {
    pub contexts: Box<[Box<[u8]>]>,
    pub approvals: u32,
    pub dismiss_stale: bool,
    pub code_owners: bool,
    pub linear: bool,
    pub signatures: bool,
    pub queue: bool,
}
impl Protection {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            contexts: Box::from([]),
            approvals: 0,
            dismiss_stale: false,
            code_owners: false,
            linear: false,
            signatures: false,
            queue: false,
        }
    }
}
/// One applicable rule, including unknown types which the plan ignores.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Rule {
    pub ruleset: u64,
    pub kind: RuleKind,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum RuleKind {
    PullRequest { approvals: u32, dismiss_stale: bool, code_owners: bool },
    StatusChecks(Box<[Box<[u8]>]>),
    Queue,
    Signatures,
    Linear,
    Skipped,
}
