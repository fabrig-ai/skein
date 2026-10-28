//! Repository routes translate HTTP requests into calls to the fake domain.
use crate::Config;
use alloc::boxed::Box;
use skein_fake_forge_domain::api::{self as a, Op, Read, Write};
use skein_forge::forgejo::{
    document as d,
    id::{ObjectFormat, ObjectId},
    request::{self as r, Request},
};
use skein_lib::{Decimal, List, Time, Wall, Writer};

/// A fake's commit count in Forgejo's SHA-1 spelling.
#[must_use]
pub fn object(commit: u64) -> ObjectId {
    let mut bytes = [0; 32];
    for (out, byte) in bytes.iter_mut().zip(skein_fake_forge_domain::object_bytes(commit)) {
        *out = byte;
    }
    ObjectId { format: ObjectFormat::Sha1, bytes }
}
/// Only the shared SHA-1 spelling names a fake commit.
#[must_use]
pub fn commit(id: &ObjectId) -> Option<u64> {
    if id.format != ObjectFormat::Sha1 {
        return None;
    }
    let mut count = 0u64;
    for byte in id.bytes.get(8..16)? {
        count = count.checked_mul(256)?.checked_add(u64::from(*byte))?;
    }
    if object(count) == *id { Some(count) } else { None }
}
#[must_use]
pub fn user(config: &Config, id: u64) -> d::User {
    for (uid, login) in &config.users {
        if *uid == id {
            return d::User { id, login: login.clone(), is_admin: false, full_name: Box::from([]) };
        }
    }
    let digits = Decimal::of(id);
    let mut login = Writer::new(digits.as_bytes().len().saturating_add(1));
    login.put(b"u").expect("sized login");
    login.put(digits.as_bytes()).expect("sized login");
    d::User { id, login: login.finish(), is_admin: false, full_name: Box::from([]) }
}
#[must_use]
pub fn page(config: &Config, page: r::Page) -> (u32, u32) {
    let size =
        if page.page.is_some() { page.limit.unwrap_or(config.default_paging_num) } else { config.default_paging_num };
    (page.page.unwrap_or(1).max(1), size.max(1).min(config.max_response_items))
}
fn full(at: &r::Repo) -> Box<[u8]> {
    let mut out = Writer::new(at.owner.len().saturating_add(at.name.len()).saturating_add(1));
    out.put(&at.owner).expect("sized path");
    out.put(b"/").expect("sized path");
    out.put(&at.name).expect("sized path");
    out.finish()
}
/// Configured path of any route that needs the domain; table routes have none.
#[must_use]
pub fn repository(config: &Config, request: &Request) -> Option<Box<[u8]>> {
    match request {
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
        | Request::UserRepos { .. }
        | Request::RepoSearch { .. }
        | Request::UserSearch { .. } => None,
        Request::RepositoryById { id } => {
            for repo in &config.repositories {
                if u64::from(repo.id) == *id {
                    return Some(repo.full.clone());
                }
            }
            None
        }
        Request::Repository { at }
        | Request::Issues { at, .. }
        | Request::Issue { at, .. }
        | Request::Comments { at, .. }
        | Request::Pull { at, .. }
        | Request::PullFor { at, .. }
        | Request::CombinedStatus { at, .. }
        | Request::Reviews { at, .. }
        | Request::ReviewComments { at, .. }
        | Request::Branch { at, .. }
        | Request::Branches { at, .. }
        | Request::Protection { at, .. }
        | Request::Protections { at }
        | Request::Collaborators { at, .. }
        | Request::Permission { at, .. }
        | Request::Pulls { at, .. }
        | Request::PullFiles { at, .. }
        | Request::Compare { at, .. }
        | Request::Contents { at, .. }
        | Request::Raw { at, .. }
        | Request::Tree { at, .. }
        | Request::WorkflowRuns { at, .. }
        | Request::Dispatch { at, .. }
        | Request::Rerun { at, .. }
        | Request::Runs { at, .. }
        | Request::Run { at, .. }
        | Request::Job { at, .. }
        | Request::Jobs { at, .. }
        | Request::JobLog { at, .. }
        | Request::Labels { at, .. }
        | Request::SetLabels { at, .. }
        | Request::CreateLabel { at, .. }
        | Request::CreateIssue { at, .. }
        | Request::OpenPull { at, .. }
        | Request::PostComment { at, .. }
        | Request::PostReview { at, .. }
        | Request::EditIssue { at, .. }
        | Request::RequestReviewers { at, .. }
        | Request::RemoveReviewers { at, .. }
        | Request::Merge { at, .. }
        | Request::Update { at, .. }
        | Request::PostStatus { at, .. }
        | Request::CreateBranch { at, .. }
        | Request::DeleteBranch { at, .. } => Some(full(at)),
    }
}
fn uid(config: &Config, login: &[u8]) -> Option<u64> {
    for (id, name) in &config.users {
        if &**name == login {
            return Some(*id);
        }
    }
    if let Some(digits) = login.strip_prefix(b"u") {
        return skein_forge::forgejo::id::unsigned(digits);
    }
    None
}
fn reviewers(config: &Config, logins: &[Box<[u8]>]) -> Option<Box<[u64]>> {
    let mut out = List::with_capacity(u32::try_from(logins.len()).ok()?);
    for login in logins {
        let user = uid(config, login)?;
        out.push(user).expect("one user per login");
    }
    Some(out.into_boxed())
}
#[must_use]
pub fn check(state: d::StatusState) -> a::Check {
    match state {
        d::StatusState::Pending => a::Check::Pending,
        d::StatusState::Success => a::Check::Passed,
        d::StatusState::Skipped => a::Check::Skipped,
        d::StatusState::Error | d::StatusState::Failure | d::StatusState::Warning => a::Check::Failed,
    }
}
/// Translates the document's fields; a bad count or user names a missing object.
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive domain request selection also names the protocol-only setup variants"
)]
pub fn op(config: &Config, request: &Request) -> Result<Op, a::Error> {
    let missing = a::Error::Missing(a::What::Commit);
    let read = match request {
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
        | Request::UserRepos { .. }
        | Request::RepoSearch { .. }
        | Request::UserSearch { .. } => return Err(a::Error::Refused),
        Request::Tree { object: id, page: p, .. } => {
            let (page, limit) = page(config, *p);
            let object = match commit(id) {
                Some(commit) => a::TreeOf::Commit(commit),
                None => a::TreeOf::Tree(tree_id(id).ok_or(missing)?),
            };
            Read::GitTree { object, page, limit }
        }
        Request::WorkflowRuns { head_sha, page: p, .. } => {
            let (page, limit) = page(config, *p);
            Read::Runs { workflow: None, commit: Some(commit(head_sha).ok_or(missing)?), number: None, page, limit }
        }
        Request::RepositoryById { .. } | Request::Repository { .. } => Read::Settings,
        Request::Issues { state, kind, since, sort, page: p, .. } => {
            let (page, limit) = page(config, *p);
            Read::Items {
                state: match state {
                    Some(r::StateFilter::Open) => Some(a::State::Open),
                    Some(r::StateFilter::Closed) => Some(a::State::Closed),
                    Some(r::StateFilter::All) | None => None,
                },
                kind: match kind {
                    Some(r::IssueKind::Issues) => Some(a::Kind::Issue),
                    Some(r::IssueKind::Pulls) => Some(a::Kind::Pull),
                    None => None,
                },
                labels: Box::from([]),
                author: None,
                order: match sort {
                    Some(r::Sort::LeastUpdate) => a::Order::LeastUpdated,
                    Some(r::Sort::Other) | None => a::Order::NewestCreated,
                },
                since: Time::from_nanos(since.unwrap_or(Wall::EPOCH).as_nanos()),
                page,
                limit,
            }
        }
        Request::Issue { number, .. } => Read::Item { number: *number, after: 0 },
        Request::Comments { number, .. } => Read::Comments { number: *number },
        Request::Pull { number, .. } | Request::Reviews { number, .. } | Request::ReviewComments { number, .. } => {
            Read::Pull { number: *number }
        }
        Request::PullFor { head, base, .. } => Read::FirstPull { head: head.clone(), base: base.clone() },
        Request::CombinedStatus { commit: id, .. } => Read::Checks { commit: commit(id).ok_or(missing)? },
        Request::Branch { name, .. } => Read::Branch { branch: name.clone() },
        Request::Branches { .. } => return Ok(Op::Git(a::Git::Clone)),
        Request::Protection { name, .. } => Read::Protection { branch: name.clone() },
        Request::Protections { .. } => protections(config, request)?,
        Request::Collaborators { .. } => Read::Collaborators,
        Request::Permission { login, .. } => {
            Read::Permission { user: uid(config, login).ok_or(a::Error::Missing(a::What::Repository))? }
        }
        Request::Pulls { page: p, .. } => {
            let (page, limit) = page(config, *p);
            Read::OpenPulls { page, limit }
        }
        Request::PullFiles { number, page: p, .. } => {
            let (page, limit) = page(config, *p);
            Read::PullFiles { number: *number, page, limit }
        }
        Request::Compare { before, after, .. } => Read::Compare {
            base: commit(before).ok_or(missing)?,
            head: commit(after).ok_or(missing)?,
            page: 1,
            limit: config.max_response_items,
        },
        Request::Contents { reference, path, .. } | Request::Raw { reference, path, .. } => {
            Read::File { commit: commit(reference).ok_or(missing)?, path: path.clone() }
        }
        Request::Runs { head_sha, run_number, .. } => Read::Runs {
            workflow: None,
            commit: Some(commit(head_sha).ok_or(missing)?),
            number: Some(*run_number),
            page: 1,
            limit: 1,
        },
        Request::Run { run, .. } => Read::Run { run: *run },
        Request::Job { job, .. } => Read::Job { job: *job },
        Request::Jobs { run, .. } => Read::Jobs { run: *run },
        Request::JobLog { job, attempt, step, .. } => Read::Log { job: *job, attempt: *attempt, step: *step },
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
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::DeleteBranch { .. } => return write(config, request),
    };
    Ok(Op::Read(read))
}
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive domain request selection also names the protocol-only setup variants"
)]
fn write(config: &Config, request: &Request) -> Result<Op, a::Error> {
    let missing = a::Error::Missing(a::What::Commit);
    match request {
        Request::Dispatch { workflow, branch, inputs, .. } => {
            let mut values = List::with_capacity(u32::try_from(inputs.len()).expect("bounded inputs"));
            for input in inputs {
                values.push((input.name.clone(), input.value.clone())).expect("input capacity");
            }
            Ok(Op::Write(Write::Dispatch {
                workflow: workflow.clone(),
                branch: branch.clone(),
                inputs: values.into_boxed(),
            }))
        }
        Request::Rerun { job, .. } => Ok(Op::Write(Write::Rerun { job: *job })),
        Request::Labels { .. } | Request::SetLabels { .. } => Ok(Op::Read(Read::LabelIds)),
        Request::CreateLabel { name, .. } => Ok(Op::Write(Write::DefineLabel { name: name.clone() })),
        Request::CreateIssue { title, body, labels, .. } => {
            if !labels.is_empty() {
                return Ok(Op::Read(Read::LabelIds));
            }
            Ok(Op::Write(Write::CreateIssue { title: title.clone(), body: body.clone(), labels: Box::from([]) }))
        }
        Request::OpenPull { title, body, head, base, .. } => Ok(Op::Write(Write::OpenPull {
            title: title.clone(),
            body: body.clone(),
            head: head.clone(),
            base: base.clone(),
            opens_empty: true,
        })),
        Request::PostComment { number, body, .. } => {
            Ok(Op::Write(Write::Comment { number: *number, body: body.clone() }))
        }
        Request::PostReview { number, event, body, .. } => {
            Ok(Op::Write(Write::Review { number: *number, body: body.clone(), verdict: Some(verdict(*event)) }))
        }
        Request::EditIssue { number, title, body, state, .. } => {
            if state.is_some() && (title.is_some() || body.is_some()) {
                return Err(a::Error::Refused);
            }
            Ok(Op::Write(match state {
                Some(d::IssueState::Closed) => Write::Close { number: *number },
                Some(d::IssueState::Open) => Write::Reopen { number: *number },
                None => Write::EditItem { number: *number, title: title.clone(), body: body.clone() },
            }))
        }
        Request::RequestReviewers { number, reviewers: names, .. } => Ok(Op::Write(Write::RequestReviewers {
            number: *number,
            reviewers: reviewers(config, names).ok_or(a::Error::Missing(a::What::Review))?,
        })),
        Request::RemoveReviewers { number, reviewers: names, .. } => Ok(Op::Write(Write::RemoveReviewers {
            number: *number,
            reviewers: reviewers(config, names).ok_or(a::Error::Missing(a::What::Review))?,
        })),
        Request::Merge { number, style, head, .. } => {
            let how = match style {
                d::MergeStyle::Squash => a::MergeHow::Style,
                d::MergeStyle::FastForwardOnly => a::MergeHow::FastForwardOnly,
                d::MergeStyle::Merge
                | d::MergeStyle::Rebase
                | d::MergeStyle::RebaseMerge
                | d::MergeStyle::ManuallyMerged => return Err(a::Error::Refused),
            };
            Ok(Op::Write(Write::Merge { number: *number, head: commit(head).ok_or(missing)?, how }))
        }
        Request::Update { number, style, .. } => {
            let style = match style {
                None | Some(r::UpdateStyle::Merge) => a::UpdateStyle::Merge,
                Some(r::UpdateStyle::Rebase) => a::UpdateStyle::Rebase,
            };
            Ok(Op::Write(Write::Update { number: *number, style }))
        }
        Request::PostStatus { commit: id, context, state, description, link, .. } => Ok(Op::Write(Write::Status {
            commit: commit(id).ok_or(missing)?,
            context: context.clone(),
            state: check(*state),
            description: description.clone(),
            link: link.clone(),
        })),
        Request::CreateBranch { branch, from, .. } => {
            Ok(Op::Write(Write::CreateBranch { branch: branch.clone(), commit: commit(from).ok_or(missing)? }))
        }
        Request::DeleteBranch { branch, .. } => Ok(Op::Write(Write::DeleteBranch { branch: branch.clone() })),
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
        | Request::UserRepos { .. }
        | Request::RepoSearch { .. }
        | Request::UserSearch { .. }
        | Request::RepositoryById { .. }
        | Request::Repository { .. }
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
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. } => Err(a::Error::Refused),
    }
}

fn protections(config: &Config, request: &Request) -> Result<Read, a::Error> {
    let path = repository(config, request).ok_or(a::Error::Missing(a::What::Repository))?;
    let mut branch = Box::from([]);
    for repo in &config.repositories {
        if repo.full == path {
            branch = repo.protected.clone().unwrap_or(Box::from([]));
        }
    }
    Ok(Read::Protection { branch })
}

fn verdict(event: r::ReviewEvent) -> a::Verdict {
    match event {
        r::ReviewEvent::Approved => a::Verdict::Approve,
        r::ReviewEvent::RequestChanges => a::Verdict::RequestChanges,
        r::ReviewEvent::Comment => a::Verdict::Comment,
    }
}

/// The domain's tree and blob ids occupy the SHA-1 document's twenty bytes.
#[must_use]
pub fn tree_object(id: [u8; 20]) -> ObjectId {
    let mut bytes = [0; 32];
    for (out, byte) in bytes.iter_mut().zip(id) {
        *out = byte;
    }
    ObjectId { format: ObjectFormat::Sha1, bytes }
}
fn tree_id(id: &ObjectId) -> Option<[u8; 20]> {
    if id.format != ObjectFormat::Sha1 {
        return None;
    }
    for byte in id.bytes.get(20..)? {
        if *byte != 0 {
            return None;
        }
    }
    let mut bytes = [0; 20];
    for (out, byte) in bytes.iter_mut().zip(id.bytes) {
        *out = byte;
    }
    Some(bytes)
}

pub(crate) fn resolves_labels(request: &Request) -> bool {
    match request {
        Request::SetLabels { .. } => true,
        Request::CreateIssue { labels, .. } => !labels.is_empty(),
        Request::RepoSearch { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::Org { .. }
        | Request::CreateOrg { .. }
        | Request::CreateRepository { .. }
        | Request::CreateFile { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::AddMember { .. }
        | Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
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
        | Request::Labels { .. }
        | Request::CreateLabel { .. }
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
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::CreateUser { .. } => false,
    }
}
pub(crate) fn labeled_write(request: &Request, catalog: &[a::Label]) -> Result<Op, a::Error> {
    let write = match request {
        Request::CreateIssue { title, body, labels, .. } => {
            Write::CreateIssue { title: title.clone(), body: body.clone(), labels: label_names(labels, catalog)? }
        }
        Request::SetLabels { number, labels, .. } => {
            Write::SetLabels { number: *number, labels: label_names(labels, catalog)? }
        }
        Request::RepoSearch { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::Org { .. }
        | Request::CreateOrg { .. }
        | Request::CreateRepository { .. }
        | Request::CreateFile { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::AddMember { .. }
        | Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
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
        | Request::Labels { .. }
        | Request::CreateLabel { .. }
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
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::CreateUser { .. } => return Err(a::Error::Refused),
    };
    Ok(Op::Write(write))
}
fn label_names(ids: &[u64], catalog: &[a::Label]) -> Result<Box<[Box<[u8]>]>, a::Error> {
    let mut labels = List::with_capacity(u32::try_from(ids.len()).or(Err(a::Error::TooLarge))?);
    // The pin silently drops unknown ids (issue-unknown-label-is-refused in the recorded facts).
    for id in ids {
        for label in catalog {
            if label.id == *id {
                labels.push(label.name.clone()).or(Err(a::Error::TooLarge))?;
                break;
            }
        }
    }
    Ok(labels.into_boxed())
}
