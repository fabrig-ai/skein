use super::{Encoded, Refusal};
use crate::forgejo::{
    id,
    limits::Limits,
    path,
    request::{IssueKind, Method, Page, Repo, Request, Sort, StateFilter},
    time,
};
use alloc::boxed::Box;
use skein_lib::{Decimal, List, Writer};
#[derive(Debug)]
pub(super) struct Target {
    pub(super) length: usize,
    pub(super) writer: Option<Writer>,
}
impl Target {
    pub(super) fn put(&mut self, bytes: &[u8]) {
        self.length = self.length.saturating_add(bytes.len());
        if let Some(out) = &mut self.writer {
            out.put(bytes).expect("measured target");
        }
    }
    pub(super) fn segment(&mut self, name: &[u8]) {
        self.length = self.length.saturating_add(path::segment_length(name));
        if let Some(out) = &mut self.writer {
            path::segment(name, out).expect("measured segment");
        }
    }
    pub(super) fn file(&mut self, path: &[u8]) {
        for byte in path {
            self.length = self.length.saturating_add(if *byte == b'/' {
                1
            } else {
                path::segment_length(core::slice::from_ref(byte))
            });
        }
        if let Some(out) = &mut self.writer {
            path::file(path, out).expect("measured file path");
        }
    }
    pub(super) fn repo(&mut self, at: &Repo) {
        self.put(b"/api/v1/repos/");
        self.segment(&at.owner);
        self.put(b"/");
        self.segment(&at.name);
    }
    pub(super) fn number(&mut self, number: u64) {
        self.put(Decimal::of(number).as_bytes());
    }
    pub(super) fn numbered(&mut self, at: &Repo, path: &[u8], number: u64) {
        self.repo(at);
        self.put(path);
        self.number(number);
    }
    pub(super) fn page(&mut self, page: Page, query: bool) {
        let mut has_query = query;
        if let Some(n) = page.page {
            self.put(if has_query { b"&page=" } else { b"?page=" });
            self.number(u64::from(n));
            has_query = true;
        }
        if let Some(n) = page.limit {
            self.put(if has_query { b"&limit=" } else { b"?limit=" });
            self.number(u64::from(n));
        }
    }
}
pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let mut measure = Target { length: 0, writer: None };
    target(request, &mut measure);
    if measure.length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let mut write = Target { length: 0, writer: Some(Writer::new(measure.length)) };
    target(request, &mut write);
    Ok(Encoded { method: Method::Get, target: write.writer.expect("writing pass").finish(), body: None })
}
fn target(request: &Request, out: &mut Target) {
    match request {
        Request::Issues { at, state, kind, since, sort, page } => {
            out.repo(at);
            out.put(b"/issues");
            issue_query(out, *state, *kind, *since, *sort, *page);
        }
        Request::Issue { at, number } => out.numbered(at, b"/issues/", *number),
        Request::Comments { at, number } => {
            out.numbered(at, b"/issues/", *number);
            out.put(b"/comments");
        }
        Request::Pull { at, number } => out.numbered(at, b"/pulls/", *number),
        Request::PullFor { at, base, head } => pull_for(out, at, base, head),
        Request::CombinedStatus { at, commit, page } => {
            out.repo(at);
            out.put(b"/commits/");
            out.put(&id::write(commit));
            out.put(b"/status");
            out.page(*page, false);
        }
        Request::Reviews { .. }
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
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
        | Request::Repository { .. }
        | Request::UserRepos { .. }
        | Request::RepoSearch { .. }
        | Request::UserSearch { .. }
        | Request::Labels { .. }
        | Request::SetLabels { .. }
        | Request::CreateLabel { .. }
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
        | Request::DeleteBranch { .. } => unreachable!("item encoder dispatch"),
    }
}

use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Item, Kind, Move, Value, Walk},
    },
    document::{
        self, Body, CombinedStatus, Comment, CommitStatus, Document, Issue, IssueState, PullBranch, PullRequest, Row,
        StatusState, User,
    },
};
use skein_lib::Wall;
fn issue() -> Issue {
    Issue {
        number: 0,
        pull: false,
        state: IssueState::Open,
        user: document::user(),
        labels: Box::from([]),
        updated_at: Wall::EPOCH,
        title: Box::from([]),
        body: Body::Kept(Box::from([])),
    }
}
fn comment() -> Comment {
    Comment { id: 0, user: document::user(), created_at: None, updated_at: None, body: Body::Kept(Box::from([])) }
}
fn branch() -> PullBranch {
    PullBranch { repo_id: 0, name: Box::from([]), sha: None }
}
fn pull() -> PullRequest {
    PullRequest {
        number: 0,
        state: IssueState::Open,
        head: branch(),
        base: branch(),
        merged: false,
        merge_commit_sha: None,
        merge_base: None,
        mergeable: false,
        requested_reviewers: Box::from([]),
    }
}
fn status() -> CommitStatus {
    CommitStatus {
        creator: None,
        updated_at: Wall::EPOCH,
        context: Box::from([]),
        status: StatusState::Pending,
        description: Box::from([]),
        target_url: Box::from([]),
    }
}
pub(crate) fn state(text: &[u8]) -> Option<IssueState> {
    match text {
        b"open" => Some(IssueState::Open),
        b"closed" => Some(IssueState::Closed),
        _ => None,
    }
}
pub(crate) fn status_state(text: &[u8]) -> Option<StatusState> {
    match text {
        b"pending" => Some(StatusState::Pending),
        b"success" => Some(StatusState::Success),
        b"error" => Some(StatusState::Error),
        b"failure" => Some(StatusState::Failure),
        b"warning" => Some(StatusState::Warning),
        b"skipped" => Some(StatusState::Skipped),
        _ => None,
    }
}
#[derive(Debug)]
pub(super) struct Items {
    root: Item,
    user: User,
    issue: Issue,
    comment: Comment,
    pull: PullRequest,
    status: CommitStatus,
    combined: CombinedStatus,
    label_id: u64,
    label_name: Box<[u8]>,
    labels: List<Box<[u8]>>,
    reviewers: List<User>,
    inner: u32,
    after: u64,
}
impl Items {
    pub(super) fn keep_after(&mut self, after: u64) {
        self.after = after;
    }
    pub(super) fn new(root: Item, limits: &Limits) -> Self {
        Self {
            root,
            user: document::user(),
            issue: issue(),
            comment: comment(),
            pull: pull(),
            status: status(),
            combined: CombinedStatus { state: None, total_count: 0 },
            label_id: 0,
            label_name: Box::new([]),
            labels: List::with_capacity(limits.inner),
            reviewers: List::with_capacity(limits.inner),
            inner: limits.inner,
            after: 0,
        }
    }
    pub(super) fn step(&mut self, movement: Move, walk: &mut Walk) -> Step {
        let result = match movement {
            Move::Demand(demand) => return Step::Demand(demand),
            Move::Scalar { kind, key, value } => {
                let result = self.scalar(kind, key, value);
                if kind == Kind::Item(Item::Comment) && key == b"id" && self.comment.id <= self.after {
                    walk.skip_comment();
                }
                result
            }
            Move::End(Kind::Item(Item::Label)) => {
                if self.root == Item::Labels {
                    let name = core::mem::replace(&mut self.label_name, Box::new([]));
                    let id = core::mem::replace(&mut self.label_id, 0);
                    return Step::Row(Row::Label { id, name }, skein_json::tokenizer::Request::Next);
                }
                Ok(())
            }
            Move::End(Kind::User) => self.finish_user(walk),
            Move::End(Kind::Item(Item::Presence)) => {
                self.issue.pull = true;
                Ok(())
            }
            Move::End(Kind::Item(Item::Issue)) => {
                let labels = core::mem::replace(&mut self.labels, List::with_capacity(self.inner));
                self.issue.labels = labels.into_boxed();
                if self.root == Item::Issues {
                    let value = core::mem::replace(&mut self.issue, issue());
                    return Step::Row(Row::Issue(value), skein_json::tokenizer::Request::Next);
                }
                Ok(())
            }
            Move::End(Kind::Item(Item::Comment)) => {
                let value = core::mem::replace(&mut self.comment, comment());
                if self.root == Item::Comment {
                    self.comment = value;
                } else if value.id > self.after {
                    return Step::Row(Row::Comment(value), skein_json::tokenizer::Request::Next);
                }
                Ok(())
            }
            Move::End(Kind::Item(Item::Status)) => {
                let value = core::mem::replace(&mut self.status, status());
                if self.root == Item::Status {
                    self.status = value;
                } else {
                    return Step::Row(Row::Status(value), skein_json::tokenizer::Request::Next);
                }
                Ok(())
            }
            Move::End(Kind::Item(Item::Pull)) => {
                self.finish_pull();
                if self.root == Item::Pulls {
                    return self.pull_row();
                }
                Ok(())
            }
            Move::End(Kind::Item(
                Item::Pulls
                | Item::Issues
                | Item::Comments
                | Item::Head
                | Item::Base
                | Item::Statuses
                | Item::Labels
                | Item::Combined
                | Item::Reviewers,
            )) => Ok(()),
            Move::End(
                Kind::File(_)
                | Kind::Action(_)
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users,
            ) => {
                unreachable!("item root selection")
            }
            Move::Done => return Step::Done(self.document(walk.skipped())),
        };
        match result {
            Ok(()) => Step::Demand(skein_json::tokenizer::Request::Next),
            Err(error) => Step::Malformed(error),
        }
    }
    fn finish_user(&mut self, walk: &Walk) -> Result<(), Malformed> {
        let user = core::mem::replace(&mut self.user, document::user());
        match walk.parent() {
            Some(Kind::Item(Item::Issue)) => self.issue.user = user,
            Some(Kind::Item(Item::Comment)) => self.comment.user = user,
            Some(Kind::Item(Item::Status)) => self.status.creator = Some(user),
            Some(Kind::Item(Item::Reviewers)) => {
                if self.reviewers.push(user).is_err() {
                    return Err(Malformed::Bound { field: b"requested_reviewers" });
                }
            }
            None
            | Some(
                Kind::File(_)
                | Kind::Action(_)
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::User
                | Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::Item(
                    Item::Pulls
                    | Item::Issues
                    | Item::Comments
                    | Item::Pull
                    | Item::Head
                    | Item::Base
                    | Item::Label
                    | Item::Labels
                    | Item::Statuses
                    | Item::Combined
                    | Item::Presence,
                ),
            ) => unreachable!("item user table"),
        }
        Ok(())
    }
    fn pull_row(&mut self) -> Step {
        let value = core::mem::replace(&mut self.pull, pull());
        Step::Row(Row::Pull(value), skein_json::tokenizer::Request::Next)
    }
    fn finish_pull(&mut self) {
        let reviewers = core::mem::replace(&mut self.reviewers, List::with_capacity(self.inner));
        self.pull.requested_reviewers = reviewers.into_boxed();
    }
    fn document(&mut self, skipped: u32) -> Document {
        match self.root {
            Item::Issue => Document::Issue(core::mem::replace(&mut self.issue, issue())),
            Item::Pull => Document::Pull(core::mem::replace(&mut self.pull, pull())),
            Item::Combined => Document::CombinedStatus(self.combined),
            Item::Labels | Item::Pulls | Item::Issues | Item::Comments => Document::Listed { skipped },
            Item::Comment => Document::Comment(core::mem::replace(&mut self.comment, comment())),
            Item::Status => Document::Status(core::mem::replace(&mut self.status, status())),
            Item::Head | Item::Base | Item::Statuses | Item::Label | Item::Reviewers | Item::Presence => {
                unreachable!("item root selection")
            }
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive dialect dispatch includes each label request and answer variant"
    )]
    fn scalar(&mut self, kind: Kind, key: &'static [u8], value: Value) -> Result<(), Malformed> {
        match value {
            Value::Input(_) => unreachable!("dispatch input table"),
            Value::Number(number) => match key {
                b"id" => {
                    if kind == Kind::Item(Item::Label) {
                        self.label_id = number;
                    } else if kind == Kind::User {
                        self.user.id = number;
                    } else {
                        self.comment.id = number;
                    }
                }
                b"number" => {
                    if kind == Kind::Item(Item::Issue) {
                        self.issue.number = number;
                    } else {
                        self.pull.number = number;
                    }
                }
                b"total_count" => self.combined.total_count = number,
                b"repo_id" => {
                    if kind == Kind::Item(Item::Head) {
                        self.pull.head.repo_id = number;
                    } else {
                        self.pull.base.repo_id = number;
                    }
                }
                _ => unreachable!("item numeric table"),
            },
            Value::Text(text) => match key {
                b"login" => self.user.login = text,
                b"title" => self.issue.title = text,
                b"state" => {
                    if kind == Kind::Item(Item::Issue) {
                        self.issue.state = state(&text).ok_or(Malformed::Value { field: key })?;
                    } else if kind == Kind::Item(Item::Pull) {
                        self.pull.state = state(&text).ok_or(Malformed::Value { field: key })?;
                    } else {
                        self.combined.state = if text.is_empty() {
                            None
                        } else {
                            Some(status_state(&text).ok_or(Malformed::Value { field: key })?)
                        };
                    }
                }
                b"ref" => {
                    if kind == Kind::Item(Item::Head) {
                        self.pull.head.name = text;
                    } else {
                        self.pull.base.name = text;
                    }
                }
                b"name" => {
                    if self.root == Item::Labels {
                        self.label_name = text;
                    } else if self.labels.push(text).is_err() {
                        return Err(Malformed::Bound { field: b"labels" });
                    }
                }
                b"context" => self.status.context = text,
                b"description" => self.status.description = text,
                b"target_url" => self.status.target_url = text,
                b"status" => self.status.status = status_state(&text).ok_or(Malformed::Value { field: key })?,
                _ => unreachable!("item text table"),
            },
            Value::Body(body) => {
                if kind == Kind::Item(Item::Issue) {
                    self.issue.body = body;
                } else {
                    self.comment.body = body;
                }
            }
            Value::Time(time) => match key {
                b"created_at" => self.comment.created_at = Some(time),
                b"updated_at" => {
                    if kind == Kind::Item(Item::Issue) {
                        self.issue.updated_at = time;
                    } else if kind == Kind::Item(Item::Comment) {
                        self.comment.updated_at = Some(time);
                    } else {
                        self.status.updated_at = time;
                    }
                }
                _ => unreachable!("item time table"),
            },
            Value::Id(id) => match key {
                b"sha" => {
                    if kind == Kind::Item(Item::Head) {
                        self.pull.head.sha = id;
                    } else {
                        self.pull.base.sha = id;
                    }
                }
                b"merge_commit_sha" => self.pull.merge_commit_sha = id,
                b"merge_base" => self.pull.merge_base = id,
                _ => unreachable!("item id table"),
            },
            Value::Boolean(value) => match key {
                b"merged" => self.pull.merged = value,
                b"mergeable" => self.pull.mergeable = value,
                _ => unreachable!("item boolean table"),
            },
        }
        Ok(())
    }
}

fn pull_for(out: &mut Target, at: &Repo, base: &[u8], head: &[u8]) {
    out.repo(at);
    out.put(b"/pulls/");
    out.segment(base);
    out.put(b"/");
    out.segment(head);
}

fn issue_query(
    out: &mut Target,
    state: Option<StateFilter>,
    kind: Option<IssueKind>,
    since: Option<Wall>,
    sort: Option<Sort>,
    page: Page,
) {
    let mut query = false;
    if let Some(state) = state {
        out.put(b"?state=");
        out.put(match state {
            StateFilter::Open => b"open",
            StateFilter::Closed => b"closed",
            StateFilter::All => b"all",
        });
        query = true;
    }
    if let Some(kind) = kind {
        out.put(if query { b"&type=" } else { b"?type=" });
        out.put(match kind {
            IssueKind::Issues => b"issues",
            IssueKind::Pulls => b"pulls",
        });
        query = true;
    }
    if let Some(since) = since {
        out.put(if query { b"&since=" } else { b"?since=" });
        out.segment(&time::write(since));
        query = true;
    }
    if let Some(sort) = sort {
        out.put(if query { b"&sort=" } else { b"?sort=" });
        out.put(match sort {
            Sort::LeastUpdate => b"leastupdate",
            Sort::Other => b"latest",
        });
        query = true;
    }
    out.page(page, query);
}
