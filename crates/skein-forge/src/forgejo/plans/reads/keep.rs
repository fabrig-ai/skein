//! Cache updates, listing bounds and long-body diagnostics.
use super::{Body, Heard, Request, Tables, d};

pub(super) fn keep_user(user: &d::User, forge: u16, tables: &mut Tables) {
    tables.keep_user(forge, user.id, user.login.clone());
}
pub(super) fn keep_pull(pull: &d::PullRequest, forge: u16, tables: &mut Tables) {
    for user in &pull.requested_reviewers {
        keep_user(user, forge, tables);
    }
}
pub(super) fn keep_row(row: &d::Row, forge: u16, tables: &mut Tables) {
    match row {
        d::Row::User(user) => keep_user(user, forge, tables),
        d::Row::Issue(issue) => keep_user(&issue.user, forge, tables),
        d::Row::Comment(comment) => keep_user(&comment.user, forge, tables),
        d::Row::Review(review) => keep_user(&review.user, forge, tables),
        d::Row::ReviewComment(remark) => keep_user(&remark.user, forge, tables),
        d::Row::Status(status) => {
            if let Some(user) = &status.creator {
                keep_user(user, forge, tables);
            }
        }
        d::Row::Pull(pull) => keep_pull(pull, forge, tables),
        d::Row::Found(_)
        | d::Row::Reachable(_)
        | d::Row::Token(_)
        | d::Row::Branch(_)
        | d::Row::Protection(_)
        | d::Row::Entry { .. }
        | d::Row::File(_)
        | d::Row::Commit(_)
        | d::Row::TreeEntry(_)
        | d::Row::Run(_)
        | d::Row::Hook(_)
        | d::Row::Label { .. }
        | d::Row::Team(_)
        | d::Row::Step(_)
        | d::Row::Job(_) => {}
    }
}
pub(crate) fn keep_document(document: &d::Document, forge: u16, tables: &mut Tables) {
    match document {
        d::Document::User(user) => keep_user(user, forge, tables),
        d::Document::Repository(repo) => keep_user(&repo.owner, forge, tables),
        d::Document::Issue(issue) => keep_user(&issue.user, forge, tables),
        d::Document::Comment(comment) => keep_user(&comment.user, forge, tables),
        d::Document::Status(status) => {
            if let Some(user) = &status.creator {
                keep_user(user, forge, tables);
            }
        }
        d::Document::Review(review) => keep_user(&review.user, forge, tables),
        d::Document::Pull(pull) => keep_pull(pull, forge, tables),
        d::Document::Permission(permission) => keep_user(&permission.user, forge, tables),
        d::Document::Token(_)
        | d::Document::Org(_)
        | d::Document::CreatedFile { .. }
        | d::Document::CreatedRepository(_)
        | d::Document::Hook(_)
        | d::Document::Team(_)
        | d::Document::ApiSettings(_)
        | d::Document::RepoSearch { .. }
        | d::Document::Search { .. }
        | d::Document::Listed { .. }
        | d::Document::CombinedStatus(_)
        | d::Document::Branch(_)
        | d::Document::Protection(_)
        | d::Document::Compare(_)
        | d::Document::Tree { .. }
        | d::Document::Runs { .. }
        | d::Document::Job(_)
        | d::Document::Run(_)
        | d::Document::Error(_) => {}
    }
}
pub(super) fn page_bound(request: &Request) -> Option<u32> {
    match request {
        Request::Issues { page, .. }
        | Request::Reviews { page, .. }
        | Request::CombinedStatus { page, .. }
        | Request::Branches { prefix: None, page, .. }
        | Request::Collaborators { page, .. }
        | Request::Pulls { page, .. }
        | Request::PullFiles { page, .. }
        | Request::Tree { page, .. }
        | Request::WorkflowRuns { page, .. }
        | Request::Runs { page, .. } => page.limit,
        Request::UserSearch { limit, .. } => *limit,
        Request::RepoSearch { .. }
        | Request::UserRepos { .. }
        | Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
        | Request::Repository { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::ReviewComments { .. }
        | Request::Branch { .. }
        | Request::Protection { .. }
        | Request::Branches { prefix: Some(_), .. }
        | Request::Protections { .. }
        | Request::Permission { .. }
        | Request::Compare { .. }
        | Request::Contents { .. }
        | Request::Raw { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
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
        | Request::DeleteBranch { .. } => None,
    }
}
#[must_use]
pub fn long(heard: &Heard) -> bool {
    match heard {
        Heard::Row(row) => match row {
            d::Row::Issue(row) => long_body(&row.body),
            d::Row::Comment(row) => long_body(&row.body),
            d::Row::Review(row) => long_body(&row.body),
            d::Row::ReviewComment(row) => long_body(&row.body),
            d::Row::Found(_)
            | d::Row::Reachable(_)
            | d::Row::Token(_)
            | d::Row::User(_)
            | d::Row::Status(_)
            | d::Row::Branch(_)
            | d::Row::Protection(_)
            | d::Row::Pull(_)
            | d::Row::Entry { .. }
            | d::Row::File(_)
            | d::Row::Commit(_)
            | d::Row::TreeEntry(_)
            | d::Row::Run(_)
            | d::Row::Hook(_)
            | d::Row::Label { .. }
            | d::Row::Team(_)
            | d::Row::Step(_)
            | d::Row::Job(_) => false,
        },
        Heard::Done(Body::Document(document)) => match document {
            d::Document::Issue(row) => long_body(&row.body),
            d::Document::Comment(row) => long_body(&row.body),
            d::Document::Review(row) => long_body(&row.body),
            d::Document::Token(_)
            | d::Document::User(_)
            | d::Document::Org(_)
            | d::Document::CreatedFile { .. }
            | d::Document::CreatedRepository(_)
            | d::Document::Hook(_)
            | d::Document::Team(_)
            | d::Document::ApiSettings(_)
            | d::Document::Repository(_)
            | d::Document::RepoSearch { .. }
            | d::Document::Search { .. }
            | d::Document::Listed { .. }
            | d::Document::Status(_)
            | d::Document::Pull(_)
            | d::Document::CombinedStatus(_)
            | d::Document::Branch(_)
            | d::Document::Protection(_)
            | d::Document::Permission(_)
            | d::Document::Compare(_)
            | d::Document::Tree { .. }
            | d::Document::Runs { .. }
            | d::Document::Job(_)
            | d::Document::Run(_)
            | d::Document::Error(_) => false,
        },
        Heard::Done(Body::Empty | Body::Malformed(_) | Body::Raw { .. }) | Heard::Head { .. } | Heard::Failed(_) => {
            false
        }
    }
}
fn long_body(body: &d::Body) -> bool {
    match body {
        d::Body::Kept(_) => false,
        d::Body::Long(_) => true,
    }
}
