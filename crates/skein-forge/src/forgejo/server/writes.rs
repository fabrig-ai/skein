//! Routes and streaming request bodies shared with the fake forge.
use super::{
    BodyStep, Route, Unrouted,
    items::{number_value, query_value},
};
use crate::forgejo::{
    decode::{
        Malformed,
        walk::{Field, Kind, Move, Read, Value, Walk},
    },
    document::{IssueState, MergeStyle, StatusState},
    id,
    limits::Limits,
    path,
    request::{Method, Repo, Request, ReviewEvent, UpdateStyle},
};
use alloc::boxed::Box;
use skein_json::tokenizer;
use skein_lib::List;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WriteKind {
    Dispatch,
    Inputs,
    Issue,
    Labels,
    LabelIds,
    LabelId,
    NewLabel,
    NewUser,
    NewToken,
    Scopes,
    Scope,
    NewHook,
    Collaborator,
    HookConfig,
    NewOrg,
    NewRepository,
    NewFile,
    Author,
    Committer,
    NewTeam,
    Pull,
    Comment,
    Review,
    Edit,
    Reviewers,
    Logins,
    Login,
    Merge,
    Status,
    Branch,
}
const fn field(key: &'static [u8], read: Read, required: bool) -> Field {
    Field { key, read, required }
}
#[expect(clippy::too_many_lines, reason = "each request body kind declares its complete field table")]
pub(crate) fn fields(kind: WriteKind) -> &'static [Field] {
    match kind {
        WriteKind::Dispatch => {
            const { &[field(b"ref", Read::Text, true), field(b"inputs", Read::Object(Kind::Write(WriteKind::Inputs)), true)] }
        }
        WriteKind::NewToken => {
            const { &[field(b"name", Read::Text, true), field(b"scopes", Read::Array(Kind::Write(WriteKind::Scopes)), true)] }
        }
        WriteKind::NewHook => super::hooks::fields(),
        WriteKind::Collaborator => const { &[field(b"permission", Read::Text, true)] },
        WriteKind::HookConfig => super::hooks::config(),
        WriteKind::NewFile => {
            const {
                &[
                    field(b"content", Read::Body, true),
                    field(b"branch", Read::Text, true),
                    field(b"message", Read::Text, true),
                    field(b"author", Read::Object(Kind::Write(WriteKind::Author)), true),
                    field(b"committer", Read::Object(Kind::Write(WriteKind::Committer)), true),
                ]
            }
        }
        WriteKind::Author | WriteKind::Committer => {
            const { &[field(b"name", Read::Text, true), field(b"email", Read::Text, true)] }
        }
        WriteKind::NewRepository => {
            const {
                &[
                    field(b"name", Read::Text, true),
                    field(b"private", Read::Boolean, true),
                    field(b"auto_init", Read::Boolean, true),
                    field(b"default_branch", Read::Text, true),
                ]
            }
        }
        WriteKind::NewOrg => const { &[field(b"username", Read::Text, true), field(b"visibility", Read::Text, true)] },
        WriteKind::NewTeam => {
            const {
                &[
                    field(b"name", Read::Text, true),
                    field(b"permission", Read::Text, true),
                    field(b"includes_all_repositories", Read::Boolean, true),
                    field(
                        b"units_map",
                        Read::Object(Kind::Setup(crate::forgejo::client::setup::SetupKind::Units)),
                        true,
                    ),
                ]
            }
        }
        WriteKind::NewUser => {
            const {
                &[
                    field(b"username", Read::Text, true),
                    field(b"email", Read::Text, true),
                    field(b"password", Read::Text, true),
                    field(b"full_name", Read::Text, true),
                    field(b"must_change_password", Read::Boolean, true),
                ]
            }
        }
        WriteKind::Issue => {
            const {
                &[
                    field(b"title", Read::Text, true),
                    field(b"body", Read::Body, true),
                    field(b"labels", Read::Array(Kind::Write(WriteKind::LabelIds)), false),
                ]
            }
        }
        WriteKind::Labels => const { &[field(b"labels", Read::Array(Kind::Write(WriteKind::LabelIds)), true)] },
        WriteKind::NewLabel => const { &[field(b"name", Read::Text, true), field(b"color", Read::Text, true)] },
        WriteKind::Pull => {
            const {
                &[
                    field(b"head", Read::Text, true),
                    field(b"base", Read::Text, true),
                    field(b"title", Read::Text, true),
                    field(b"body", Read::Body, true),
                ]
            }
        }
        WriteKind::Comment => const { &[field(b"body", Read::Body, true)] },
        WriteKind::Review => const { &[field(b"event", Read::Text, true), field(b"body", Read::Body, true)] },
        WriteKind::Edit => {
            const {
                &[
                    field(b"title", Read::Text, false),
                    field(b"body", Read::Body, false),
                    field(b"state", Read::Text, false),
                ]
            }
        }
        WriteKind::Reviewers => const { &[field(b"reviewers", Read::Array(Kind::Write(WriteKind::Logins)), true)] },
        WriteKind::Merge => {
            const { &[field(b"Do", Read::Text, true), field(b"head_commit_id", Read::Id { empty: false }, true)] }
        }
        WriteKind::Status => {
            const {
                &[
                    field(b"state", Read::Text, true),
                    field(b"context", Read::Text, true),
                    field(b"description", Read::Text, false),
                    field(b"target_url", Read::Text, false),
                ]
            }
        }
        WriteKind::Branch => {
            const { &[field(b"new_branch_name", Read::Text, true), field(b"old_ref_name", Read::Id { empty: false }, true)] }
        }
        WriteKind::LabelIds
        | WriteKind::LabelId
        | WriteKind::Inputs
        | WriteKind::Scopes
        | WriteKind::Scope
        | WriteKind::Logins
        | WriteKind::Login => &[],
    }
}
pub(crate) fn element(kind: WriteKind) -> Option<Kind> {
    match kind {
        WriteKind::LabelIds => Some(Kind::Write(WriteKind::LabelId)),
        WriteKind::Scopes => Some(Kind::Write(WriteKind::Scope)),
        WriteKind::Logins => Some(Kind::Write(WriteKind::Login)),
        WriteKind::LabelId
        | WriteKind::Labels
        | WriteKind::NewLabel
        | WriteKind::Dispatch
        | WriteKind::Inputs
        | WriteKind::Collaborator
        | WriteKind::HookConfig
        | WriteKind::NewHook
        | WriteKind::NewToken
        | WriteKind::Scope
        | WriteKind::NewFile
        | WriteKind::Author
        | WriteKind::Committer
        | WriteKind::NewRepository
        | WriteKind::NewOrg
        | WriteKind::NewTeam
        | WriteKind::NewUser
        | WriteKind::Issue
        | WriteKind::Pull
        | WriteKind::Comment
        | WriteKind::Review
        | WriteKind::Edit
        | WriteKind::Reviewers
        | WriteKind::Login
        | WriteKind::Merge
        | WriteKind::Status
        | WriteKind::Branch => None,
    }
}
fn zero() -> id::ObjectId {
    id::ObjectId { format: id::ObjectFormat::Sha1, bytes: [0; 32] }
}
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let target = path::read(bytes, limits).ok().ok_or(Unrouted::Malformed)?;
    let [api, v1, repos, owner, name, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if &**api != b"api" || &**v1 != b"v1" || &**repos != b"repos" {
        return Err(Unrouted::NotFound);
    }
    let at = Repo { owner: owner.clone(), name: name.clone() };
    if method == Method::Get {
        if let [labels] = tail
            && labels.as_ref() == b"labels"
        {
            return Ok(Route::Ready(Request::Labels { at, page: super::items::page(&target.query)? }));
        }
        return Err(Unrouted::NotFound);
    }
    route_tail(method, at, tail, &target.query, limits)
}
#[expect(clippy::too_many_lines, reason = "each mutation route declares its method and body kind")]
fn route_tail(
    method: Method,
    at: Repo,
    tail: &[Box<[u8]>],
    query: &[path::Pair],
    limits: &Limits,
) -> Result<Route, Unrouted> {
    let (required, kind, request) = match tail {
        [labels] if labels.as_ref() == b"labels" => {
            (Method::Post, WriteKind::NewLabel, Request::CreateLabel { at, name: Box::new([]), color: [0; 3] })
        }
        [issues, number, labels] if issues.as_ref() == b"issues" && labels.as_ref() == b"labels" => (
            Method::Put,
            WriteKind::Labels,
            Request::SetLabels { at, number: number_value(number)?, labels: Box::new([]) },
        ),
        [actions, workflows, workflow, dispatches]
            if &**actions == b"actions" && &**workflows == b"workflows" && &**dispatches == b"dispatches" =>
        {
            (
                Method::Post,
                WriteKind::Dispatch,
                Request::Dispatch { at, workflow: workflow.clone(), branch: Box::new([]), inputs: Box::new([]) },
            )
        }
        [actions, jobs, job, rerun] if &**actions == b"actions" && &**jobs == b"jobs" && &**rerun == b"rerun" => {
            if method != Method::Post {
                return Err(Unrouted::Method);
            }
            return Ok(Route::Ready(Request::Rerun { at, job: number_value(job)? }));
        }
        [contents, parts @ ..] if &**contents == b"contents" && !parts.is_empty() => {
            (Method::Post, WriteKind::NewFile, new_file(at, parts))
        }
        [issues] if &**issues == b"issues" => (
            Method::Post,
            WriteKind::Issue,
            Request::CreateIssue { at, title: Box::from([]), body: Box::from([]), labels: Box::new([]) },
        ),
        [pulls] if &**pulls == b"pulls" => (
            Method::Post,
            WriteKind::Pull,
            Request::OpenPull {
                at,
                head: Box::from([]),
                base: Box::from([]),
                title: Box::from([]),
                body: Box::from([]),
            },
        ),
        [issues, number] if &**issues == b"issues" => (
            Method::Patch,
            WriteKind::Edit,
            Request::EditIssue { at, number: number_value(number)?, title: None, body: None, state: None },
        ),
        [issues, number, comments] if &**issues == b"issues" && &**comments == b"comments" => (
            Method::Post,
            WriteKind::Comment,
            Request::PostComment { at, number: number_value(number)?, body: Box::from([]) },
        ),
        [pulls, number, reviews] if &**pulls == b"pulls" && &**reviews == b"reviews" => (
            Method::Post,
            WriteKind::Review,
            Request::PostReview { at, number: number_value(number)?, event: ReviewEvent::Comment, body: Box::from([]) },
        ),
        [pulls, number, reviewers] if &**pulls == b"pulls" && &**reviewers == b"requested_reviewers" => {
            let number = number_value(number)?;
            match method {
                Method::Post => (
                    Method::Post,
                    WriteKind::Reviewers,
                    Request::RequestReviewers { at, number, reviewers: Box::from([]) },
                ),
                Method::Delete => (
                    Method::Delete,
                    WriteKind::Reviewers,
                    Request::RemoveReviewers { at, number, reviewers: Box::from([]) },
                ),
                Method::Get | Method::Put | Method::Patch => return Err(Unrouted::Method),
            }
        }
        [pulls, number, merge] if &**pulls == b"pulls" && &**merge == b"merge" => (
            Method::Post,
            WriteKind::Merge,
            Request::Merge { at, number: number_value(number)?, style: MergeStyle::Merge, head: zero() },
        ),
        [pulls, number, update] if &**pulls == b"pulls" && &**update == b"update" => {
            if method != Method::Post {
                return Err(Unrouted::Method);
            }
            let style = match query_value(query, b"style") {
                None => None,
                Some(text) => Some(match &*text {
                    b"merge" => UpdateStyle::Merge,
                    b"rebase" => UpdateStyle::Rebase,
                    _ => return Err(Unrouted::Malformed),
                }),
            };
            return Ok(Route::Ready(Request::Update { at, number: number_value(number)?, style }));
        }
        [statuses, commit] if &**statuses == b"statuses" => (
            Method::Post,
            WriteKind::Status,
            Request::PostStatus {
                at,
                commit: id::read(commit).ok_or(Unrouted::Malformed)?,
                state: StatusState::Pending,
                context: Box::from([]),
                description: Box::new([]),
                link: Box::new([]),
            },
        ),
        [branches] if &**branches == b"branches" => {
            (Method::Post, WriteKind::Branch, Request::CreateBranch { at, branch: Box::from([]), from: zero() })
        }
        [branches, branch] if &**branches == b"branches" => {
            if method != Method::Delete {
                return Err(Unrouted::Method);
            }
            return Ok(Route::Ready(Request::DeleteBranch { at, branch: branch.clone() }));
        }
        _ => return Err(Unrouted::NotFound),
    };
    if method != required {
        return Err(Unrouted::Method);
    }
    Ok(Route::Body(BodyDecoder {
        walk: Walk::new(Kind::Write(kind), limits),
        request,
        texts: List::with_capacity(limits.inner),
        numbers: List::with_capacity(if kind == WriteKind::Issue || kind == WriteKind::Labels {
            limits.inner
        } else {
            0
        }),
        inputs: List::with_capacity(if kind == WriteKind::Dispatch {
            u32::try_from(crate::api::INPUTS).expect("input bound")
        } else {
            0
        }),
        terminal: false,
        committer: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
    }))
}
#[derive(Debug)]
pub struct BodyDecoder {
    pub(super) walk: Walk,
    pub(super) request: Request,
    pub(super) texts: List<Box<[u8]>>,
    pub(super) numbers: List<u64>,
    pub(super) inputs: List<crate::api::Input>,
    pub(super) terminal: bool,
    pub(super) committer: crate::forgejo::request::Author,
}
#[must_use]
pub fn body_first(_decoder: &BodyDecoder) -> tokenizer::Request {
    tokenizer::Request::Next
}
pub fn body(decoder: &mut BodyDecoder, limits: &Limits, event: &tokenizer::Event) -> BodyStep {
    if decoder.terminal {
        return BodyStep::Malformed(Malformed::Shape { field: b"" });
    }
    let movement = match decoder.walk.step(event, limits) {
        Ok(movement) => movement,
        Err(error) => {
            decoder.terminal = true;
            return BodyStep::Malformed(error);
        }
    };
    let result = match movement {
        Move::Demand(demand) => {
            return BodyStep::Demand(
                if decoder.walk.parent() == Some(Kind::Write(WriteKind::Logins))
                    || decoder.walk.parent() == Some(Kind::Write(WriteKind::Scopes))
                    || decoder.walk.parent() == Some(Kind::Setup(crate::forgejo::client::setup::SetupKind::Events))
                {
                    tokenizer::Request::Text(limits.string_bytes)
                } else {
                    demand
                },
            );
        }
        Move::Scalar { kind, key, value } => decoder.scalar(kind, key, value, limits),
        Move::End(kind) => {
            decoder.end(kind, limits);
            Ok(())
        }
        Move::Done => {
            decoder.terminal = true;
            if let Request::CreateFile { author, .. } = &decoder.request
                && *author != decoder.committer
            {
                return BodyStep::Malformed(Malformed::Value { field: b"committer" });
            }
            if let Request::CreateIssue { labels, .. } | Request::SetLabels { labels, .. } = &mut decoder.request {
                *labels = core::mem::replace(&mut decoder.numbers, List::with_capacity(0)).into_boxed();
            }
            return BodyStep::Decoded(core::mem::replace(&mut decoder.request, Request::User));
        }
    };
    match result {
        Ok(()) => BodyStep::Demand(
            if decoder.walk.parent() == Some(Kind::Write(WriteKind::Logins))
                || decoder.walk.parent() == Some(Kind::Write(WriteKind::Scopes))
                || decoder.walk.parent() == Some(Kind::Setup(crate::forgejo::client::setup::SetupKind::Events))
            {
                tokenizer::Request::Text(limits.string_bytes)
            } else {
                tokenizer::Request::Next
            },
        ),
        Err(error) => {
            decoder.terminal = true;
            BodyStep::Malformed(error)
        }
    }
}
impl BodyDecoder {
    fn end(&mut self, kind: Kind, limits: &Limits) {
        if kind == Kind::Write(WriteKind::Inputs) {
            let Request::Dispatch { inputs, .. } = &mut self.request else {
                unreachable!("dispatch input table");
            };
            *inputs = core::mem::replace(&mut self.inputs, List::with_capacity(0)).into_boxed();
        }
        if kind == Kind::Write(WriteKind::Logins)
            || kind == Kind::Write(WriteKind::Scopes)
            || kind == Kind::Setup(crate::forgejo::client::setup::SetupKind::Events)
        {
            let logins = core::mem::replace(&mut self.texts, List::with_capacity(limits.inner)).into_boxed();
            match &mut self.request {
                Request::CreateOrgHook { hook, .. }
                | Request::EditOrgHook { hook, .. }
                | Request::CreateRepoHook { hook, .. }
                | Request::EditRepoHook { hook, .. } => hook.events = logins,
                Request::CreateToken { scopes, .. } => *scopes = logins,
                Request::RequestReviewers { reviewers, .. } | Request::RemoveReviewers { reviewers, .. } => {
                    *reviewers = logins;
                }
                Request::Labels { .. }
                | Request::SetLabels { .. }
                | Request::CreateLabel { .. }
                | Request::CreateIssue { .. }
                | Request::OpenPull { .. }
                | Request::PostComment { .. }
                | Request::PostReview { .. }
                | Request::EditIssue { .. }
                | Request::Dispatch { .. }
                | Request::Rerun { .. }
                | Request::Merge { .. }
                | Request::Update { .. }
                | Request::PostStatus { .. }
                | Request::CreateBranch { .. }
                | Request::Tokens { .. }
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
                | Request::DeleteOrgHook { .. }
                | Request::AddMember { .. }
                | Request::AddCollaborator { .. }
                | Request::RepoHooks { .. }
                | Request::DeleteRepoHook { .. }
                | Request::CreateUser { .. }
                | Request::DeleteBranch { .. }
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
                | Request::Pulls { .. }
                | Request::CombinedStatus { .. }
                | Request::Reviews { .. }
                | Request::ReviewComments { .. }
                | Request::Branch { .. }
                | Request::Branches { .. }
                | Request::Protection { .. }
                | Request::Protections { .. }
                | Request::Collaborators { .. }
                | Request::Permission { .. }
                | Request::PullFiles { .. }
                | Request::Compare { .. }
                | Request::Contents { .. }
                | Request::Raw { .. }
                | Request::Tree { .. }
                | Request::WorkflowRuns { .. }
                | Request::Runs { .. }
                | Request::Run { .. }
                | Request::Job { .. }
                | Request::Jobs { .. }
                | Request::JobLog { .. }
                | Request::User
                | Request::ApiSettings => unreachable!("reviewers body table"),
            }
        }
    }

    fn flag(&mut self, key: &'static [u8], value: bool) -> Result<(), Malformed> {
        if let Request::CreateOrgHook { hook, .. }
        | Request::EditOrgHook { hook, .. }
        | Request::CreateRepoHook { hook, .. }
        | Request::EditRepoHook { hook, .. } = &mut self.request
        {
            hook.active = value;
            return Ok(());
        }
        if let Request::CreateTeam { spec, .. } | Request::EditTeam { spec, .. } = &mut self.request {
            spec.includes_all_repositories = value;
            return Ok(());
        }
        if let Request::CreateRepository { private, .. } = &mut self.request {
            if key == b"private" {
                *private = value;
            } else if !value {
                return Err(Malformed::Value { field: key });
            }
            return Ok(());
        }
        if let Request::CreateUser { must_change_password, .. } = &mut self.request {
            *must_change_password = value;
            return Ok(());
        }
        Err(Malformed::Shape { field: key })
    }
    fn scalar(&mut self, kind: Kind, key: &'static [u8], value: Value, limits: &Limits) -> Result<(), Malformed> {
        let text = match value {
            Value::Input(input) => {
                if input.name.is_empty() {
                    return Err(Malformed::Value { field: b"inputs" });
                }
                for held in self.inputs.as_slice() {
                    if held.name == input.name {
                        return Err(Malformed::Value { field: b"inputs" });
                    }
                }
                self.inputs.push(input).or(Err(Malformed::Bound { field: b"inputs" }))?;
                return Ok(());
            }
            Value::Text(text) | Value::Body(crate::forgejo::document::Body::Kept(text)) => Some(text),
            Value::Body(crate::forgejo::document::Body::Long(_)) => return Err(Malformed::TooLong { field: key }),
            Value::Id(id) => {
                self.set_id(id.expect("required id"));
                return Ok(());
            }
            Value::Boolean(value) => return self.flag(key, value),
            Value::Number(number) => {
                if kind != Kind::Write(WriteKind::LabelId) {
                    return Err(Malformed::Shape { field: key });
                }
                self.numbers.push(number).or(Err(Malformed::Bound { field: key }))?;
                return Ok(());
            }
            Value::Time(_) => return Err(Malformed::Shape { field: key }),
        };
        let text = text.expect("text field");
        if kind == Kind::Write(WriteKind::Login)
            || kind == Kind::Write(WriteKind::Scope)
            || kind == Kind::Setup(crate::forgejo::client::setup::SetupKind::Event)
        {
            self.texts.push(text).ok().ok_or(Malformed::Bound { field: key })?;
            return Ok(());
        }
        if let Request::CreateFile { branch, message, content, author, .. } = &mut self.request {
            if kind == Kind::Write(WriteKind::Author) || kind == Kind::Write(WriteKind::Committer) {
                let signature = if kind == Kind::Write(WriteKind::Author) { author } else { &mut self.committer };
                if key == b"name" {
                    signature.name = text;
                } else {
                    signature.email = text;
                }
            } else if key == b"branch" {
                *branch = text;
            } else if key == b"message" {
                *message = text;
            } else {
                *content = skein_lib::base64::decode(skein_lib::base64::Alphabet::Standard, &text, limits.body_bytes)
                    .or(Err(Malformed::Value { field: key }))?;
            }
            return Ok(());
        }
        self.text(key, text)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive request selection lists every setup variant beside its owning group"
    )]
    fn text(&mut self, key: &'static [u8], text: Box<[u8]>) -> Result<(), Malformed> {
        if let Request::CreateOrgHook { hook, .. }
        | Request::EditOrgHook { hook, .. }
        | Request::CreateRepoHook { hook, .. }
        | Request::EditRepoHook { hook, .. } = &mut self.request
        {
            return super::hooks::text(hook, key, text);
        }
        match &mut self.request {
            Request::CreateLabel { name, color, .. } => {
                if key == b"name" {
                    *name = text;
                } else {
                    *color = read_color(&text).ok_or(Malformed::Value { field: key })?;
                }
            }
            Request::Labels { .. } | Request::SetLabels { .. } => unreachable!("numeric label fields"),
            Request::AddCollaborator { permission, .. } => {
                *permission = crate::forgejo::request::Grant::read(&text).ok_or(Malformed::Value { field: key })?;
            }
            Request::CreateToken { name, .. } => *name = text,
            Request::CreateTeam { spec, .. } | Request::EditTeam { spec, .. } => {
                crate::forgejo::client::orgs::team_text(spec, key, text)?;
            }
            Request::CreateOrg { name, .. } => org_name(name, key, text)?,
            Request::CreateRepository { name, .. } => {
                if key == b"name" {
                    *name = text;
                } else if text.as_ref() != b"main" {
                    return Err(Malformed::Value { field: key });
                }
            }
            Request::CreateUser { login, email, full_name, password, .. } => match key {
                b"username" => *login = text,
                b"email" => *email = text,
                b"full_name" => *full_name = text,
                b"password" => *password = skein_lib::Secret::new(text),
                _ => unreachable!("new user fields"),
            },
            Request::CreateIssue { title, body, .. } => {
                if key == b"title" {
                    *title = text;
                } else {
                    *body = text;
                }
            }
            Request::OpenPull { head, base, title, body, .. } => match key {
                b"head" => *head = text,
                b"base" => *base = text,
                b"title" => *title = text,
                b"body" => *body = text,
                _ => unreachable!("pull fields"),
            },
            Request::PostComment { body, .. } => *body = text,
            Request::PostReview { event, body, .. } => review_text(event, body, key, text)?,
            Request::EditIssue { title, body, state, .. } => match key {
                b"title" => *title = Some(text),
                b"body" => *body = Some(text),
                b"state" => *state = Some(issue_state(&text).ok_or(Malformed::Value { field: key })?),
                _ => unreachable!("edit fields"),
            },
            Request::Merge { style, .. } => *style = merge_style(&text).ok_or(Malformed::Value { field: key })?,
            Request::PostStatus { state, context, description, link, .. } => {
                if key == b"description" {
                    *description = text;
                } else if key == b"target_url" {
                    *link = text;
                } else if key == b"context" {
                    *context = text;
                } else {
                    *state =
                        crate::forgejo::client::items::status_state(&text).ok_or(Malformed::Value { field: key })?;
                }
            }
            Request::Dispatch { branch, .. } | Request::CreateBranch { branch, .. } => *branch = text,
            Request::Rerun { .. }
            | Request::RequestReviewers { .. }
            | Request::RemoveReviewers { .. }
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
            | Request::Pulls { .. }
            | Request::CombinedStatus { .. }
            | Request::Reviews { .. }
            | Request::ReviewComments { .. }
            | Request::Branch { .. }
            | Request::Branches { .. }
            | Request::Protection { .. }
            | Request::Protections { .. }
            | Request::Collaborators { .. }
            | Request::Permission { .. }
            | Request::PullFiles { .. }
            | Request::Compare { .. }
            | Request::Contents { .. }
            | Request::Raw { .. }
            | Request::Tree { .. }
            | Request::WorkflowRuns { .. }
            | Request::Runs { .. }
            | Request::Run { .. }
            | Request::Job { .. }
            | Request::Jobs { .. }
            | Request::JobLog { .. }
            | Request::Update { .. }
            | Request::Tokens { .. }
            | Request::DeleteToken { .. }
            | Request::UserOf { .. }
            | Request::Org { .. }
            | Request::Teams { .. }
            | Request::TeamMembers { .. }
            | Request::CreateOrgHook { .. }
            | Request::EditOrgHook { .. }
            | Request::OrgHooks { .. }
            | Request::DeleteOrgHook { .. }
            | Request::AddMember { .. }
            | Request::RepoHooks { .. }
            | Request::CreateRepoHook { .. }
            | Request::EditRepoHook { .. }
            | Request::DeleteRepoHook { .. }
            | Request::CreateFile { .. }
            | Request::DeleteBranch { .. } => unreachable!("body text selection"),
        }
        Ok(())
    }
    fn set_id(&mut self, id: id::ObjectId) {
        match &mut self.request {
            Request::Merge { head, .. } => *head = id,
            Request::CreateBranch { from, .. } => *from = id,
            Request::Dispatch { .. }
            | Request::Rerun { .. }
            | Request::Labels { .. }
            | Request::SetLabels { .. }
            | Request::CreateLabel { .. }
            | Request::CreateIssue { .. }
            | Request::OpenPull { .. }
            | Request::PostComment { .. }
            | Request::PostReview { .. }
            | Request::EditIssue { .. }
            | Request::PostStatus { .. }
            | Request::RequestReviewers { .. }
            | Request::RemoveReviewers { .. }
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
            | Request::Pulls { .. }
            | Request::CombinedStatus { .. }
            | Request::Reviews { .. }
            | Request::ReviewComments { .. }
            | Request::Branch { .. }
            | Request::Branches { .. }
            | Request::Protection { .. }
            | Request::Protections { .. }
            | Request::Collaborators { .. }
            | Request::Permission { .. }
            | Request::PullFiles { .. }
            | Request::Compare { .. }
            | Request::Contents { .. }
            | Request::Raw { .. }
            | Request::Tree { .. }
            | Request::WorkflowRuns { .. }
            | Request::Runs { .. }
            | Request::Run { .. }
            | Request::Job { .. }
            | Request::Jobs { .. }
            | Request::JobLog { .. }
            | Request::Update { .. }
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
            | Request::DeleteBranch { .. } => unreachable!("body id selection"),
        }
    }
}
fn issue_state(text: &[u8]) -> Option<IssueState> {
    match text {
        b"open" => Some(IssueState::Open),
        b"closed" => Some(IssueState::Closed),
        _ => None,
    }
}
fn merge_style(text: &[u8]) -> Option<MergeStyle> {
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

fn org_name(name: &mut Box<[u8]>, key: &'static [u8], text: Box<[u8]>) -> Result<(), Malformed> {
    if key == b"username" {
        *name = text;
    } else if text.as_ref() != b"private" {
        return Err(Malformed::Value { field: key });
    }
    Ok(())
}

fn review_text(
    event: &mut ReviewEvent,
    body: &mut Box<[u8]>,
    key: &'static [u8],
    text: Box<[u8]>,
) -> Result<(), Malformed> {
    if key == b"body" {
        *body = text;
    } else {
        *event = match &*text {
            b"APPROVED" => ReviewEvent::Approved,
            b"REQUEST_CHANGES" => ReviewEvent::RequestChanges,
            b"COMMENT" => ReviewEvent::Comment,
            _ => return Err(Malformed::Value { field: key }),
        };
    }

    Ok(())
}

fn file_path(parts: &[Box<[u8]>]) -> Box<[u8]> {
    let mut length = parts.len().saturating_sub(1);
    for part in parts {
        length = length.saturating_add(part.len());
    }
    let mut out = skein_lib::Writer::new(length);
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            out.put(b"/").expect("path separators");
        }
        out.put(part).expect("path segments");
    }
    out.finish()
}

fn new_file(at: Repo, parts: &[Box<[u8]>]) -> Request {
    Request::CreateFile {
        at,
        path: file_path(parts),
        branch: Box::new([]),
        content: Box::new([]),
        message: Box::new([]),
        author: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
    }
}

fn read_color(bytes: &[u8]) -> Option<[u8; 3]> {
    let bytes = bytes.strip_prefix(b"#").unwrap_or(bytes);
    if bytes.len() != 6 {
        return None;
    }
    let mut color = [0; 3];
    for (byte, digits) in color.iter_mut().zip(bytes.chunks_exact(2)) {
        let mut value = 0u8;
        for digit in digits {
            let nibble = match digit {
                b'0'..=b'9' => digit.checked_sub(b'0')?,
                b'a'..=b'f' => digit.checked_sub(b'a')?.checked_add(10)?,
                b'A'..=b'F' => digit.checked_sub(b'A')?.checked_add(10)?,
                _ => return None,
            };
            value = value.checked_mul(16)?.checked_add(nibble)?;
        }
        *byte = value;
    }
    Some(color)
}
