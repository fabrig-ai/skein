//! Typed fields retained from Forgejo's answers.
use super::id::{ObjectFormat, ObjectId};
use alloc::boxed::Box;
use skein_lib::Wall;
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Body {
    Kept(Box<[u8]>),
    Long(u64),
}
impl Body {
    /// Reads the tokenizer's capped body string, retaining a long body's length alone.
    pub fn read(
        event: &skein_json::tokenizer::Event,
        field: &'static [u8],
    ) -> Result<Self, crate::forgejo::decode::Malformed> {
        match event {
            skein_json::tokenizer::Event::Token(skein_json::Token::String(bytes)) => Ok(Self::Kept(bytes.clone())),
            skein_json::tokenizer::Event::Long(length) => Ok(Self::Long(*length)),
            skein_json::tokenizer::Event::Failed(error) => Err(crate::forgejo::decode::Malformed::Json(*error)),
            skein_json::tokenizer::Event::Token(
                skein_json::Token::ObjectStart
                | skein_json::Token::ObjectEnd
                | skein_json::Token::ArrayStart
                | skein_json::Token::ArrayEnd
                | skein_json::Token::Key(_)
                | skein_json::Token::Number(_)
                | skein_json::Token::True
                | skein_json::Token::False
                | skein_json::Token::Null,
            )
            | skein_json::tokenizer::Event::Skipped(_)
            | skein_json::tokenizer::Event::Done
            | skein_json::tokenizer::Event::Closed => Err(crate::forgejo::decode::Malformed::Shape { field }),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct User {
    pub id: u64,
    pub login: Box<[u8]>,
    pub is_admin: bool,
    pub full_name: Box<[u8]>,
}
/// An access token's value is present only in its creation answer.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct AccessToken {
    pub id: u64,
    pub name: Box<[u8]>,
    pub scopes: Box<[Box<[u8]>]>,
    pub last_eight: Box<[u8]>,
    pub value: Option<skein_lib::Secret>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ApiSettings {
    pub max_response_items: u32,
    pub default_paging_num: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MergeStyle {
    Merge,
    Rebase,
    RebaseMerge,
    Squash,
    FastForwardOnly,
    ManuallyMerged,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CreatedRepository {
    pub id: u64,
    pub full_name: Box<[u8]>,
    pub default_branch: Box<[u8]>,
    pub clone_url: Box<[u8]>,
    pub ssh_url: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    pub id: u64,
    pub owner: User,
    pub name: Box<[u8]>,
    pub object_format: ObjectFormat,
    pub default_branch: Box<[u8]>,
    pub default_merge_style: MergeStyle,
    pub allow_merge_commits: bool,
    pub allow_squash_merge: bool,
    pub allow_rebase: bool,
    pub html_url: Box<[u8]>,
}
/// A repository a picker can reach, shared by connected and admin listings.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Reachable {
    pub repository: u64,
    pub owner: u64,
    pub login: Box<[u8]>,
    pub name: Box<[u8]>,
    pub private: bool,
    /// The row's `permissions.push`, or true for an admin search.
    pub push: bool,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Row {
    CommitMessage(CommitMessage),
    /// Retains every repository so the setup host can filter after counting the page.
    Reachable(Reachable),
    /// Admin search reaches every repository, so each row has push set true.
    Found(Reachable),
    Hook(Hook),
    Token(AccessToken),
    Team(Team),
    User(User),
    Issue(Issue),
    Comment(Comment),
    Status(CommitStatus),
    Review(Review),
    ReviewComment(ReviewComment),
    Branch(Branch),
    Protection(BranchProtection),
    Pull(PullRequest),
    File(ChangedFile),
    Commit(ObjectId),
    Entry(Entry),
    Run(Run),
    Job(Job),
    Step(Step),
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Document {
    Hook(Hook),
    Token(AccessToken),
    Org(Org),
    Team(Team),
    User(User),
    ApiSettings(ApiSettings),
    Repository(Repository),
    CreatedRepository(CreatedRepository),
    /// The commit made by the contents API.
    CreatedFile {
        commit: ObjectId,
    },
    Search {
        ok: bool,
    },
    /// Reports the search flag for the setup host to judge.
    RepoSearch {
        ok: bool,
    },
    Listed {
        skipped: u32,
    },
    Issue(Issue),
    Comment(Comment),
    Status(CommitStatus),
    Review(Review),
    Pull(PullRequest),
    CombinedStatus(CombinedStatus),
    Branch(Branch),
    Protection(BranchProtection),
    Permission(CollaboratorPermission),
    Compare(Compare),
    Tree {
        truncated: bool,
    },
    Runs {
        total_count: u64,
    },
    Run(Run),
    Job(Job),
    Error(ErrorBody),
}
/// An error message capped at `string_bytes`, retaining its length when long.
/// A real 401 may echo the API token in this message; callers must never log it.
/// Fake 401 answers use only `token is required`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ErrorBody {
    pub message: Body,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IssueState {
    Open,
    Closed,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Issue {
    pub number: u64,
    pub pull: bool,
    pub state: IssueState,
    pub user: User,
    pub labels: Box<[Box<[u8]>]>,
    pub updated_at: Wall,
    pub title: Box<[u8]>,
    pub body: Body,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub user: User,
    pub created_at: Option<Wall>,
    pub updated_at: Option<Wall>,
    pub body: Body,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PullBranch {
    pub repo_id: u64,
    pub name: Box<[u8]>,
    pub sha: Option<ObjectId>,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PullRequest {
    pub number: u64,
    pub state: IssueState,
    pub head: PullBranch,
    pub base: PullBranch,
    pub merged: bool,
    pub merge_commit_sha: Option<ObjectId>,
    pub merge_base: Option<ObjectId>,
    pub mergeable: bool,
    pub requested_reviewers: Box<[User]>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StatusState {
    Pending,
    Success,
    Error,
    Failure,
    Warning,
    Skipped,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CombinedStatus {
    pub state: Option<StatusState>,
    pub total_count: u64,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CommitStatus {
    /// None for an Actions status whose creator is null (ruling 111).
    pub creator: Option<User>,
    pub updated_at: Wall,
    pub context: Box<[u8]>,
    pub status: StatusState,
    pub description: Box<[u8]>,
    pub target_url: Box<[u8]>,
}
pub(crate) fn user() -> User {
    User { id: 0, login: Box::from([]), is_admin: false, full_name: Box::from([]) }
}
pub(crate) fn repository() -> Repository {
    Repository {
        id: 0,
        owner: user(),
        name: Box::from([]),
        object_format: ObjectFormat::Sha1,
        default_branch: Box::from([]),
        default_merge_style: MergeStyle::Merge,
        allow_merge_commits: false,
        allow_squash_merge: false,
        allow_rebase: false,
        html_url: Box::from([]),
    }
}
pub(crate) fn style(text: &[u8]) -> Option<MergeStyle> {
    match text {
        b"merge" => Some(MergeStyle::Merge),
        b"rebase" => Some(MergeStyle::Rebase),
        b"rebase-merge" => Some(MergeStyle::RebaseMerge),
        b"squash" => Some(MergeStyle::Squash),
        b"fast-forward-only" => Some(MergeStyle::FastForwardOnly),
        b"manually-merged" => Some(MergeStyle::ManuallyMerged),
        _ => None,
    }
}
pub(crate) fn style_text(style: MergeStyle) -> &'static [u8] {
    match style {
        MergeStyle::Merge => b"merge",
        MergeStyle::Rebase => b"rebase",
        MergeStyle::RebaseMerge => b"rebase-merge",
        MergeStyle::Squash => b"squash",
        MergeStyle::FastForwardOnly => b"fast-forward-only",
        MergeStyle::ManuallyMerged => b"manually-merged",
    }
}

/// A review's `state` (06.3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ReviewState {
    /// `APPROVED`.
    Approved,
    /// `REQUEST_CHANGES`.
    RequestChanges,
    /// `COMMENT`.
    Comment,
    /// `PENDING`: not a review yet; 13 skips it.
    Pending,
    /// `REQUEST_REVIEW`: a request, not a review; 13 skips it.
    RequestReview,
}

/// A pull request's review (06.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    pub id: u64,
    pub user: User,
    pub state: ReviewState,
    pub commit_id: ObjectId,
    pub body: Body,
    pub submitted_at: Option<Wall>,
    pub updated_at: Option<Wall>,
    pub official: bool,
    pub dismissed: bool,
}

/// One comment of a review: a remark (06.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ReviewComment {
    pub id: u64,
    pub user: User,
    pub path: Box<[u8]>,
    pub position: u32,
    pub body: Body,
}

/// A branch, alone or as a row of the branches' listing (06.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Branch {
    pub name: Box<[u8]>,
    /// `commit.id`.
    pub commit: ObjectId,
    /// An empty name is `None`: no rule applies.
    pub effective_branch_protection_name: Option<Box<[u8]>>,
}

/// A branch protection rule (06.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BranchProtection {
    pub rule_name: Box<[u8]>,
    pub enable_status_check: bool,
    /// At most `Limits::inner`.
    pub status_check_contexts: Box<[Box<[u8]>]>,
    pub required_approvals: u32,
    pub dismiss_stale_approvals: bool,
    /// Whether commits must be signed.
    pub require_signed_commits: bool,
}

/// A collaborator's `permission` (06.3); `owner` is its own value, read as admin by 13.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Permission {
    /// `none`.
    None,
    /// `read`.
    Read,
    /// `write`.
    Write,
    /// `admin`.
    Admin,
    /// `owner`.
    Owner,
}

/// `GET R/collaborators/{login}/permission` (06.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CollaboratorPermission {
    pub permission: Permission,
    pub user: User,
}

/// A changed file's `status` (06.4), as the transcripts spell them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FileStatus {
    /// `added`: no side before.
    Added,
    /// A deletion, as the transcripts spell it: no side after.
    Deleted,
    /// `modified`.
    Modified,
    /// `renamed`: the side before at `previous_filename`.
    Renamed,
    /// `copied`.
    Copied,
}

/// A pull request's or a comparison's changed file (06.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ChangedFile {
    pub filename: Box<[u8]>,
    pub previous_filename: Option<Box<[u8]>>,
    pub status: FileStatus,
}

/// A comparison's own field; its `commits[].sha` and `files` come as rows (06.4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Compare {
    pub total_commits: u64,
}

/// An Actions run (06.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    pub id: u64,
    pub index_in_repo: u64,
    pub commit_sha: ObjectId,
    pub workflow: Box<[u8]>,
    pub event: Box<[u8]>,
    pub trigger_actor: u64,
    pub status: StepStatus,
    pub html_url: Box<[u8]>,
    pub created_at: Wall,
}

/// An Actions job of a run (06.4).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Job {
    pub id: u64,
    pub run_id: u64,
    pub attempt: u32,
    pub name: Box<[u8]>,
    pub status: StepStatus,
}

/// One step of a job's latest attempt, delivered as a streaming row.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Step {
    pub number: u32,
    /// Empty when the name exceeds `string_bytes` (forge.md, 3.3).
    pub name: Box<[u8]>,
    pub status: StepStatus,
}
/// Forgejo's Actions status words.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StepStatus {
    Unknown,
    Waiting,
    Running,
    Success,
    Failure,
    Cancelled,
    Skipped,
    Blocked,
}
pub(crate) fn step_status(text: &[u8]) -> Option<StepStatus> {
    match text {
        b"unknown" => Some(StepStatus::Unknown),
        b"waiting" => Some(StepStatus::Waiting),
        b"running" => Some(StepStatus::Running),
        b"success" => Some(StepStatus::Success),
        b"failure" => Some(StepStatus::Failure),
        b"cancelled" => Some(StepStatus::Cancelled),
        b"skipped" => Some(StepStatus::Skipped),
        b"blocked" => Some(StepStatus::Blocked),
        _ => None,
    }
}
pub(crate) fn step_status_text(status: StepStatus) -> &'static [u8] {
    match status {
        StepStatus::Unknown => b"unknown",
        StepStatus::Waiting => b"waiting",
        StepStatus::Running => b"running",
        StepStatus::Success => b"success",
        StepStatus::Failure => b"failure",
        StepStatus::Cancelled => b"cancelled",
        StepStatus::Skipped => b"skipped",
        StepStatus::Blocked => b"blocked",
    }
}

/// A team's permission or a unit's access in Forgejo's words.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Access {
    None,
    Read,
    Write,
    Admin,
    Owner,
}
/// The four repository units the dialect uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Units {
    pub code: Access,
    pub issues: Access,
    pub pulls: Access,
    pub actions: Access,
}
/// The same specification is sent when making and editing a team.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TeamSpec {
    pub name: Box<[u8]>,
    pub permission: Access,
    pub includes_all_repositories: bool,
    pub units: Units,
}
/// A team's id and retained specification.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Team {
    pub id: u64,
    pub spec: TeamSpec,
}
/// An organization's id and login, read from `name`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Org {
    pub id: u64,
    pub name: Box<[u8]>,
}
/// Reads one of Forgejo's five access words.
#[must_use]
pub fn access(text: &[u8]) -> Option<Access> {
    match text {
        b"none" => Some(Access::None),
        b"read" => Some(Access::Read),
        b"write" => Some(Access::Write),
        b"admin" => Some(Access::Admin),
        b"owner" => Some(Access::Owner),
        _ => None,
    }
}
/// Spells an access in Forgejo's words.
#[must_use]
pub fn access_text(access: Access) -> &'static [u8] {
    match access {
        Access::None => b"none",
        Access::Read => b"read",
        Access::Write => b"write",
        Access::Admin => b"admin",
        Access::Owner => b"owner",
    }
}
pub(crate) fn team() -> Team {
    Team {
        id: 0,
        spec: TeamSpec {
            name: Box::from([]),
            permission: Access::None,
            includes_all_repositories: false,
            units: Units { code: Access::None, issues: Access::None, pulls: Access::None, actions: Access::None },
        },
    }
}

/// An organization hook as listed. Its target is `config.url`; no secret is exposed.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Hook {
    pub id: u64,
    pub url: Box<[u8]>,
    pub events: Box<[Box<[u8]>]>,
    pub active: bool,
}

/// One nonrecursive git tree entry (forge.md, 5.2).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Entry {
    pub path: Box<[u8]>,
    pub mode: u32,
    pub kind: crate::api::EntryKind,
    pub sha: ObjectId,
}
pub(crate) fn run() -> Run {
    Run {
        id: 0,
        index_in_repo: 0,
        commit_sha: ObjectId { format: ObjectFormat::Sha1, bytes: [0; 32] },
        workflow: Box::new([]),
        event: Box::new([]),
        trigger_actor: 0,
        status: StepStatus::Unknown,
        html_url: Box::new([]),
        created_at: Wall::from_nanos(0),
    }
}
pub(crate) fn job() -> Job {
    Job { id: 0, run_id: 0, attempt: 0, name: Box::new([]), status: StepStatus::Unknown }
}

/// A comparison's full commit fields.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CommitMessage {
    pub id: ObjectId,
    pub parents: Box<[ObjectId]>,
    pub message: Box<[u8]>,
}
