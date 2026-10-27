use super::{Encoded, Refusal, items::Target, setup::SetupKind};
use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Field, Kind, Move, Read, Value},
    },
    document::{self, Document, Org, Row, Team, TeamSpec},
    limits::Limits,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_json::{
    tokenizer,
    writer::{self, Encoder},
};
use skein_lib::Writer;
const fn field(key: &'static [u8], read: Read, required: bool) -> Field {
    Field { key, read, required }
}
pub(crate) fn fields(kind: SetupKind) -> &'static [Field] {
    match kind {
        SetupKind::Org => const { &[field(b"id", Read::Number, true), field(b"name", Read::Text, true)] },
        SetupKind::Team => {
            const {
                &[
                    field(b"id", Read::Number, true),
                    field(b"name", Read::Text, true),
                    field(b"permission", Read::Text, true),
                    field(b"includes_all_repositories", Read::Boolean, true),
                    field(b"units_map", Read::Object(Kind::Setup(SetupKind::Units)), false),
                ]
            }
        }
        SetupKind::Units => {
            const {
                &[
                    field(b"repo.code", Read::Text, false),
                    field(b"repo.issues", Read::Text, false),
                    field(b"repo.pulls", Read::Text, false),
                    field(b"repo.actions", Read::Text, false),
                ]
            }
        }
        SetupKind::Teams => &[],
        SetupKind::CreatedFile
        | SetupKind::FileCommit
        | SetupKind::CreatedRepository
        | SetupKind::Hook
        | SetupKind::Hooks
        | SetupKind::Config
        | SetupKind::Events
        | SetupKind::Event
        | SetupKind::Account
        | SetupKind::Token
        | SetupKind::Tokens
        | SetupKind::Scopes
        | SetupKind::Scope => {
            unreachable!("organization tables")
        }
    }
}
pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let mut measure = Target { length: 0, writer: None };
    let method = target(request, &mut measure);
    if measure.length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let mut out = Target { length: 0, writer: Some(Writer::new(measure.length)) };
    target(request, &mut out);
    let has_body = if let Request::AddCollaborator { .. } = request {
        true
    } else {
        method == Method::Post || method == Method::Patch
    };
    let body = if has_body {
        let mut measure = Encoder::measure(&limits.writer);
        body(&mut measure, request);
        let length = match measure.measured() {
            Ok(length) => length,
            Err(writer::Refusal::Text) => return Err(Refusal::Text),
            Err(writer::Refusal::TooLong | writer::Refusal::TooDeep | writer::Refusal::Number) => {
                return Err(Refusal::TooLong);
            }
        };
        let mut out = Encoder::write(length, &limits.writer);
        body(&mut out, request);
        Some(out.finish())
    } else {
        None
    };
    Ok(Encoded { method, target: out.writer.expect("writing pass").finish(), body })
}
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive request selection lists every setup variant beside its owning group"
)]
fn target(request: &Request, out: &mut Target) -> Method {
    match request {
        Request::AddCollaborator { at, login, .. } => {
            out.repo(at);
            out.put(b"/collaborators/");
            out.segment(login);
            Method::Put
        }
        Request::RepoHooks { at, page } => {
            super::hooks::repo_target(out, at, Some(*page), None);
            Method::Get
        }
        Request::CreateRepoHook { at, .. } => {
            super::hooks::repo_target(out, at, None, None);
            Method::Post
        }
        Request::EditRepoHook { at, id, .. } => {
            super::hooks::repo_target(out, at, None, Some(*id));
            Method::Patch
        }
        Request::DeleteRepoHook { at, id } => {
            super::hooks::repo_target(out, at, None, Some(*id));
            Method::Delete
        }
        Request::Org { name } => {
            out.put(b"/api/v1/orgs/");
            out.segment(name);
            Method::Get
        }
        Request::CreateRepository { org, .. } => {
            org_target(out, org, b"/repos");
            Method::Post
        }
        Request::CreateOrg { person, .. } => {
            out.put(b"/api/v1/admin/users/");
            out.segment(person);
            out.put(b"/orgs");
            Method::Post
        }
        Request::Teams { org, page } => {
            org_target(out, org, b"/teams");
            out.page(*page, false);
            Method::Get
        }
        Request::CreateTeam { org, .. } => {
            org_target(out, org, b"/teams");
            Method::Post
        }
        Request::EditTeam { team, .. } => {
            out.put(b"/api/v1/teams/");
            out.number(*team);
            Method::Patch
        }
        Request::TeamMembers { team, page } => {
            member_target(out, *team, None);
            out.page(*page, false);
            Method::Get
        }
        Request::AddMember { team, login } => {
            member_target(out, *team, Some(login));
            Method::Put
        }
        Request::OrgHooks { org, page } => {
            super::hooks::target(out, org, Some(*page), None);
            Method::Get
        }
        Request::CreateOrgHook { org, .. } => {
            super::hooks::target(out, org, None, None);
            Method::Post
        }
        Request::EditOrgHook { org, id, .. } => {
            super::hooks::target(out, org, None, Some(*id));
            Method::Patch
        }
        Request::DeleteOrgHook { org, id } => {
            super::hooks::target(out, org, None, Some(*id));
            Method::Delete
        }
        Request::CreateFile { .. }
        | Request::User
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
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
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
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::DeleteBranch { .. }
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::CreateUser { .. } => unreachable!("organization request selection"),
    }
}
fn body(json: &mut Encoder, request: &Request) {
    match request {
        Request::AddCollaborator { permission, .. } => {
            json.object_start();
            json.key(b"permission");
            json.string(permission.text());
            json.object_end();
        }
        Request::CreateOrgHook { hook, .. } | Request::CreateRepoHook { hook, .. } => {
            super::hooks::specification(json, hook, true);
        }
        Request::EditOrgHook { hook, .. } | Request::EditRepoHook { hook, .. } => {
            super::hooks::specification(json, hook, false);
        }
        Request::RepoHooks { .. }
        | Request::DeleteRepoHook { .. }
        | Request::OrgHooks { .. }
        | Request::DeleteOrgHook { .. } => unreachable!("hook has no body"),
        Request::CreateRepository { name, private, .. } => {
            json.object_start();
            json.key(b"name");
            json.string(name);
            json.key(b"private");
            json.boolean(*private);
            json.key(b"auto_init");
            json.boolean(true);
            json.key(b"default_branch");
            json.string(b"main");
            json.object_end();
        }
        Request::CreateOrg { name, .. } => {
            json.object_start();
            json.key(b"username");
            json.string(name);
            json.key(b"visibility");
            json.string(b"private");
            json.object_end();
        }
        Request::CreateTeam { spec, .. } | Request::EditTeam { spec, .. } => {
            json.object_start();
            specification(json, spec, false);
            json.object_end();
        }
        Request::CreateFile { .. }
        | Request::Org { .. }
        | Request::Teams { .. }
        | Request::TeamMembers { .. }
        | Request::AddMember { .. }
        | Request::User
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
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
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
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::DeleteBranch { .. }
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::CreateUser { .. } => unreachable!("organization body selection"),
    }
}
pub(crate) fn specification(json: &mut Encoder, spec: &TeamSpec, omit_none: bool) {
    json.key(b"name");
    json.string(&spec.name);
    json.key(b"permission");
    json.string(document::access_text(spec.permission));
    json.key(b"includes_all_repositories");
    json.boolean(spec.includes_all_repositories);
    json.key(b"units_map");
    json.object_start();
    for (key, access) in [
        (b"repo.code".as_slice(), spec.units.code),
        (b"repo.issues", spec.units.issues),
        (b"repo.pulls", spec.units.pulls),
        (b"repo.actions", spec.units.actions),
    ] {
        if omit_none && access == document::Access::None {
            continue;
        }
        json.key(key);
        json.string(document::access_text(access));
    }
    json.object_end();
}
pub(crate) fn team_text(spec: &mut TeamSpec, key: &'static [u8], text: Box<[u8]>) -> Result<(), Malformed> {
    if key == b"name" {
        spec.name = text;
        return Ok(());
    }
    let access = document::access(&text).ok_or(Malformed::Value { field: key })?;
    match key {
        b"permission" => spec.permission = access,
        b"repo.code" => spec.units.code = access,
        b"repo.issues" => spec.units.issues = access,
        b"repo.pulls" => spec.units.pulls = access,
        b"repo.actions" => spec.units.actions = access,
        _ => unreachable!("team text fields"),
    }
    Ok(())
}
#[derive(Debug)]
pub(super) struct Orgs {
    kind: SetupKind,
    org: Org,
    team: Team,
}
impl Orgs {
    pub(super) fn new(kind: SetupKind) -> Self {
        Self { kind, org: Org { id: 0, name: Box::from([]) }, team: document::team() }
    }
    pub(super) fn step(&mut self, movement: Move) -> Step {
        match movement {
            Move::Demand(demand) => return Step::Demand(demand),
            Move::Scalar { key, value, .. } => match value {
                Value::Input(_) => unreachable!("dispatch input table"),
                Value::Number(id) => {
                    if self.kind == SetupKind::Org {
                        self.org.id = id;
                    } else {
                        self.team.id = id;
                    }
                }
                Value::Boolean(value) => self.team.spec.includes_all_repositories = value,
                Value::Text(text) => {
                    if self.kind == SetupKind::Org {
                        self.org.name = text;
                    } else if let Err(error) = team_text(&mut self.team.spec, key, text) {
                        return Step::Malformed(error);
                    }
                }
                Value::Body(_) | Value::Time(_) | Value::Id(_) => {
                    return Step::Malformed(Malformed::Shape { field: key });
                }
            },
            Move::End(Kind::Setup(SetupKind::Team)) if self.kind == SetupKind::Teams => {
                return Step::Row(
                    Row::Team(core::mem::replace(&mut self.team, document::team())),
                    tokenizer::Request::Next,
                );
            }
            Move::End(
                Kind::Setup(_)
                | Kind::User
                | Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Users
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_)
                | Kind::Review(_)
                | Kind::Branch(_)
                | Kind::Delivery(_),
            ) => {}
            Move::Done => {
                return Step::Done(match self.kind {
                    SetupKind::Org => {
                        Document::Org(core::mem::replace(&mut self.org, Org { id: 0, name: Box::from([]) }))
                    }
                    SetupKind::Team => Document::Team(core::mem::replace(&mut self.team, document::team())),
                    SetupKind::Teams => Document::Listed { skipped: 0 },
                    SetupKind::CreatedFile
                    | SetupKind::FileCommit
                    | SetupKind::CreatedRepository
                    | SetupKind::Hook
                    | SetupKind::Hooks
                    | SetupKind::Config
                    | SetupKind::Events
                    | SetupKind::Event
                    | SetupKind::Units
                    | SetupKind::Account
                    | SetupKind::Token
                    | SetupKind::Tokens
                    | SetupKind::Scopes
                    | SetupKind::Scope => unreachable!("root organization table"),
                });
            }
        }
        Step::Demand(tokenizer::Request::Next)
    }
}

fn member_target(out: &mut Target, team: u64, login: Option<&[u8]>) {
    out.put(b"/api/v1/teams/");
    out.number(team);
    out.put(b"/members");
    if let Some(login) = login {
        out.put(b"/");
        out.segment(login);
    }
}

fn org_target(out: &mut Target, org: &[u8], tail: &[u8]) {
    out.put(b"/api/v1/orgs/");
    out.segment(org);
    out.put(tail);
}
