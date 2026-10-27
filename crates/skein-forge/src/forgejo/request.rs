//! A request value shared by the client and server.
use alloc::boxed::Box;
use skein_lib::Secret;
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repo {
    pub owner: Box<[u8]>,
    pub name: Box<[u8]>,
}
/// The name and email recorded in a file commit.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Author {
    pub name: Box<[u8]>,
    pub email: Box<[u8]>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub page: Option<u32>,
    pub limit: Option<u32>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IssueKind {
    Issues,
    Pulls,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sort {
    LeastUpdate,
    Other,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Request {
    /// A comparison read retaining parents and messages, with files disabled.
    Commits {
        at: Repo,
        before: super::id::ObjectId,
        after: super::id::ObjectId,
    },
    /// Lists repositories in id order for an admin picker.
    RepoSearch {
        page: Page,
    },
    /// Grants repository access to a collaborator, answering without a document.
    AddCollaborator {
        at: Repo,
        login: Box<[u8]>,
        permission: Grant,
    },
    /// Lists one page of a repository's hooks for setup inspection.
    RepoHooks {
        at: Repo,
        page: Page,
    },
    /// Creates a repository's Forgejo JSON hook.
    CreateRepoHook {
        at: Repo,
        hook: HookSpec,
    },
    /// Replaces a repository hook's complete configuration.
    EditRepoHook {
        at: Repo,
        id: u64,
        hook: HookSpec,
    },
    /// Deletes a repository hook, answering without a document.
    DeleteRepoHook {
        at: Repo,
        id: u64,
    },
    /// Lists one page of an organization's hooks.
    OrgHooks {
        org: Box<[u8]>,
        page: Page,
    },
    /// Creates a Forgejo JSON hook.
    CreateOrgHook {
        org: Box<[u8]>,
        hook: HookSpec,
    },
    /// Replaces the hook configuration, secret, events and active flag.
    EditOrgHook {
        org: Box<[u8]>,
        id: u64,
        hook: HookSpec,
    },
    /// Deletes an organization's hook; answers 204 without a document.
    DeleteOrgHook {
        org: Box<[u8]>,
        id: u64,
    },
    /// Reads an organization by name.
    Org {
        name: Box<[u8]>,
    },
    /// Makes a private organization for a person, on the admin route.
    CreateOrg {
        person: Box<[u8]>,
        name: Box<[u8]>,
    },
    /// Makes an organization repository with a README and default branch `main`.
    CreateRepository {
        org: Box<[u8]>,
        name: Box<[u8]>,
        private: bool,
    },
    /// Commits a missing file, using the author as committer too.
    CreateFile {
        at: Repo,
        path: Box<[u8]>,
        branch: Box<[u8]>,
        content: Box<[u8]>,
        message: Box<[u8]>,
        author: Author,
    },
    /// Lists one page of an organization's teams.
    Teams {
        org: Box<[u8]>,
        page: Page,
    },
    /// Makes a team with the complete specification.
    CreateTeam {
        org: Box<[u8]>,
        spec: crate::forgejo::document::TeamSpec,
    },
    /// Replaces a team's specification.
    EditTeam {
        team: u64,
        spec: crate::forgejo::document::TeamSpec,
    },
    /// Lists one page of a team's members.
    TeamMembers {
        team: u64,
        page: Page,
    },
    /// Adds a member, answering 204 without a document.
    AddMember {
        team: u64,
        login: Box<[u8]>,
    },
    User,
    ApiSettings,
    RepositoryById {
        id: u64,
    },
    Repository {
        at: Repo,
    },
    /// Lists the connected user's repositories, retaining their write permissions.
    UserRepos {
        page: Page,
    },
    UserSearch {
        uid: u64,
        limit: Option<u32>,
    },
    Issues {
        at: Repo,
        state: Option<StateFilter>,
        kind: Option<IssueKind>,
        since: Option<skein_lib::Wall>,
        sort: Option<Sort>,
        page: Page,
    },
    Issue {
        at: Repo,
        number: u64,
    },
    Comments {
        at: Repo,
        number: u64,
    },
    Pull {
        at: Repo,
        number: u64,
    },
    PullFor {
        at: Repo,
        base: Box<[u8]>,
        head: Box<[u8]>,
    },
    CombinedStatus {
        at: Repo,
        commit: crate::forgejo::id::ObjectId,
        page: Page,
    },
    Reviews {
        at: Repo,
        number: u64,
        page: Page,
    },
    ReviewComments {
        at: Repo,
        number: u64,
        review: u64,
    },
    Branch {
        at: Repo,
        name: Box<[u8]>,
    },
    Branches {
        at: Repo,
        prefix: Option<Box<[u8]>>,
        page: Page,
    },
    Protection {
        at: Repo,
        name: Box<[u8]>,
    },
    Protections {
        at: Repo,
    },
    Collaborators {
        at: Repo,
        page: Page,
    },
    Permission {
        at: Repo,
        login: Box<[u8]>,
    },
    Pulls {
        at: Repo,
        page: Page,
    },
    PullFiles {
        at: Repo,
        number: u64,
        page: Page,
    },
    Compare {
        at: Repo,
        before: crate::forgejo::id::ObjectId,
        after: crate::forgejo::id::ObjectId,
        files: bool,
    },
    Raw {
        at: Repo,
        path: Box<[u8]>,
        reference: crate::forgejo::id::ObjectId,
    },
    Tree {
        at: Repo,
        object: crate::forgejo::id::ObjectId,
        page: Page,
    },
    WorkflowRuns {
        at: Repo,
        workflow: Box<[u8]>,
        head_sha: crate::forgejo::id::ObjectId,
        page: Page,
    },
    Dispatch {
        at: Repo,
        workflow: Box<[u8]>,
        branch: Box<[u8]>,
        inputs: Box<[crate::api::Input]>,
    },
    Rerun {
        at: Repo,
        job: u64,
    },
    Runs {
        at: Repo,
        head_sha: crate::forgejo::id::ObjectId,
        run_number: u64,
        page: Page,
    },
    Run {
        at: Repo,
        run: u64,
    },
    Jobs {
        at: Repo,
        run: u64,
    },
    Job {
        at: Repo,
        job: u64,
    },
    JobLog {
        at: Repo,
        job: u64,
        attempt: u32,
        step: Option<u32>,
    },
    CreateIssue {
        at: Repo,
        title: Box<[u8]>,
        body: Box<[u8]>,
    },
    OpenPull {
        at: Repo,
        head: Box<[u8]>,
        base: Box<[u8]>,
        title: Box<[u8]>,
        body: Box<[u8]>,
    },
    PostComment {
        at: Repo,
        number: u64,
        body: Box<[u8]>,
    },
    PostReview {
        at: Repo,
        number: u64,
        event: ReviewEvent,
        body: Box<[u8]>,
    },
    EditIssue {
        at: Repo,
        number: u64,
        title: Option<Box<[u8]>>,
        body: Option<Box<[u8]>>,
        state: Option<crate::forgejo::document::IssueState>,
    },
    RequestReviewers {
        at: Repo,
        number: u64,
        reviewers: Box<[Box<[u8]>]>,
    },
    RemoveReviewers {
        at: Repo,
        number: u64,
        reviewers: Box<[Box<[u8]>]>,
    },
    Merge {
        at: Repo,
        number: u64,
        style: crate::forgejo::document::MergeStyle,
        head: crate::forgejo::id::ObjectId,
    },
    Update {
        at: Repo,
        number: u64,
        style: Option<UpdateStyle>,
    },
    PostStatus {
        at: Repo,
        commit: crate::forgejo::id::ObjectId,
        state: crate::forgejo::document::StatusState,
        context: Box<[u8]>,
        description: Box<[u8]>,
        link: Box<[u8]>,
    },
    CreateBranch {
        at: Repo,
        branch: Box<[u8]>,
        from: crate::forgejo::id::ObjectId,
    },
    DeleteBranch {
        at: Repo,
        branch: Box<[u8]>,
    },
    Tokens {
        login: Box<[u8]>,
        page: Page,
    },
    CreateToken {
        login: Box<[u8]>,
        name: Box<[u8]>,
        scopes: Box<[Box<[u8]>]>,
    },
    DeleteToken {
        login: Box<[u8]>,
        id: u64,
    },
    UserOf {
        login: Box<[u8]>,
    },
    CreateUser {
        login: Box<[u8]>,
        email: Box<[u8]>,
        full_name: Box<[u8]>,
        password: Secret,
        must_change_password: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ReviewEvent {
    Approved,
    RequestChanges,
    Comment,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UpdateStyle {
    Merge,
    Rebase,
}

/// The configuration sent when creating or patching a hook.
/// The encoder fixes its type to `forgejo` and its content type to JSON.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct HookSpec {
    pub url: Box<[u8]>,
    pub secret: Secret,
    pub events: Box<[Box<[u8]>]>,
    pub active: bool,
}

/// A collaborator grant excludes the absence of permission from a write body.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Grant {
    /// Allows reading the repository.
    Read,
    /// Allows writing the repository.
    Write,
    /// Allows administering the repository.
    Admin,
}
impl Grant {
    pub(crate) fn text(self) -> &'static [u8] {
        match self {
            Self::Read => b"read",
            Self::Write => b"write",
            Self::Admin => b"admin",
        }
    }
    pub(crate) fn read(text: &[u8]) -> Option<Self> {
        if text == b"read" {
            Some(Self::Read)
        } else if text == b"write" {
            Some(Self::Write)
        } else if text == b"admin" {
            Some(Self::Admin)
        } else {
            None
        }
    }
}
