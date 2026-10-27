//! Per-route projections and post-write reads; one write precedes its readback.
use crate::{Config, Limits, answers, errors, routes, setup::Store};
use alloc::boxed::Box;
use skein_fake_forge_domain::api::{self as a, Answer};
use skein_forge::forgejo::{
    document as d,
    request::{Page, Request},
    server,
};
use skein_lib::{Decimal, List, Writer, bytes};
#[derive(PartialEq, Eq, Debug)]
pub enum Select {
    Document,
    Permission(u64),
    Branch(Box<[u8]>),
    Branches(Page),
    Review(u64),
    Status(Box<[u8]>),
    Raw(Option<Range>),
}
#[derive(PartialEq, Eq, Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Box<[skein_http::Header]>,
    pub body: Box<[u8]>,
}
fn number(answer: &Answer) -> Option<u64> {
    match answer {
        Answer::Created(id) | Answer::Commented(id) | Answer::Reviewed(id) => Some(*id),
        Answer::Located { .. }
        | Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Pulls(_)
        | Answer::Comments(_)
        | Answer::Runs(_)
        | Answer::Run(_)
        | Answer::Jobs(_)
        | Answer::Job { .. }
        | Answer::PullFiles { .. }
        | Answer::Comparison { .. }
        | Answer::Checks(_)
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Comment { .. }
        | Answer::Dependencies(_)
        | Answer::Labels(_)
        | Answer::Commit(_)
        | Answer::GitTree { .. }
        | Answer::Tree(_)
        | Answer::Artifact { .. }
        | Answer::Commits { .. }
        | Answer::File(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done
        | Answer::Cloned { .. }
        | Answer::Pushed(_)
        | Answer::Branch(_) => None,
    }
}
pub fn follow(request: &Request, answer: &Answer) -> Result<Option<(a::Read, Select)>, a::Error> {
    let next = match request {
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
        | Request::CreateUser { .. } => return Err(a::Error::Refused),
        Request::CreateIssue { .. } => {
            (a::Read::Item { number: number(answer).ok_or(a::Error::Refused)?, after: 0 }, Select::Document)
        }
        Request::OpenPull { .. } => {
            (a::Read::Pull { number: number(answer).ok_or(a::Error::Refused)? }, Select::Document)
        }
        Request::PostComment { .. } => {
            (a::Read::Comment { id: number(answer).ok_or(a::Error::Refused)? }, Select::Document)
        }
        Request::PostReview { number: pull, .. } => {
            (a::Read::Pull { number: *pull }, Select::Review(number(answer).ok_or(a::Error::Refused)?))
        }
        Request::EditIssue { number, .. } => (a::Read::Item { number: *number, after: 0 }, Select::Document),
        Request::PostStatus { commit, context, .. } => (
            a::Read::Checks { commit: routes::commit(commit).ok_or(a::Error::Missing(a::What::Commit))? },
            Select::Status(context.clone()),
        ),
        Request::CreateBranch { branch, .. } => {
            (a::Read::Branch { branch: branch.clone() }, Select::Branch(branch.clone()))
        }
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
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::DeleteBranch { .. } => return Ok(None),
    };
    Ok(Some(next))
}
pub fn select(config: &Config, request: &Request, range: Option<Range>) -> Result<Select, a::Error> {
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
        | Request::CreateUser { .. } => Err(a::Error::Refused),
        Request::Branch { name, .. } => Ok(Select::Branch(name.clone())),
        Request::Branches { page, .. } => Ok(Select::Branches(*page)),
        Request::Permission { login, .. } => {
            for (user, name) in &config.users {
                if name == login {
                    return Ok(Select::Permission(*user));
                }
            }
            let id = login.strip_prefix(b"u").ok_or(a::Error::Missing(a::What::Repository))?;
            Ok(Select::Permission(
                skein_forge::forgejo::id::unsigned(id).ok_or(a::Error::Missing(a::What::Repository))?,
            ))
        }
        Request::Raw { .. } | Request::JobLog { .. } => Ok(Select::Raw(range)),
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
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Pulls { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Commits { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
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
        | Request::DeleteBranch { .. } => Ok(Select::Document),
    }
}
fn status(request: &Request) -> u16 {
    match crate::route(request) {
        crate::Route::EditIssue
        | crate::Route::RequestReviewers
        | crate::Route::CreateIssue
        | crate::Route::OpenPull
        | crate::Route::PostComment
        | crate::Route::PostStatus
        | crate::Route::CreateBranch => 201,
        crate::Route::Dispatch | crate::Route::Rerun | crate::Route::RemoveReviewers | crate::Route::DeleteBranch => {
            204
        }
        crate::Route::UserOf
        | crate::Route::Tokens
        | crate::Route::CreateToken
        | crate::Route::DeleteToken
        | crate::Route::Git(_)
        | crate::Route::Org
        | crate::Route::CreateOrg
        | crate::Route::CreateFile
        | crate::Route::CreateRepository
        | crate::Route::Teams
        | crate::Route::CreateTeam
        | crate::Route::EditTeam
        | crate::Route::TeamMembers
        | crate::Route::OrgHooks
        | crate::Route::CreateOrgHook
        | crate::Route::EditOrgHook
        | crate::Route::DeleteOrgHook
        | crate::Route::AddCollaborator
        | crate::Route::RepoHooks
        | crate::Route::CreateRepoHook
        | crate::Route::EditRepoHook
        | crate::Route::DeleteRepoHook
        | crate::Route::AddMember
        | crate::Route::CreateUser
        | crate::Route::User
        | crate::Route::ApiSettings
        | crate::Route::RepositoryById
        | crate::Route::Repository
        | crate::Route::UserRepos
        | crate::Route::RepoSearch
        | crate::Route::UserSearch
        | crate::Route::Issues
        | crate::Route::Issue
        | crate::Route::Comments
        | crate::Route::Pull
        | crate::Route::PullFor
        | crate::Route::Pulls
        | crate::Route::CombinedStatus
        | crate::Route::Reviews
        | crate::Route::ReviewComments
        | crate::Route::Branch
        | crate::Route::Branches
        | crate::Route::Protection
        | crate::Route::Protections
        | crate::Route::Collaborators
        | crate::Route::Permission
        | crate::Route::PullFiles
        | crate::Route::Commits
        | crate::Route::Compare
        | crate::Route::Raw
        | crate::Route::Tree
        | crate::Route::WorkflowRuns
        | crate::Route::Runs
        | crate::Route::Run
        | crate::Route::Job
        | crate::Route::Jobs
        | crate::Route::JobLog
        | crate::Route::PostReview
        | crate::Route::Merge
        | crate::Route::Update => 200,
    }
}
#[must_use]
pub fn failure(limits: &Limits, failure: errors::Failure) -> Reply {
    let body = if failure.message.is_empty() {
        Box::from([])
    } else {
        server::encode(
            &server::Answer::Error(d::ErrorBody { message: d::Body::Kept(bytes::copy_of(failure.message)) }),
            &limits.documents,
        )
        .unwrap_or(Box::from([]))
    };
    let mut headers = List::with_capacity(1);
    if let Some(seconds) = failure.retry_after {
        let digits = Decimal::of(seconds);
        headers
            .push(skein_http::Header { name: bytes::copy_of(b"retry-after"), value: bytes::copy_of(digits.as_bytes()) })
            .expect("one header");
    }
    Reply { status: failure.status, body, headers: headers.into_boxed() }
}
fn document(
    config: &Config,
    setup: &Store,
    request: &Request,
    select: &Select,
    answer: &Answer,
) -> Result<server::Answer, a::Error> {
    match answer {
        Answer::Comment { comment, .. } => Ok(server::Answer::Comment(answers::comment(config, setup, comment))),
        Answer::Permission(permission) => {
            let user = match select {
                Select::Permission(user) => *user,
                Select::Document
                | Select::Branch(_)
                | Select::Branches(_)
                | Select::Review(_)
                | Select::Status(_)
                | Select::Raw(_) => return Err(a::Error::Refused),
            };
            Ok(server::Answer::Permission(d::CollaboratorPermission {
                permission: answers::permission(*permission),
                user: setup.resolve_user(config, user),
            }))
        }
        Answer::Commit(commit) => {
            let name = match select {
                Select::Branch(name) => name,
                Select::Document
                | Select::Permission(_)
                | Select::Branches(_)
                | Select::Review(_)
                | Select::Status(_)
                | Select::Raw(_) => return Err(a::Error::Refused),
            };
            Ok(server::Answer::Branch(answers::branch(config, request, name, *commit)))
        }
        Answer::Cloned { branches: values, .. } => branches(config, request, select, values),
        Answer::Pull(pull) => pull_document(config, setup, request, select, pull, answer),
        Answer::Checks(checks) => match select {
            Select::Status(context) => {
                for check in checks {
                    if check.status.context == *context {
                        return Ok(server::Answer::Status(d::CommitStatus {
                            creator: if check.link.is_empty() {
                                Some(setup.resolve_user(config, check.status.author))
                            } else {
                                None
                            },
                            updated_at: skein_lib::Wall::from_nanos(check.status.at.as_nanos()),
                            context: check.status.context.clone(),
                            status: answers::status(check.status.state),
                            description: check.description.clone(),
                            target_url: check.link.clone(),
                        }));
                    }
                }
                Err(a::Error::Missing(a::What::Commit))
            }
            Select::Document => answers::read(config, setup, request, answer),
            Select::Permission(_) | Select::Branch(_) | Select::Branches(_) | Select::Review(_) | Select::Raw(_) => {
                Err(a::Error::Refused)
            }
        },
        Answer::Located { .. }
        | Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pulls(_)
        | Answer::Comments(_)
        | Answer::Runs(_)
        | Answer::Run(_)
        | Answer::Jobs(_)
        | Answer::Job { .. }
        | Answer::PullFiles { .. }
        | Answer::Comparison { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators(_)
        | Answer::Statuses(_)
        | Answer::Dependencies(_)
        | Answer::Labels(_)
        | Answer::GitTree { .. }
        | Answer::Tree(_)
        | Answer::Artifact { .. }
        | Answer::Commits { .. }
        | Answer::File(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done
        | Answer::Pushed(_)
        | Answer::Branch(_) => answers::read(config, setup, request, answer),
    }
}
#[expect(clippy::too_many_lines, reason = "the response dispatcher exhaustively names the domain answers")]
pub fn answer(
    config: &Config,
    setup: &Store,
    limits: &Limits,
    request: &Request,
    select: &Select,
    answer: &Answer,
) -> Result<Reply, a::Error> {
    bounded::answer(config, limits, answer).ok_or(a::Error::TooLarge)?;
    match answer {
        Answer::Done | Answer::Merged(_) => {
            return Ok(Reply { status: status(request), headers: Box::from([]), body: Box::from([]) });
        }
        Answer::File(body) => {
            let range = match select {
                Select::Raw(range) => *range,
                Select::Document
                | Select::Permission(_)
                | Select::Branch(_)
                | Select::Branches(_)
                | Select::Review(_)
                | Select::Status(_) => return Err(a::Error::Refused),
            };
            return raw(limits, body, range);
        }
        Answer::Located { .. }
        | Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Pulls(_)
        | Answer::Comments(_)
        | Answer::Runs(_)
        | Answer::Run(_)
        | Answer::Jobs(_)
        | Answer::Job { .. }
        | Answer::PullFiles { .. }
        | Answer::Comparison { .. }
        | Answer::Checks(_)
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Comment { .. }
        | Answer::Dependencies(_)
        | Answer::Labels(_)
        | Answer::Commit(_)
        | Answer::GitTree { .. }
        | Answer::Tree(_)
        | Answer::Artifact { .. }
        | Answer::Commits { .. }
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Revision(_)
        | Answer::Cloned { .. }
        | Answer::Pushed(_)
        | Answer::Branch(_) => {}
    }
    let document = document(config, setup, request, select, answer)?;
    let Ok(body) = server::encode(&document, &limits.documents) else {
        return Err(a::Error::TooLarge);
    };
    let mut headers = List::with_capacity(1);
    match answer {
        Answer::PullFiles { more, .. } => headers
            .push(skein_http::Header {
                name: bytes::copy_of(b"x-hasmore"),
                value: bytes::copy_of(if *more { b"true" } else { b"false" }),
            })
            .expect("one header"),
        Answer::Located { .. }
        | Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Pulls(_)
        | Answer::Comments(_)
        | Answer::Runs(_)
        | Answer::Run(_)
        | Answer::Jobs(_)
        | Answer::Job { .. }
        | Answer::Comparison { .. }
        | Answer::Checks(_)
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Comment { .. }
        | Answer::Dependencies(_)
        | Answer::Labels(_)
        | Answer::Commit(_)
        | Answer::GitTree { .. }
        | Answer::Tree(_)
        | Answer::Artifact { .. }
        | Answer::Commits { .. }
        | Answer::File(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        | Answer::Revision(_)
        | Answer::Done
        | Answer::Cloned { .. }
        | Answer::Pushed(_)
        | Answer::Branch(_) => {}
    }
    Ok(Reply { status: status(request), headers: headers.into_boxed(), body })
}
/// One byte range, as the pinned Forgejo's plaintext routes serve it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Range {
    /// A range with a first byte and an optional last byte.
    From { first: u64, last: Option<u64> },
    /// The last N bytes, with N at least one.
    Suffix(u64),
}

/// Reads the request's single byte range, or refuses an unsupported form.
pub fn range(value: Option<&[u8]>) -> Result<Option<Range>, a::Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.strip_prefix(b"bytes=").ok_or(a::Error::Refused)?;
    if let Some(suffix) = value.strip_prefix(b"-") {
        let count = skein_forge::forgejo::id::unsigned(suffix).ok_or(a::Error::Refused)?;
        if count == 0 {
            return Err(a::Error::Refused);
        }
        return Ok(Some(Range::Suffix(count)));
    }
    let dash = bytes::find(value, b"-").ok_or(a::Error::Refused)?;
    let first =
        skein_forge::forgejo::id::unsigned(value.get(..dash).ok_or(a::Error::Refused)?).ok_or(a::Error::Refused)?;
    let last = value.get(dash.saturating_add(1)..).ok_or(a::Error::Refused)?;
    let last =
        if last.is_empty() { None } else { Some(skein_forge::forgejo::id::unsigned(last).ok_or(a::Error::Refused)?) };
    if let Some(last) = last
        && last < first
    {
        return Err(a::Error::Refused);
    }
    Ok(Some(Range::From { first, last }))
}
fn raw(limits: &Limits, body: &[u8], range: Option<Range>) -> Result<Reply, a::Error> {
    let total = u64::try_from(body.len()).expect("bounded body");
    let range = match range {
        Some(Range::From { .. }) if total == 0 => None,
        Some(Range::From { first, last }) => Some((first, last)),
        Some(Range::Suffix(_)) if total == 0 => {
            return Ok(Reply {
                status: 206,
                headers: Box::new([skein_http::Header {
                    name: bytes::copy_of(b"content-range"),
                    value: bytes::copy_of(b"bytes 0--1/0"),
                }]),
                body: Box::new([]),
            });
        }
        Some(Range::Suffix(count)) => Some((total.saturating_sub(count), None)),
        None => None,
    };
    let mut headers = List::with_capacity(1);
    let mut status = 200;
    let bytes = if let Some((first, last)) = range {
        if first >= total {
            return Ok(Reply { status: 416, body: Box::from([]), headers: Box::from([]) });
        }
        let last = last.unwrap_or(total.saturating_sub(1)).min(total.saturating_sub(1));
        let first_text = Decimal::of(first);
        let last_text = Decimal::of(last);
        let total_text = Decimal::of(total);
        let mut value = Writer::new(
            6usize
                .saturating_add(first_text.as_bytes().len())
                .saturating_add(last_text.as_bytes().len())
                .saturating_add(total_text.as_bytes().len())
                .saturating_add(2),
        );
        for part in
            [b"bytes ".as_slice(), first_text.as_bytes(), b"-", last_text.as_bytes(), b"/", total_text.as_bytes()]
        {
            value.put(part).expect("sized content range");
        }
        headers
            .push(skein_http::Header { name: bytes::copy_of(b"content-range"), value: value.finish() })
            .expect("one header");
        status = 206;
        let Ok(first) = usize::try_from(first) else {
            return Err(a::Error::TooLarge);
        };
        let Ok(end) = usize::try_from(last.saturating_add(1)) else {
            return Err(a::Error::TooLarge);
        };
        body.get(first..end).ok_or(a::Error::Refused)?
    } else {
        body
    };
    if bytes.len() > usize::try_from(limits.raw_bytes).expect("u32 fits") {
        return Err(a::Error::TooLarge);
    }
    Ok(Reply { status, headers: headers.into_boxed(), body: bytes::copy_of(bytes) })
}

fn pull_document(
    config: &Config,
    setup: &Store,
    request: &Request,
    select: &Select,
    pull: &a::Pull,
    answer: &Answer,
) -> Result<server::Answer, a::Error> {
    match select {
        Select::Review(id) => {
            for value in &pull.reviews {
                if value.id == *id {
                    return Ok(server::Answer::Review(answers::review(config, setup, value)));
                }
            }
            Err(a::Error::Missing(a::What::Review))
        }
        Select::Document => {
            let path = routes::repository(config, request).ok_or(a::Error::Missing(a::What::Repository))?;
            if crate::route(request) == crate::Route::OpenPull {
                for repo in &config.repositories {
                    if repo.full == path {
                        return Ok(server::Answer::Pull(answers::pull(config, setup, u64::from(repo.id), pull)));
                    }
                }
                Err(a::Error::Missing(a::What::Repository))
            } else {
                answers::read(config, setup, request, answer)
            }
        }
        Select::Permission(_) | Select::Branch(_) | Select::Branches(_) | Select::Status(_) | Select::Raw(_) => {
            Err(a::Error::Refused)
        }
    }
}

fn branches(
    config: &Config,
    request: &Request,
    select: &Select,
    branches: &[a::Head],
) -> Result<server::Answer, a::Error> {
    if let Request::Branches { prefix: Some(prefix), .. } = request {
        let Ok(capacity) = u32::try_from(branches.len()) else {
            return Err(a::Error::TooLarge);
        };
        let mut out = List::with_capacity(capacity);
        for branch in branches {
            if branch.branch.starts_with(prefix) {
                out.push(answers::branch(config, request, &branch.branch, branch.commit)).expect("bounded clone");
            }
        }
        if out.is_empty() {
            return Err(a::Error::Missing(a::What::Branch));
        }
        return Ok(server::Answer::BranchRefs(out.into_boxed()));
    }
    let page = match select {
        Select::Branches(page) => *page,
        Select::Document
        | Select::Permission(_)
        | Select::Branch(_)
        | Select::Review(_)
        | Select::Status(_)
        | Select::Raw(_) => return Err(a::Error::Refused),
    };
    let (page, limit) = routes::page(config, page);
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(limit));
    let mut out = List::with_capacity(limit);
    for branch in branches
        .iter()
        .skip(usize::try_from(skip).unwrap_or(usize::MAX))
        .take(usize::try_from(limit).expect("u32 fits"))
    {
        out.push(answers::branch(config, request, &branch.branch, branch.commit)).expect("page cap");
    }
    Ok(server::Answer::Branches(out.into_boxed()))
}

mod bounded {
    //! Price a projection before it clones unpaged domain rows.
    use crate::{Config, Limits};
    use alloc::boxed::Box;
    use skein_fake_forge_domain::api as a;
    struct Budget {
        left: usize,
        user: usize,
    }
    impl Budget {
        fn row(&mut self, bytes: usize) -> Option<()> {
            self.left = self.left.checked_sub(bytes.checked_add(self.user)?.checked_add(256)?)?;
            Some(())
        }
        fn names(&mut self, names: &[Box<[u8]>]) -> Option<()> {
            for name in names {
                self.left = self.left.checked_sub(name.len().checked_add(16)?)?;
            }
            Some(())
        }
        fn issue(&mut self, item: &a::Summary) -> Option<()> {
            self.row(item.title.len().checked_add(item.body.len())?)?;
            self.names(&item.labels)
        }
        fn pull(&mut self, pull: &a::Pull) -> Option<()> {
            self.row(pull.head.len().checked_add(pull.base.len())?)?;
            for _ in &pull.reviewers {
                self.row(0)?;
            }
            for review in &pull.reviews {
                self.row(review.body.len())?;
            }
            Some(())
        }
        fn run(&mut self, config: &Config, limits: &Limits, run: &a::Run) -> Option<()> {
            let workflow = match &run.workflow {
                Some(file) => file.len(),
                None => 2,
            };
            self.row(
                workflow
                    .checked_add(config.base.len())?
                    .checked_add(usize::try_from(limits.documents.target_bytes).ok()?)?,
            )
        }
        fn steps(&mut self, steps: &[a::Step]) -> Option<()> {
            self.row(0)?;
            for step in steps {
                self.row(step.name.len())?;
            }
            Some(())
        }
        fn files(&mut self, files: &[a::ChangedFile]) -> Option<()> {
            for file in files {
                self.row(file.path.len())?;
            }
            Some(())
        }
    }
    /// The temporary document's arrays, text and user objects fit one encoded-answer budget.
    #[expect(clippy::too_many_lines, reason = "each domain answer explicitly prices its temporary document")]
    pub fn answer(config: &Config, limits: &Limits, answer: &a::Answer) -> Option<()> {
        let mut user = 21usize;
        for (_, login) in &config.users {
            user = user.max(login.len());
        }
        let mut budget = Budget { left: usize::try_from(limits.documents.writer.length).ok()?, user };
        match answer {
            a::Answer::Items { items, .. } => {
                for item in items {
                    budget.issue(item)?;
                }
            }
            a::Answer::Item { item, .. } => budget.issue(item)?,
            a::Answer::Comments(comments) => {
                for comment in comments {
                    budget.row(comment.body.len())?;
                }
            }
            a::Answer::Comment { comment, .. } => budget.row(comment.body.len())?,
            a::Answer::Pull(pull) => budget.pull(pull)?,
            a::Answer::Pulls(pulls) => {
                for pull in pulls {
                    budget.pull(pull)?;
                }
            }
            a::Answer::Checks(checks) => {
                for check in checks {
                    budget.row(
                        check
                            .status
                            .context
                            .len()
                            .checked_add(check.description.len())?
                            .checked_add(check.link.len())?,
                    )?;
                }
            }
            a::Answer::Protection(Some(protection)) => {
                budget.row(protection.branch.len())?;
                budget.names(&protection.contexts)?;
            }
            a::Answer::Settings(settings) => budget.row(
                settings
                    .default
                    .len()
                    .checked_add(config.base.len())?
                    .checked_add(usize::try_from(limits.documents.target_bytes).ok()?)?,
            )?,
            a::Answer::Collaborators(users) => {
                for _ in users {
                    budget.row(0)?;
                }
            }
            a::Answer::Runs(runs) => {
                for run in runs {
                    budget.run(config, limits, run)?;
                }
            }
            a::Answer::Run(run) => budget.run(config, limits, run)?,
            a::Answer::Jobs(jobs) => {
                for job in jobs {
                    budget.row(job.name.len())?;
                }
            }
            a::Answer::GitTree { entries, .. } => {
                for entry in entries {
                    budget.row(entry.name.len())?;
                }
            }
            a::Answer::Job { steps, .. } => budget.steps(steps)?,
            a::Answer::PullFiles { files, .. } => budget.files(files)?,
            a::Answer::Comparison { files, commits, .. } => {
                budget.files(files)?;
                for _ in commits {
                    budget.row(0)?;
                }
            }
            a::Answer::Commits { commits } => {
                for commit in commits {
                    let parents = commit.parents.len().checked_mul(size_of::<skein_forge::forgejo::id::ObjectId>())?;
                    budget.row(commit.message.len().checked_add(parents)?)?;
                }
            }
            a::Answer::Cloned { branches, .. } => {
                for branch in branches {
                    budget.row(branch.branch.len())?;
                }
            }
            a::Answer::File(bytes) => {
                if bytes.len() > usize::try_from(limits.raw_bytes).ok()? {
                    return None;
                }
            }
            a::Answer::Protection(None)
            | a::Answer::Located { .. }
            | a::Answer::Permission(_)
            | a::Answer::Statuses(_)
            | a::Answer::Dependencies(_)
            | a::Answer::Labels(_)
            | a::Answer::Commit(_)
            | a::Answer::Tree(_)
            | a::Answer::Artifact { .. }
            | a::Answer::Pages { .. }
            | a::Answer::Page(_)
            | a::Answer::Created(_)
            | a::Answer::Commented(_)
            | a::Answer::Reviewed(_)
            | a::Answer::Merged(_)
            | a::Answer::Revision(_)
            | a::Answer::Done
            | a::Answer::Pushed(_)
            | a::Answer::Branch(_) => {}
        }
        Some(())
    }
}
