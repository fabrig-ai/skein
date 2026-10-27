use super::{Route, Unrouted};
use crate::forgejo::{
    id,
    limits::Limits,
    path::{self, Pair},
    request::{IssueKind, Method, Page, Repo, Request, Sort, StateFilter},
    time,
};
use alloc::boxed::Box;
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let Ok(target) = path::read(bytes, limits) else {
        return Err(Unrouted::Malformed);
    };
    let [api, v1, repos, owner, name, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if &**api != b"api" || &**v1 != b"v1" || &**repos != b"repos" {
        return Err(Unrouted::NotFound);
    }
    let at = Repo { owner: owner.clone(), name: name.clone() };
    let query = &target.query;
    let request = match tail {
        [issues] if &**issues == b"issues" => {
            let state = match query_value(query, b"state") {
                None => None,
                Some(text) => Some(match &*text {
                    b"open" => StateFilter::Open,
                    b"closed" => StateFilter::Closed,
                    b"all" => StateFilter::All,
                    _ => return Err(Unrouted::Malformed),
                }),
            };
            let kind = match query_value(query, b"type") {
                None => None,
                Some(text) => Some(match &*text {
                    b"issues" => IssueKind::Issues,
                    b"pulls" => IssueKind::Pulls,
                    _ => return Err(Unrouted::Malformed),
                }),
            };
            let since = match query_value(query, b"since") {
                None => None,
                Some(text) => Some(time::read(&text).ok_or(Unrouted::Malformed)?),
            };
            let sort = sorting(query);
            Request::Issues { at, state, kind, since, sort, page: page(query)? }
        }
        [issues, number] if &**issues == b"issues" => Request::Issue { at, number: number_value(number)? },
        [issues, number, comments] if &**issues == b"issues" && &**comments == b"comments" => {
            Request::Comments { at, number: number_value(number)? }
        }
        [pulls, number] if &**pulls == b"pulls" => Request::Pull { at, number: number_value(number)? },
        [pulls, base, head] if &**pulls == b"pulls" => Request::PullFor { at, base: base.clone(), head: head.clone() },
        [commits, commit, status] if &**commits == b"commits" && &**status == b"status" => {
            Request::CombinedStatus { at, commit: id::read(commit).ok_or(Unrouted::Malformed)?, page: page(query)? }
        }
        _ => return Err(Unrouted::NotFound),
    };
    match method {
        Method::Get => Ok(Route::Ready(request)),
        Method::Post | Method::Put | Method::Patch | Method::Delete => Err(Unrouted::Method),
    }
}
fn sorting(query: &[Pair]) -> Option<Sort> {
    let text = query_value(query, b"sort")?;
    Some(if &*text == b"leastupdate" { Sort::LeastUpdate } else { Sort::Other })
}
pub(super) fn query_value(pairs: &[Pair], key: &[u8]) -> Option<Box<[u8]>> {
    for pair in pairs {
        if pair.name.as_ref() == key {
            return Some(pair.value.clone());
        }
    }
    None
}
pub(super) fn number_value(text: &[u8]) -> Result<u64, Unrouted> {
    id::unsigned(text).ok_or(Unrouted::Malformed)
}
pub(super) fn page(query: &[Pair]) -> Result<Page, Unrouted> {
    let mut page = Page { page: None, limit: None };
    for key in [b"page".as_slice(), b"limit".as_slice()] {
        if let Some(value) = query_value(query, key) {
            let Ok(n) = u32::try_from(number_value(&value)?) else {
                return Err(Unrouted::Malformed);
            };
            if key == b"page" {
                page.page = Some(n);
            } else {
                page.limit = Some(n);
            }
        }
    }
    Ok(page)
}

use super::Answer;
use crate::forgejo::document::{Body, Comment, Issue, IssueState, PullBranch, StatusState};
use skein_json::writer::{self, Encoder};
fn state(state: IssueState) -> &'static [u8] {
    match state {
        IssueState::Open => b"open",
        IssueState::Closed => b"closed",
    }
}
fn status(state: StatusState) -> &'static [u8] {
    match state {
        StatusState::Pending => b"pending",
        StatusState::Success => b"success",
        StatusState::Error => b"error",
        StatusState::Failure => b"failure",
        StatusState::Warning => b"warning",
        StatusState::Skipped => b"skipped",
    }
}
fn body(json: &mut Encoder, body: &Body) -> Result<(), writer::Refusal> {
    match body {
        Body::Kept(text) => json.string(text),
        Body::Long(_) => return Err(writer::Refusal::TooLong),
    }
    Ok(())
}
fn issue(json: &mut Encoder, value: &Issue) -> Result<(), writer::Refusal> {
    json.object_start();
    json.key(b"number");
    json.unsigned(value.number);
    json.key(b"pull_request");
    if value.pull {
        json.object_start();
        json.object_end();
    } else {
        json.null();
    }
    json.key(b"state");
    json.string(state(value.state));
    json.key(b"user");
    super::start::user(json, &value.user);
    json.key(b"labels");
    json.array_start();
    for label in &value.labels {
        json.object_start();
        json.key(b"name");
        json.string(label);
        json.object_end();
    }
    json.array_end();
    json.key(b"updated_at");
    json.string(&time::write(value.updated_at));
    json.key(b"title");
    json.string(&value.title);
    json.key(b"body");
    body(json, &value.body)?;
    json.object_end();
    Ok(())
}
pub(super) fn comment(json: &mut Encoder, value: &Comment) -> Result<(), writer::Refusal> {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    json.key(b"user");
    super::start::user(json, &value.user);
    for (key, time) in [(b"created_at".as_slice(), value.created_at), (b"updated_at", value.updated_at)] {
        if let Some(time) = time {
            json.key(key);
            json.string(&time::write(time));
        }
    }
    json.key(b"body");
    body(json, &value.body)?;
    json.object_end();
    Ok(())
}
fn branch(json: &mut Encoder, value: &PullBranch) {
    json.object_start();
    json.key(b"repo_id");
    json.unsigned(value.repo_id);
    json.key(b"ref");
    json.string(&value.name);
    json.key(b"sha");
    match value.sha {
        Some(id) => json.string(&id::write(&id)),
        None => json.string(b""),
    }
    json.object_end();
}
pub(super) fn write(json: &mut Encoder, answer: &Answer) -> Result<(), writer::Refusal> {
    match answer {
        Answer::Issue(value) => issue(json, value)?,
        Answer::Issues(values) => {
            json.array_start();
            for value in values {
                issue(json, value)?;
            }
            json.array_end();
        }
        Answer::Comments(values) => {
            json.array_start();
            for value in values {
                comment(json, value)?;
            }
            json.array_end();
        }
        Answer::Pull(value) => pull(json, value),
        Answer::CombinedStatus { state, sha, statuses } => {
            json.object_start();
            json.key(b"state");
            json.string(match state.state {
                Some(value) => status(value),
                None => b"",
            });
            json.key(b"sha");
            match sha {
                Some(id) => json.string(&id::write(id)),
                None => json.string(b""),
            }
            json.key(b"total_count");
            json.unsigned(state.total_count);
            json.key(b"statuses");
            if statuses.is_empty() {
                json.null();
            } else {
                json.array_start();
                for value in statuses {
                    commit_status(json, value);
                }
                json.array_end();
            }
            json.object_end();
        }
        Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Files(_)
        | Answer::Compare { .. }
        | Answer::Tree { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Job { .. }
        | Answer::Jobs(_)
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Permission(_)
        | Answer::Pulls(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::Org(_)
        | Answer::CreatedFile { .. }
        | Answer::CreatedRepository(_)
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. } => {
            unreachable!("item answer dispatch")
        }
    }
    Ok(())
}

pub(super) fn pull(json: &mut Encoder, value: &crate::forgejo::document::PullRequest) {
    json.object_start();
    json.key(b"number");
    json.unsigned(value.number);
    json.key(b"state");
    json.string(state(value.state));
    json.key(b"head");
    branch(json, &value.head);
    json.key(b"base");
    branch(json, &value.base);
    json.key(b"merged");
    json.boolean(value.merged);
    for (key, id) in [(b"merge_commit_sha".as_slice(), value.merge_commit_sha), (b"merge_base", value.merge_base)] {
        json.key(key);
        match id {
            Some(id) => json.string(&id::write(&id)),
            None => json.null(),
        }
    }
    json.key(b"mergeable");
    json.boolean(value.mergeable);
    json.key(b"requested_reviewers");
    json.array_start();
    for user in &value.requested_reviewers {
        super::start::user(json, user);
    }
    json.array_end();
    json.object_end();
}

pub(super) fn commit_status(json: &mut Encoder, value: &crate::forgejo::document::CommitStatus) {
    json.object_start();
    json.key(b"creator");
    match &value.creator {
        Some(user) => super::start::user(json, user),
        None => json.null(),
    }
    json.key(b"updated_at");
    json.string(&time::write(value.updated_at));
    json.key(b"context");
    json.string(&value.context);
    json.key(b"status");
    json.string(status(value.status));
    json.key(b"description");
    json.string(&value.description);
    json.key(b"target_url");
    json.string(&value.target_url);
    json.object_end();
}
