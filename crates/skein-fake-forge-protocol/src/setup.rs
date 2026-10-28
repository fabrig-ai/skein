//! Setup's protocol state, separate from repository permissions (testing.md, 4.2).
use crate::{Config, Limits, response::Reply};
use alloc::boxed::Box;
use skein_forge::forgejo::{
    document::{self, User},
    request::Request,
    server::{self, Answer},
};
use skein_lib::{List, Secret, Writer, bytes};
mod accounts;
mod collaborators;
mod creation;
mod hooks;
mod reads;
mod repo_hooks;
pub use repo_hooks::{RepositoryHook, RepositoryHooks};
mod orgs;
pub(crate) mod repos;
mod tokens;
pub use hooks::Subscription;
pub use orgs::{Organization, Team};
/// A setup account; its password is never retained.
#[derive(Debug)]
pub struct Account {
    pub id: u64,
    pub login: Box<[u8]>,
    pub admin: bool,
    pub full_name: Box<[u8]>,
}
/// An issued token, including the id used by its listing and deletion.
#[derive(Debug)]
pub struct Issued {
    pub id: u64,
    pub user: u64,
    pub name: Box<[u8]>,
    pub scopes: Box<[Box<[u8]>]>,
    pub secret: Secret,
}
/// Bounded scenario state for the setup routes.
#[derive(Debug)]
pub struct Store {
    accounts: List<Account>,
    tokens: List<Issued>,
    orgs: List<Organization>,
    teams: List<Team>,
    hooks: List<Subscription>,
    private: List<u64>,
    /// Setup grants, mirrored into a domain when the owner lends it for the call.
    collaborators: List<collaborators::Collaborator>,
    /// Repository hooks made by setup, in creation id order.
    repo_hooks: List<RepositoryHook>,
    next_repo_hook: u64,
    next_org: u64,
    next_team: u64,
    next_hook: u64,
    next_account: u64,
    next_token: u64,
    string_bytes: usize,
    secret_bytes: usize,
}
impl Store {
    pub(crate) fn new(limits: &Limits) -> Self {
        let cap = limits.documents.inner;
        Self {
            accounts: List::with_capacity(cap),
            tokens: List::with_capacity(cap),
            orgs: List::with_capacity(cap),
            teams: List::with_capacity(cap),
            hooks: List::with_capacity(cap),
            private: List::with_capacity(cap),
            collaborators: List::with_capacity(cap),
            repo_hooks: List::with_capacity(cap),
            next_repo_hook: 1,
            next_org: 1,
            next_team: 1,
            next_hook: 1,
            next_account: 1,
            next_token: 1,
            string_bytes: usize::try_from(limits.documents.string_bytes).expect("u32 fits"),
            secret_bytes: usize::try_from(limits.http.head).expect("u32 fits"),
        }
    }
    #[must_use]
    pub fn find(&self, login: &[u8]) -> Option<&Account> {
        let mut found = None;
        for account in &self.accounts {
            if account.login.as_ref() == login {
                found = Some(account);
            }
        }
        found
    }
    /// All currently issued tokens for scenario inspection.
    #[must_use]
    pub fn issued(&self) -> &[Issued] {
        self.tokens.as_slice()
    }
    pub(crate) fn user(&self, id: u64) -> Option<User> {
        for account in &self.accounts {
            if account.id == id {
                return Some(User {
                    id,
                    login: account.login.clone(),
                    is_admin: account.admin,
                    full_name: account.full_name.clone(),
                });
            }
        }
        None
    }
    pub(crate) fn resolve_user(&self, config: &Config, id: u64) -> User {
        match self.user(id) {
            Some(user) => user,
            None => crate::routes::user(config, id),
        }
    }
    #[must_use]
    pub fn token_user(&self, secret: &[u8]) -> Option<u64> {
        for token in &self.tokens {
            if token.secret.expose() == secret {
                return Some(token.user);
            }
        }
        None
    }
    pub(crate) fn accesses_repository(&self, secret: &[u8], write: bool) -> bool {
        self.permits(secret, b"read:repository", b"write:repository", write)
    }
    pub(crate) fn permits(&self, secret: &[u8], read: &[u8], write: &[u8], writing: bool) -> bool {
        for token in &self.tokens {
            if token.secret.expose() == secret {
                for scope in &token.scopes {
                    if scope.as_ref() == b"all" || scope.as_ref() == write || (!writing && scope.as_ref() == read) {
                        return true;
                    }
                }
            }
        }
        false
    }
    #[must_use]
    pub fn scopes(&self, user: u64, name: &[u8]) -> Option<&[Box<[u8]>]> {
        for token in &self.tokens {
            if token.user == user && token.name.as_ref() == name {
                return Some(&token.scopes);
            }
        }
        None
    }
    pub(crate) fn answer(&mut self, request: &Request, user: u64, config: &Config, limits: &Limits) -> Option<Reply> {
        match request {
            Request::RepoSearch { page } => Some(self.search(config, user, *page, limits)),
            Request::RepositoryById { id } => {
                let caller = self.user(user)?;
                Some(self.repository_by_id(*id, &caller, config, limits, None))
            }
            Request::Permission { at, login } => {
                let caller = self.user(user)?;
                Some(self.read_permission(at, login, &caller, config, limits))
            }
            Request::User => {
                let user = self.user(user)?;
                Some(encoded(200, Answer::User(user), limits))
            }
            Request::UserOf { login } => Some(match self.find(login) {
                Some(account) => encoded(200, Answer::User(self.user(account.id).expect("account found")), limits),
                None => refusal(404, &joined(&[b"user redirect does not exist [name: ", login, b"]"]), limits),
            }),
            Request::CreateUser { .. }
            | Request::Tokens { .. }
            | Request::CreateToken { .. }
            | Request::DeleteToken { .. }
            | Request::Org { .. }
            | Request::CreateOrg { .. }
            | Request::CreateFile { .. }
            | Request::CreateRepository { .. }
            | Request::Teams { .. }
            | Request::CreateTeam { .. }
            | Request::EditTeam { .. }
            | Request::TeamMembers { .. }
            | Request::OrgHooks { .. }
            | Request::CreateOrgHook { .. }
            | Request::EditOrgHook { .. }
            | Request::DeleteOrgHook { .. }
            | Request::AddCollaborator { .. }
            | Request::RepoHooks { .. }
            | Request::CreateRepoHook { .. }
            | Request::EditRepoHook { .. }
            | Request::DeleteRepoHook { .. }
            | Request::AddMember { .. } => {
                let admin = match self.user(user) {
                    Some(account) => account.is_admin,
                    None => false,
                };
                Some(if admin {
                    self.admin(request, config, limits)
                } else {
                    refusal(403, b"user should be the site admin", limits)
                })
            }
            Request::ApiSettings
            | Request::Repository { .. }
            | Request::UserRepos { .. }
            | Request::UserSearch { .. }
            | Request::Issues { .. }
            | Request::Issue { .. }
            | Request::Comments { .. }
            | Request::Pull { .. }
            | Request::PullFor { .. }
            | Request::CombinedStatus { .. }
            | Request::Reviews { .. }
            | Request::ReviewComments { .. }
            | Request::Branch { .. }
            | Request::Branches { .. }
            | Request::Protection { .. }
            | Request::Protections { .. }
            | Request::Collaborators { .. }
            | Request::Pulls { .. }
            | Request::PullFiles { .. }
            | Request::Compare { .. }
            | Request::Contents { .. }
            | Request::Raw { .. }
            | Request::Tree { .. }
            | Request::WorkflowRuns { .. }
            | Request::Dispatch { .. }
            | Request::Rerun { .. }
            | Request::Runs { .. }
            | Request::Run { .. }
            | Request::Jobs { .. }
            | Request::Job { .. }
            | Request::JobLog { .. }
            | Request::CreateIssue { .. }
            | Request::OpenPull { .. }
            | Request::PostComment { .. }
            | Request::PostReview { .. }
            | Request::EditIssue { .. }
            | Request::RequestReviewers { .. }
            | Request::RemoveReviewers { .. }
            | Request::Merge { .. }
            | Request::Update { .. }
            | Request::PostStatus { .. }
            | Request::CreateBranch { .. }
            | Request::DeleteBranch { .. } => None,
        }
    }
    fn admin(&mut self, request: &Request, config: &Config, limits: &Limits) -> Reply {
        match request {
            Request::AddCollaborator { at, login, permission } => {
                self.add_collaborator(at, login, *permission, config, limits)
            }
            Request::RepoHooks { at, page } => self.list_repo_hooks(at, *page, config, limits),
            Request::CreateRepoHook { at, hook } => self.create_repo_hook(at, hook, config, limits),
            Request::EditRepoHook { at, id, hook } => self.edit_repo_hook(at, *id, hook, config, limits),
            Request::DeleteRepoHook { at, id } => self.remove_repo_hook(at, *id, config, limits),
            Request::CreateUser { login, full_name, .. } => self.create_account(login, full_name, limits),
            Request::Tokens { login, page } => self.list_tokens(login, *page, config, limits),
            Request::CreateToken { login, name, scopes } => self.create_token(login, name, scopes, limits),
            Request::DeleteToken { login, id } => self.delete_token(login, *id, limits),
            Request::Org { name } => self.read_org(name, limits),
            Request::CreateOrg { person, name } => self.create_org(person, name, limits),
            Request::CreateRepository { .. } | Request::CreateFile { .. } => full(limits),
            Request::Teams { org, page } => self.list_teams(org, *page, config, limits),
            Request::CreateTeam { org, spec } => self.create_team(org, spec, limits),
            Request::EditTeam { team, spec } => self.patch_team(*team, spec, limits),
            Request::TeamMembers { team, page } => self.members(*team, *page, config, limits),
            Request::AddMember { team, login } => self.add_member(*team, login, limits),
            Request::OrgHooks { org, page } => self.list_hooks(org, *page, config, limits),
            Request::CreateOrgHook { org, hook } => self.create_hook(org, hook, limits),
            Request::EditOrgHook { org, id, hook } => self.edit_hook(org, *id, hook, limits),
            Request::DeleteOrgHook { org, id } => self.remove_hook(org, *id, limits),
            Request::User
            | Request::ApiSettings
            | Request::RepositoryById { .. }
            | Request::Repository { .. }
            | Request::UserRepos { .. }
            | Request::RepoSearch { .. }
            | Request::UserSearch { .. }
            | Request::Issues { .. }
            | Request::Issue { .. }
            | Request::Comments { .. }
            | Request::Pull { .. }
            | Request::PullFor { .. }
            | Request::CombinedStatus { .. }
            | Request::Reviews { .. }
            | Request::ReviewComments { .. }
            | Request::Branch { .. }
            | Request::Branches { .. }
            | Request::Protection { .. }
            | Request::Protections { .. }
            | Request::Collaborators { .. }
            | Request::Permission { .. }
            | Request::Pulls { .. }
            | Request::PullFiles { .. }
            | Request::Compare { .. }
            | Request::Contents { .. }
            | Request::Raw { .. }
            | Request::Tree { .. }
            | Request::WorkflowRuns { .. }
            | Request::Dispatch { .. }
            | Request::Rerun { .. }
            | Request::Runs { .. }
            | Request::Run { .. }
            | Request::Jobs { .. }
            | Request::Job { .. }
            | Request::JobLog { .. }
            | Request::CreateIssue { .. }
            | Request::OpenPull { .. }
            | Request::PostComment { .. }
            | Request::PostReview { .. }
            | Request::EditIssue { .. }
            | Request::RequestReviewers { .. }
            | Request::RemoveReviewers { .. }
            | Request::Merge { .. }
            | Request::Update { .. }
            | Request::PostStatus { .. }
            | Request::CreateBranch { .. }
            | Request::DeleteBranch { .. }
            | Request::UserOf { .. } => unreachable!("only setup admin routes"),
        }
    }
}
fn encoded(status: u16, answer: Answer, limits: &Limits) -> Reply {
    match server::encode(&answer, &limits.documents) {
        Ok(body) => Reply { status, headers: Box::new([]), body },
        Err(_) => Reply { status: 413, headers: Box::new([]), body: Box::new([]) },
    }
}
fn refusal(status: u16, message: &[u8], limits: &Limits) -> Reply {
    encoded(
        status,
        Answer::Error(document::ErrorBody { message: document::Body::Kept(bytes::copy_of(message)) }),
        limits,
    )
}

fn joined(parts: &[&[u8]]) -> Box<[u8]> {
    let mut length = 0_usize;
    for part in parts {
        length = length.saturating_add(part.len());
    }
    let mut writer = Writer::new(length);
    for part in parts {
        writer.put(part).expect("sized setup message");
    }
    writer.finish()
}

fn empty() -> Reply {
    Reply { status: 204, headers: Box::new([]), body: Box::new([]) }
}
fn absent(limits: &Limits) -> Reply {
    refusal(404, b"The target couldn't be found.", limits)
}
fn full(limits: &Limits) -> Reply {
    refusal(503, b"setup store is full", limits)
}

impl Store {
    /// Removes a scenario account and its tokens and memberships.
    pub fn delete_account(&mut self, login: &[u8]) {
        for index in 0..self.accounts.len() {
            let account = self.accounts.get(index).expect("account index");
            if account.login.as_ref() == login {
                let id = account.id;
                remove(&mut self.accounts, index);
                for index in (0..self.tokens.len()).rev() {
                    if self.tokens.get(index).expect("token index").user == id {
                        remove(&mut self.tokens, index);
                    }
                }
                for index in 0..self.teams.len() {
                    let team = self.teams.get(index).expect("team index").id;
                    self.remove_member(team, id);
                }
                return;
            }
        }
    }
    /// Revokes one scenario token by its secret.
    pub fn revoke(&mut self, secret: &[u8]) {
        for index in 0..self.tokens.len() {
            if self.tokens.get(index).expect("token index").secret.expose() == secret {
                remove(&mut self.tokens, index);
                return;
            }
        }
    }
    /// Adds a scenario member to an existing team.
    pub fn member(&mut self, id: u64, user: u64) {
        for index in 0..self.teams.len() {
            let team = self.teams.get_mut(index).expect("team index");
            if team.id == id {
                team.members.push(user).expect("member capacity");
                return;
            }
        }
    }
    /// Removes one member without changing the team's grants.
    pub fn remove_member(&mut self, id: u64, user: u64) {
        for index in 0..self.teams.len() {
            let team = self.teams.get_mut(index).expect("team index");
            if team.id == id {
                for index in 0..team.members.len() {
                    if team.members.get(index) == Some(&user) {
                        remove(&mut team.members, index);
                        return;
                    }
                }
            }
        }
    }
    /// Deletes one scenario hook by id.
    pub fn delete_hook(&mut self, id: u64) {
        for index in 0..self.hooks.len() {
            if self.hooks.get(index).expect("hook index").id == id {
                remove(&mut self.hooks, index);
                return;
            }
        }
    }
    /// Changes the grants a person has edited on a scenario team.
    pub fn edit_team(&mut self, id: u64, spec: document::TeamSpec) {
        assert!(spec.name.len() <= self.string_bytes, "scenario team name bound");
        for index in 0..self.teams.len() {
            let team = self.teams.get_mut(index).expect("team index");
            if team.id == id {
                team.spec = spec;
                return;
            }
        }
    }
}
fn remove<T>(rows: &mut List<T>, at: u32) {
    let old = core::mem::replace(rows, List::with_capacity(rows.capacity()));
    for (index, row) in old.into_boxed().into_iter().enumerate() {
        if index != usize::try_from(at).expect("u32 fits") {
            rows.push(row).ok().expect("same table capacity");
        }
    }
}
