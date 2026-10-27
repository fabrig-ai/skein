//! Sized request bodies and the write routes' methods and targets.
use super::{Encoded, Refusal, items::Target};
use crate::forgejo::{
    document::{IssueState, MergeStyle, StatusState},
    id,
    limits::Limits,
    request::{Method, Request, ReviewEvent, UpdateStyle},
};
use alloc::boxed::Box;
use skein_json::writer::{self, Encoder};
use skein_lib::Writer;
pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let mut measure = Target { length: 0, writer: None };
    let (method, has_body) = target(request, &mut measure);
    if measure.length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let mut out = Target { length: 0, writer: Some(Writer::new(measure.length)) };
    target(request, &mut out);
    let body = if has_body {
        let mut measure = Encoder::measure(&limits.writer);
        write(&mut measure, request);
        let length = match measure.measured() {
            Ok(length) => length,
            Err(writer::Refusal::Text) => return Err(Refusal::Text),
            Err(writer::Refusal::TooLong | writer::Refusal::TooDeep | writer::Refusal::Number) => {
                return Err(Refusal::TooLong);
            }
        };
        let mut out = Encoder::write(length, &limits.writer);
        write(&mut out, request);
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
fn target(request: &Request, out: &mut Target) -> (Method, bool) {
    match request {
        Request::Dispatch { at, workflow, .. } => {
            out.repo(at);
            out.put(b"/actions/workflows/");
            out.segment(workflow);
            out.put(b"/dispatches");
        }
        Request::Rerun { at, job } => {
            out.numbered(at, b"/actions/jobs/", *job);
            out.put(b"/rerun");
            return (Method::Post, false);
        }
        Request::CreateIssue { at, .. } => {
            out.repo(at);
            out.put(b"/issues");
        }
        Request::OpenPull { at, .. } => {
            out.repo(at);
            out.put(b"/pulls");
        }
        Request::PostComment { at, number, .. } => {
            out.numbered(at, b"/issues/", *number);
            out.put(b"/comments");
        }
        Request::PostReview { at, number, .. } => {
            out.numbered(at, b"/pulls/", *number);
            out.put(b"/reviews");
        }
        Request::EditIssue { at, number, .. } => {
            out.numbered(at, b"/issues/", *number);
            return (Method::Patch, true);
        }
        Request::RequestReviewers { at, number, .. } => {
            out.numbered(at, b"/pulls/", *number);
            out.put(b"/requested_reviewers");
        }
        Request::RemoveReviewers { at, number, .. } => {
            out.numbered(at, b"/pulls/", *number);
            out.put(b"/requested_reviewers");
            return (Method::Delete, true);
        }
        Request::Merge { at, number, .. } => {
            out.numbered(at, b"/pulls/", *number);
            out.put(b"/merge");
        }
        Request::Update { at, number, style } => {
            update_target(out, at, *number, *style);
            return (Method::Post, false);
        }
        Request::PostStatus { at, commit, .. } => {
            out.repo(at);
            out.put(b"/statuses/");
            out.put(&id::write(commit));
        }
        Request::CreateBranch { at, .. } => {
            out.repo(at);
            out.put(b"/branches");
        }
        Request::DeleteBranch { at, branch } => {
            out.repo(at);
            out.put(b"/branches/");
            out.segment(branch);
            return (Method::Delete, false);
        }
        Request::Tokens { .. }
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
        | Request::JobLog { .. } => unreachable!("write target selection"),
    }
    (Method::Post, true)
}
pub(crate) fn update(value: UpdateStyle) -> &'static [u8] {
    match value {
        UpdateStyle::Merge => b"merge",
        UpdateStyle::Rebase => b"rebase",
    }
}
pub(crate) fn event(value: ReviewEvent) -> &'static [u8] {
    match value {
        ReviewEvent::Approved => b"APPROVED",
        ReviewEvent::RequestChanges => b"REQUEST_CHANGES",
        ReviewEvent::Comment => b"COMMENT",
    }
}
pub(crate) fn style(value: MergeStyle) -> &'static [u8] {
    match value {
        MergeStyle::Merge => b"merge",
        MergeStyle::Rebase => b"rebase",
        MergeStyle::RebaseMerge => b"rebase-merge",
        MergeStyle::Squash => b"squash",
        MergeStyle::FastForwardOnly => b"fast-forward-only",
        MergeStyle::ManuallyMerged => b"manually-merged",
    }
}
pub(crate) fn state(value: IssueState) -> &'static [u8] {
    match value {
        IssueState::Open => b"open",
        IssueState::Closed => b"closed",
    }
}
pub(crate) fn status(value: StatusState) -> &'static [u8] {
    match value {
        StatusState::Pending => b"pending",
        StatusState::Success => b"success",
        StatusState::Failure => b"failure",
        StatusState::Error => b"error",
        StatusState::Warning => b"warning",
        StatusState::Skipped => b"skipped",
    }
}
fn text(json: &mut Encoder, key: &[u8], value: &[u8]) {
    json.key(key);
    json.string(value);
}
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive request selection lists every setup variant beside its owning group"
)]
fn write(json: &mut Encoder, request: &Request) {
    json.object_start();
    match request {
        Request::Dispatch { branch, inputs, .. } => {
            text(json, b"ref", branch);
            json.key(b"inputs");
            json.object_start();
            for input in inputs {
                text(json, &input.name, &input.value);
            }
            json.object_end();
        }
        Request::Rerun { .. } => unreachable!("rerun has no body"),
        Request::CreateIssue { title, body, .. } => {
            text(json, b"title", title);
            text(json, b"body", body);
        }
        Request::OpenPull { head, base, title, body, .. } => {
            text(json, b"head", head);
            text(json, b"base", base);
            text(json, b"title", title);
            text(json, b"body", body);
        }
        Request::PostComment { body, .. } => text(json, b"body", body),
        Request::PostReview { event: value, body, .. } => {
            text(json, b"event", event(*value));
            text(json, b"body", body);
        }
        Request::EditIssue { title, body, state: value, .. } => {
            if let Some(title) = title {
                text(json, b"title", title);
            }
            if let Some(body) = body {
                text(json, b"body", body);
            }
            if let Some(value) = value {
                text(json, b"state", state(*value));
            }
        }
        Request::RequestReviewers { reviewers, .. } | Request::RemoveReviewers { reviewers, .. } => {
            json.key(b"reviewers");
            json.array_start();
            for login in reviewers {
                json.string(login);
            }
            json.array_end();
        }
        Request::Merge { style: value, head, .. } => {
            text(json, b"Do", style(*value));
            text(json, b"head_commit_id", &id::write(head));
        }
        Request::PostStatus { state: value, context, description, link, .. } => {
            if !description.is_empty() {
                text(json, b"description", description);
            }
            if !link.is_empty() {
                text(json, b"target_url", link);
            }
            text(json, b"state", status(*value));
            text(json, b"context", context);
        }
        Request::CreateBranch { branch, from, .. } => {
            text(json, b"new_branch_name", branch);
            text(json, b"old_ref_name", &id::write(from));
        }
        Request::Tokens { .. }
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
        | Request::Update { .. }
        | Request::DeleteBranch { .. } => unreachable!("write body selection"),
    }
    json.object_end();
}

use crate::forgejo::{
    decode::{
        Step,
        walk::{Move, Value},
    },
    document::{Body, Document, ErrorBody},
};
#[derive(Debug)]
pub(super) struct Error {
    message: Body,
}
impl Error {
    pub(super) fn new() -> Self {
        Self { message: Body::Kept(Box::from([])) }
    }
    pub(super) fn step(&mut self, movement: Move) -> Step {
        match movement {
            Move::Demand(demand) => Step::Demand(demand),
            Move::Scalar { value, .. } => {
                match value {
                    Value::Input(_) => unreachable!("dispatch input table"),
                    Value::Body(body) => self.message = body,
                    Value::Number(_) | Value::Text(_) | Value::Boolean(_) | Value::Time(_) | Value::Id(_) => {
                        unreachable!("error field table")
                    }
                }
                Step::Demand(skein_json::tokenizer::Request::Next)
            }
            Move::End(_) => Step::Demand(skein_json::tokenizer::Request::Next),
            Move::Done => Step::Done(Document::Error(ErrorBody {
                message: core::mem::replace(&mut self.message, Body::Kept(Box::from([]))),
            })),
        }
    }
}

fn update_target(out: &mut Target, at: &crate::forgejo::request::Repo, number: u64, style: Option<UpdateStyle>) {
    out.numbered(at, b"/pulls/", number);
    out.put(b"/update");
    if let Some(style) = style {
        out.put(b"?style=");
        out.put(update(style));
    }
}
