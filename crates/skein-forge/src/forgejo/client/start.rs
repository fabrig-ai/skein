use super::{Encoded, Refusal};
use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Kind, Move, Value},
    },
    document::{self, ApiSettings, Document, Repository, Row, User},
    id::ObjectFormat,
    limits::Limits,
    path,
    request::{Method, Request},
};
use skein_json::tokenizer;
use skein_lib::{Decimal, Writer};
#[expect(clippy::too_many_lines, reason = "the encoder exhaustively names every dialect request")]
pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let length = match request {
        Request::RepoSearch { page } => {
            let mut out = super::items::Target { length: 0, writer: None };
            out.put(b"/api/v1/repos/search?sort=id&order=asc");
            out.page(*page, true);
            out.length
        }
        Request::UserRepos { page } => {
            let mut out = super::items::Target { length: 0, writer: None };
            out.put(b"/api/v1/user/repos");
            out.page(*page, false);
            out.length
        }
        Request::User => 12,
        Request::ApiSettings => 20,
        Request::RepositoryById { id } => 21usize.saturating_add(Decimal::of(*id).as_bytes().len()),
        Request::Repository { at } => {
            15usize.saturating_add(path::segment_length(&at.owner)).saturating_add(path::segment_length(&at.name))
        }
        Request::UserSearch { uid, limit } => {
            let mut size = 25usize.saturating_add(Decimal::of(*uid).as_bytes().len());
            if let Some(limit) = limit {
                size = size.saturating_add(7).saturating_add(Decimal::of(u64::from(*limit)).as_bytes().len());
            }
            size
        }
        Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Artifacts { .. }
        | Request::ArtifactZip { .. }
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::Pulls { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::CombinedStatus { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
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
        | Request::AddMember { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::CreateUser { .. }
        | Request::DeleteBranch { .. } => unreachable!("start encoder dispatch"),
    };
    if length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let mut out = Writer::new(length);
    let result = target(request, &mut out);
    if result.is_err() {
        return Err(Refusal::TooLong);
    }
    let target = out.finish();
    Ok(Encoded { method: Method::Get, target, body: None })
}
#[expect(clippy::too_many_lines, reason = "exhaustive request selection names every dialect request")]
fn target(request: &Request, out: &mut Writer) -> Result<(), skein_lib::Overflow> {
    out.put(b"/api/v1/")?;
    match request {
        Request::RepoSearch { page } => search_target(out, *page),
        Request::UserRepos { page } => {
            out.put(b"user/repos")?;
            let mut query = false;
            if let Some(n) = page.page {
                out.put(b"?page=")?;
                out.put(Decimal::of(u64::from(n)).as_bytes())?;
                query = true;
            }
            if let Some(n) = page.limit {
                out.put(if query { b"&limit=" } else { b"?limit=" })?;
                out.put(Decimal::of(u64::from(n)).as_bytes())?;
            }
            Ok(())
        }
        Request::User => out.put(b"user"),
        Request::ApiSettings => out.put(b"settings/api"),
        Request::RepositoryById { id } => {
            out.put(b"repositories/")?;
            out.put(Decimal::of(*id).as_bytes())
        }
        Request::Repository { at } => {
            out.put(b"repos/")?;
            path::segment(&at.owner, out)?;
            out.put(b"/")?;
            path::segment(&at.name, out)
        }
        Request::UserSearch { uid, limit } => {
            out.put(b"users/search?uid=")?;
            out.put(Decimal::of(*uid).as_bytes())?;
            if let Some(limit) = limit {
                out.put(b"&limit=")?;
                out.put(Decimal::of(u64::from(*limit)).as_bytes())?;
            }
            Ok(())
        }
        Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Artifacts { .. }
        | Request::ArtifactZip { .. }
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::Pulls { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
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
        | Request::AddMember { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::CreateUser { .. }
        | Request::DeleteBranch { .. }
        | Request::CombinedStatus { .. } => unreachable!("start encoder dispatch"),
    }
}
#[derive(Debug)]
pub(super) struct Start {
    kind: Kind,
    user: User,
    repo: Repository,
    settings: ApiSettings,
    ok: bool,
}
impl Start {
    pub(super) fn new(kind: Kind) -> Self {
        Self {
            kind,
            user: document::user(),
            repo: document::repository(),
            settings: ApiSettings { max_response_items: 0, default_paging_num: 0 },
            ok: false,
        }
    }
    pub(super) fn step(&mut self, movement: Move, parent: Option<Kind>) -> Step {
        match movement {
            Move::Demand(demand) => Step::Demand(demand),
            Move::Scalar { kind, key, value } => match self.scalar(kind, key, value) {
                Ok(()) => Step::Demand(tokenizer::Request::Next),
                Err(error) => Step::Malformed(error),
            },
            Move::End(Kind::User) => match parent {
                Some(Kind::Repository) => {
                    self.repo.owner = core::mem::replace(&mut self.user, document::user());
                    Step::Demand(tokenizer::Request::Next)
                }
                Some(Kind::Users) => {
                    let user = core::mem::replace(&mut self.user, document::user());
                    Step::Row(Row::User(user), tokenizer::Request::Next)
                }
                None
                | Some(
                    Kind::Setup(_)
                    | Kind::User
                    | Kind::Reachable(_)
                    | Kind::Error
                    | Kind::Write(_)
                    | Kind::Settings
                    | Kind::Search
                    | Kind::Review(_)
                    | Kind::Delivery(_)
                    | Kind::Branch(_)
                    | Kind::File(_)
                    | Kind::Action(_)
                    | Kind::Item(_),
                ) => Step::Demand(tokenizer::Request::Next),
            },
            Move::End(
                Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_),
            ) => Step::Demand(tokenizer::Request::Next),
            Move::Done => {
                let document = match self.kind {
                    Kind::User => Document::User(core::mem::replace(&mut self.user, document::user())),
                    Kind::Settings => Document::ApiSettings(self.settings),
                    Kind::Repository => {
                        Document::Repository(core::mem::replace(&mut self.repo, document::repository()))
                    }
                    Kind::Search => Document::Search { ok: self.ok },
                    Kind::Users => Document::Listed { skipped: 0 },
                    Kind::Setup(_)
                    | Kind::Reachable(_)
                    | Kind::Error
                    | Kind::Write(_)
                    | Kind::Review(_)
                    | Kind::Delivery(_)
                    | Kind::Branch(_)
                    | Kind::File(_)
                    | Kind::Action(_)
                    | Kind::Item(_) => {
                        unreachable!("start decoder selection")
                    }
                };
                Step::Done(document)
            }
        }
    }
    fn scalar(&mut self, kind: Kind, key: &'static [u8], value: Value) -> Result<(), Malformed> {
        match value {
            Value::Input(_) => unreachable!("dispatch input table"),
            Value::Number(number) => match kind {
                Kind::User => self.user.id = number,
                Kind::Repository => self.repo.id = number,
                Kind::Settings => {
                    let Ok(n) = u32::try_from(number) else {
                        return Err(Malformed::Range { field: key });
                    };
                    match key {
                        b"max_response_items" => self.settings.max_response_items = n,
                        b"default_paging_num" => self.settings.default_paging_num = n,
                        _ => unreachable!("settings table"),
                    }
                }
                Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_) => {
                    unreachable!("number table")
                }
            },
            Value::Text(text) => match kind {
                Kind::User => self.user.login = text,
                Kind::Repository => match key {
                    b"name" => self.repo.name = text,
                    b"default_branch" => self.repo.default_branch = text,
                    b"html_url" => self.repo.html_url = text,
                    b"object_format_name" => {
                        self.repo.object_format = match &*text {
                            b"sha1" => ObjectFormat::Sha1,
                            b"sha256" => ObjectFormat::Sha256,
                            _ => return Err(Malformed::Value { field: key }),
                        }
                    }
                    b"default_merge_style" => {
                        self.repo.default_merge_style =
                            document::style(&text).ok_or(Malformed::Value { field: key })?;
                    }
                    _ => unreachable!("repository string table"),
                },
                Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_) => {
                    unreachable!("string table")
                }
            },
            Value::Boolean(value) => match kind {
                Kind::Search => self.ok = value,
                Kind::Repository => match key {
                    b"allow_merge_commits" => self.repo.allow_merge_commits = value,
                    b"allow_squash_merge" => self.repo.allow_squash_merge = value,
                    b"allow_rebase" => self.repo.allow_rebase = value,
                    _ => unreachable!("repository bool table"),
                },
                Kind::User
                | Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Setup(_)
                | Kind::Users
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_) => {
                    unreachable!("boolean table")
                }
            },
            Value::Body(_) | Value::Time(_) | Value::Id(_) => unreachable!("start scalar table"),
        }
        Ok(())
    }
}

fn search_target(out: &mut Writer, page: crate::forgejo::request::Page) -> Result<(), skein_lib::Overflow> {
    out.put(b"repos/search?sort=id&order=asc")?;
    if let Some(n) = page.page {
        out.put(b"&page=")?;
        out.put(Decimal::of(u64::from(n)).as_bytes())?;
    }
    if let Some(n) = page.limit {
        out.put(b"&limit=")?;
        out.put(Decimal::of(u64::from(n)).as_bytes())?;
    }
    Ok(())
}
