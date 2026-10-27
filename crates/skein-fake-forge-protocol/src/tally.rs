//! Requests counted at their HTTP entrance (fake-forge.md, section 4).
use skein_lib::Map;
pub(crate) const ROUTES: u32 = 80;
use skein_forge::forgejo::request::Request;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Route {
    Org,
    CreateOrg,
    CreateRepository,
    CreateFile,
    Teams,
    CreateTeam,
    EditTeam,
    TeamMembers,
    AddMember,
    /// Counts repository collaborator grants.
    AddCollaborator,
    /// Counts repository hook listings.
    RepoHooks,
    /// Counts repository hook creation.
    CreateRepoHook,
    /// Counts repository hook replacements.
    EditRepoHook,
    /// Counts repository hook removal.
    DeleteRepoHook,

    OrgHooks,
    CreateOrgHook,
    EditOrgHook,
    DeleteOrgHook,
    User,
    Tokens,
    CreateToken,
    DeleteToken,
    UserOf,
    CreateUser,
    ApiSettings,
    RepositoryById,
    Repository,
    /// Counts connected-token repository listings.
    UserRepos,
    /// Counts repository search pages used by setup.
    RepoSearch,
    UserSearch,
    Issues,
    Issue,
    Comments,
    Pull,
    PullFor,
    Pulls,
    CombinedStatus,
    Reviews,
    ReviewComments,
    Branch,
    Branches,
    Protection,
    Protections,
    Collaborators,
    Permission,
    PullFiles,
    Compare,
    Commits,
    Raw,
    Tree,
    WorkflowRuns,
    Dispatch,
    Rerun,
    Runs,
    Run,
    Jobs,
    Job,
    JobLog,
    CreateIssue,
    OpenPull,
    PostComment,
    PostReview,
    EditIssue,
    RequestReviewers,
    RemoveReviewers,
    Merge,
    Update,
    PostStatus,
    CreateBranch,
    DeleteBranch,
    Git(crate::git::Name),
}
impl Route {
    pub(crate) const fn is_git(self) -> bool {
        match self {
            Self::Git(_) => true,
            Self::Org
            | Self::CreateFile
            | Self::CreateRepository
            | Self::CreateOrg
            | Self::Teams
            | Self::CreateTeam
            | Self::EditTeam
            | Self::TeamMembers
            | Self::AddMember
            | Self::AddCollaborator
            | Self::RepoHooks
            | Self::CreateRepoHook
            | Self::EditRepoHook
            | Self::DeleteRepoHook
            | Self::OrgHooks
            | Self::CreateOrgHook
            | Self::EditOrgHook
            | Self::DeleteOrgHook
            | Self::User
            | Self::Tokens
            | Self::CreateToken
            | Self::DeleteToken
            | Self::UserOf
            | Self::CreateUser
            | Self::ApiSettings
            | Self::RepositoryById
            | Self::Repository
            | Self::UserRepos
            | Self::RepoSearch
            | Self::UserSearch
            | Self::Issues
            | Self::Issue
            | Self::Comments
            | Self::Pull
            | Self::PullFor
            | Self::Pulls
            | Self::CombinedStatus
            | Self::Reviews
            | Self::ReviewComments
            | Self::Branch
            | Self::Branches
            | Self::Protection
            | Self::Protections
            | Self::Collaborators
            | Self::Permission
            | Self::PullFiles
            | Self::Commits
            | Self::Compare
            | Self::Raw
            | Self::Tree
            | Self::WorkflowRuns
            | Self::Dispatch
            | Self::Rerun
            | Self::Runs
            | Self::Run
            | Self::Jobs
            | Self::Job
            | Self::JobLog
            | Self::CreateIssue
            | Self::OpenPull
            | Self::PostComment
            | Self::PostReview
            | Self::EditIssue
            | Self::RequestReviewers
            | Self::RemoveReviewers
            | Self::Merge
            | Self::Update
            | Self::PostStatus
            | Self::CreateBranch
            | Self::DeleteBranch => false,
        }
    }
}
#[must_use]
pub fn route(request: &Request) -> Route {
    match request {
        Request::Tokens { .. } => Route::Tokens,
        Request::CreateToken { .. } => Route::CreateToken,
        Request::DeleteToken { .. } => Route::DeleteToken,
        Request::Org { .. } => Route::Org,
        Request::CreateFile { .. } => Route::CreateFile,
        Request::CreateRepository { .. } => Route::CreateRepository,
        Request::CreateOrg { .. } => Route::CreateOrg,
        Request::Teams { .. } => Route::Teams,
        Request::CreateTeam { .. } => Route::CreateTeam,
        Request::EditTeam { .. } => Route::EditTeam,
        Request::TeamMembers { .. } => Route::TeamMembers,
        Request::AddMember { .. } => Route::AddMember,
        Request::AddCollaborator { .. } => Route::AddCollaborator,
        Request::RepoHooks { .. } => Route::RepoHooks,
        Request::CreateRepoHook { .. } => Route::CreateRepoHook,
        Request::EditRepoHook { .. } => Route::EditRepoHook,
        Request::DeleteRepoHook { .. } => Route::DeleteRepoHook,

        Request::OrgHooks { .. } => Route::OrgHooks,
        Request::CreateOrgHook { .. } => Route::CreateOrgHook,
        Request::EditOrgHook { .. } => Route::EditOrgHook,
        Request::DeleteOrgHook { .. } => Route::DeleteOrgHook,

        Request::UserOf { .. } => Route::UserOf,
        Request::CreateUser { .. } => Route::CreateUser,
        Request::User => Route::User,
        Request::ApiSettings => Route::ApiSettings,
        Request::RepositoryById { .. } => Route::RepositoryById,
        Request::Repository { .. } => Route::Repository,
        Request::UserRepos { .. } => Route::UserRepos,
        Request::RepoSearch { .. } => Route::RepoSearch,
        Request::UserSearch { .. } => Route::UserSearch,
        Request::Issues { .. } => Route::Issues,
        Request::Issue { .. } => Route::Issue,
        Request::Comments { .. } => Route::Comments,
        Request::Pull { .. } => Route::Pull,
        Request::PullFor { .. } => Route::PullFor,
        Request::Pulls { .. } => Route::Pulls,
        Request::CombinedStatus { .. } => Route::CombinedStatus,
        Request::Reviews { .. } => Route::Reviews,
        Request::ReviewComments { .. } => Route::ReviewComments,
        Request::Branch { .. } => Route::Branch,
        Request::Branches { .. } => Route::Branches,
        Request::Protection { .. } => Route::Protection,
        Request::Protections { .. } => Route::Protections,
        Request::Collaborators { .. } => Route::Collaborators,
        Request::Permission { .. } => Route::Permission,
        Request::PullFiles { .. } => Route::PullFiles,
        Request::Compare { files: false, .. } | Request::Commits { .. } => Route::Commits,
        Request::Compare { files: true, .. } => Route::Compare,
        Request::Raw { .. } => Route::Raw,
        Request::Tree { .. } => Route::Tree,
        Request::WorkflowRuns { .. } => Route::WorkflowRuns,
        Request::Dispatch { .. } => Route::Dispatch,
        Request::Rerun { .. } => Route::Rerun,
        Request::Runs { .. } => Route::Runs,
        Request::Run { .. } => Route::Run,
        Request::Job { .. } => Route::Job,
        Request::Jobs { .. } => Route::Jobs,
        Request::JobLog { .. } => Route::JobLog,
        Request::CreateIssue { .. } => Route::CreateIssue,
        Request::OpenPull { .. } => Route::OpenPull,
        Request::PostComment { .. } => Route::PostComment,
        Request::PostReview { .. } => Route::PostReview,
        Request::EditIssue { .. } => Route::EditIssue,
        Request::RequestReviewers { .. } => Route::RequestReviewers,
        Request::RemoveReviewers { .. } => Route::RemoveReviewers,
        Request::Merge { .. } => Route::Merge,
        Request::Update { .. } => Route::Update,
        Request::PostStatus { .. } => Route::PostStatus,
        Request::CreateBranch { .. } => Route::CreateBranch,
        Request::DeleteBranch { .. } => Route::DeleteBranch,
    }
}
/// Count of read requests, including unrouted and unauthorized entrances.
#[derive(Debug)]
pub struct Tally {
    pub total: u64,
    pub routes: Map<Route, u64>,
    pub users: Map<u64, u64>,
    pub unrouted: u64,
    pub unauthorized: u64,
}
impl Tally {
    #[must_use]
    pub fn new(users: u32) -> Self {
        Self {
            total: 0,
            routes: Map::with_capacity(ROUTES),
            users: Map::with_capacity(users),
            unrouted: 0,
            unauthorized: 0,
        }
    }
    pub(crate) fn count_store(&mut self, route: Option<Route>) {
        let unauthorized = self.unauthorized;
        self.count(route, None);
        self.unauthorized = unauthorized;
    }
    pub fn count(&mut self, route: Option<Route>, user: Option<u64>) {
        self.total = self.total.saturating_add(1);
        if let Some(route) = route {
            let count = self.routes.get(&route).copied().unwrap_or(0).saturating_add(1);
            self.routes.insert(route, count).expect("route capacity");
        } else {
            self.unrouted = self.unrouted.saturating_add(1);
        }
        if let Some(user) = user {
            let count = self.users.get(&user).copied().unwrap_or(0).saturating_add(1);
            self.users.insert(user, count).expect("users bounded by configured and setup token tables");
        } else {
            self.unauthorized = self.unauthorized.saturating_add(1);
        }
    }
}
