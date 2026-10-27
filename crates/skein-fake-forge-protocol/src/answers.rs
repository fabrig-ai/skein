//! The fake's typed reads in Forgejo's document vocabulary (fake-forge.md, 4).
use crate::{Config, routes, setup::Store};
use alloc::boxed::Box;
use skein_fake_forge_domain::api as a;
use skein_forge::forgejo::{document as d, id::ObjectFormat, request::Request, server::Answer};
use skein_lib::{List, Wall, Writer, bytes};

#[must_use]
pub fn state(state: a::State) -> d::IssueState {
    match state {
        a::State::Open => d::IssueState::Open,
        a::State::Closed => d::IssueState::Closed,
    }
}
#[must_use]
pub fn status(state: a::Check) -> d::StatusState {
    match state {
        a::Check::Pending => d::StatusState::Pending,
        a::Check::Passed => d::StatusState::Success,
        a::Check::Failed => d::StatusState::Failure,
    }
}
#[must_use]
pub fn issue(config: &Config, setup: &Store, item: &a::Summary) -> d::Issue {
    d::Issue {
        number: item.number,
        pull: item.kind == a::Kind::Pull,
        state: state(item.state),
        user: setup.resolve_user(config, item.author),
        labels: item.labels.clone(),
        updated_at: Wall::from_nanos(item.updated.as_nanos()),
        title: item.title.clone(),
        body: d::Body::Kept(item.body.clone()),
    }
}
#[must_use]
pub fn comment(config: &Config, setup: &Store, comment: &a::Comment) -> d::Comment {
    d::Comment {
        id: comment.id,
        user: setup.resolve_user(config, comment.author),
        created_at: Some(Wall::from_nanos(comment.created.as_nanos())),
        updated_at: wall(comment.edited),
        body: d::Body::Kept(comment.body.clone()),
    }
}
#[must_use]
pub fn review(config: &Config, setup: &Store, review: &a::Review) -> d::Review {
    d::Review {
        id: review.id,
        user: setup.resolve_user(config, review.author),
        state: match review.verdict {
            a::Verdict::Approve => d::ReviewState::Approved,
            a::Verdict::RequestChanges => d::ReviewState::RequestChanges,
            a::Verdict::Comment => d::ReviewState::Comment,
        },
        commit_id: routes::object(review.commit),
        body: d::Body::Kept(review.body.clone()),
        submitted_at: Some(Wall::from_nanos(review.at.as_nanos())),
        updated_at: None,
        official: review.official,
        dismissed: false,
    }
}
fn repository_id(config: &Config, request: &Request) -> Option<u64> {
    let full = routes::repository(config, request)?;
    for repo in &config.repositories {
        if repo.full == full {
            return Some(u64::from(repo.id));
        }
    }
    None
}
#[must_use]
pub fn pull(config: &Config, setup: &Store, id: u64, pull: &a::Pull) -> d::PullRequest {
    let mut reviewers = List::with_capacity(u32::try_from(pull.reviewers.len()).expect("domain bound"));
    for user in &pull.reviewers {
        reviewers.push(setup.resolve_user(config, *user)).expect("sized reviewers");
    }
    d::PullRequest {
        number: pull.number,
        state: state(pull.state),
        head: d::PullBranch { repo_id: id, name: pull.head.clone(), sha: Some(routes::object(pull.commit)) },
        base: d::PullBranch { repo_id: id, name: pull.base.clone(), sha: optional_object(pull.base_commit) },
        merged: pull.merged.is_some(),
        merge_commit_sha: optional_object(pull.merged),
        merge_base: optional_object(pull.merge_base),
        // Forgejo exposes a boolean: false while mergeability is checking.
        mergeable: pull.mergeable,
        requested_reviewers: reviewers.into_boxed(),
    }
}
#[must_use]
pub fn protection(protection: &a::Protection) -> d::BranchProtection {
    d::BranchProtection {
        rule_name: protection.branch.clone(),
        enable_status_check: !protection.contexts.is_empty(),
        status_check_contexts: protection.contexts.clone(),
        required_approvals: protection.approvals,
        dismiss_stale_approvals: protection.dismiss_stale,
        // The domain does not sign, so it keeps no signatures rule.
        require_signed_commits: false,
    }
}
#[must_use]
pub fn permission(permission: a::Permission) -> d::Permission {
    match permission {
        a::Permission::None => d::Permission::None,
        a::Permission::Read => d::Permission::Read,
        a::Permission::Write => d::Permission::Write,
        a::Permission::Admin => d::Permission::Admin,
    }
}
#[must_use]
pub fn branch(config: &Config, request: &Request, name: &[u8], commit: u64) -> d::Branch {
    let mut protected = None;
    if let Some(full) = routes::repository(config, request) {
        for repo in &config.repositories {
            if repo.full == full && repo.protected.as_deref() == Some(name) {
                protected.clone_from(&repo.protected);
            }
        }
    }
    d::Branch {
        name: bytes::copy_of(name),
        commit: routes::object(commit),
        effective_branch_protection_name: protected,
    }
}
fn files(files: &[a::ChangedFile]) -> Box<[d::ChangedFile]> {
    let mut out = List::with_capacity(u32::try_from(files.len()).expect("domain bound"));
    for file in files {
        let status = if file.before.is_none() {
            d::FileStatus::Added
        } else if file.after.is_none() {
            d::FileStatus::Deleted
        } else {
            d::FileStatus::Modified
        };
        out.push(d::ChangedFile { filename: file.path.clone(), previous_filename: None, status }).expect("sized files");
    }
    out.into_boxed()
}
fn compared(request: &Request, values: &[a::ChangedFile]) -> Box<[d::ChangedFile]> {
    if let Request::Compare { files: false, .. } = request { Box::from([]) } else { files(values) }
}
fn repository(config: &Config, request: &Request, settings: &a::Settings) -> Result<Answer, a::Error> {
    let full = routes::repository(config, request).ok_or(a::Error::Missing(a::What::Repository))?;
    repository_document(
        config,
        repository_id(config, request).ok_or(a::Error::Missing(a::What::Repository))?,
        &full,
        settings,
    )
}
/// Repository identity and URL are shared by domain and setup reads.
pub(crate) fn repository_document(
    config: &Config,
    id: u64,
    full: &[u8],
    settings: &a::Settings,
) -> Result<Answer, a::Error> {
    let slash = bytes::find(full, b"/").ok_or(a::Error::Missing(a::What::Repository))?;
    let owner = full.get(..slash).ok_or(a::Error::Refused)?;
    let name = full.get(slash.saturating_add(1)..).ok_or(a::Error::Refused)?;
    let mut html = Writer::new(config.base.len().saturating_add(full.len()).saturating_add(1));
    html.put(&config.base).expect("sized html URL");
    html.put(b"/").expect("sized html URL");
    html.put(full).expect("sized html URL");
    Ok(Answer::Repository(d::Repository {
        id,
        owner: d::User { id: 0, login: bytes::copy_of(owner), is_admin: false, full_name: Box::from([]) },
        name: bytes::copy_of(name),
        object_format: ObjectFormat::Sha1,
        default_branch: settings.default.clone(),
        default_merge_style: d::MergeStyle::Squash,
        allow_merge_commits: settings.merge,
        allow_squash_merge: settings.squash,
        allow_rebase: settings.rebase,
        html_url: html.finish(),
    }))
}
fn pull_answer(config: &Config, setup: &Store, request: &Request, value: &a::Pull) -> Result<Answer, a::Error> {
    match request {
        Request::Reviews { page, .. } => {
            let (page, limit) = routes::page(config, *page);
            let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(limit));
            let mut out = List::with_capacity(limit);
            for value in value
                .reviews
                .iter()
                .skip(usize::try_from(skip).unwrap_or(usize::MAX))
                .take(usize::try_from(limit).expect("u32 fits"))
            {
                out.push(review(config, setup, value)).expect("page cap");
            }
            Ok(Answer::Reviews(out.into_boxed()))
        }
        Request::ReviewComments { review: id, .. } => {
            for value in &value.reviews {
                if value.id == *id {
                    return Ok(Answer::ReviewComments(Box::from([])));
                }
            }
            Err(a::Error::Missing(a::What::Review))
        }
        Request::Pull { .. } | Request::PullFor { .. } => Ok(Answer::Pull(pull(
            config,
            setup,
            repository_id(config, request).ok_or(a::Error::Missing(a::What::Repository))?,
            value,
        ))),
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
        | Request::CombinedStatus { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::Pulls { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
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
        | Request::DeleteBranch { .. } => Err(a::Error::Refused),
    }
}
fn checks(config: &Config, setup: &Store, request: &Request, values: &[a::CheckSummary]) -> Result<Answer, a::Error> {
    let mut rows = List::with_capacity(u32::try_from(values.len()).expect("domain bound"));
    let mut combined = None;
    for value in values {
        match value.status.state {
            a::Check::Failed => combined = Some(d::StatusState::Failure),
            a::Check::Pending if combined != Some(d::StatusState::Failure) => combined = Some(d::StatusState::Pending),
            a::Check::Passed if combined.is_none() => combined = Some(d::StatusState::Success),
            a::Check::Pending | a::Check::Passed => {}
        }
        rows.push(d::CommitStatus {
            creator: if value.link.is_empty() { Some(setup.resolve_user(config, value.status.author)) } else { None },
            updated_at: Wall::from_nanos(value.status.at.as_nanos()),
            context: value.status.context.clone(),
            status: status(value.status.state),
            description: value.description.clone(),
            target_url: value.link.clone(),
        })
        .expect("sized statuses");
    }
    let sha = match request {
        Request::CombinedStatus { commit, .. } => Some(*commit),
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
        | Request::DeleteBranch { .. } => return Err(a::Error::Refused),
    };
    Ok(Answer::CombinedStatus {
        state: d::CombinedStatus { state: combined, total_count: u64::try_from(values.len()).expect("domain bound") },
        sha,
        statuses: rows.into_boxed(),
    })
}

/// A decoded read reply, naming users from setup and scenario configuration.
pub fn read(config: &Config, setup: &Store, request: &Request, answer: &a::Answer) -> Result<Answer, a::Error> {
    match answer {
        a::Answer::Settings(settings) => repository(config, request, settings),
        a::Answer::Items { items, .. } => {
            let mut out = List::with_capacity(u32::try_from(items.len()).expect("domain bound"));
            for value in items {
                out.push(issue(config, setup, value)).expect("sized items");
            }
            Ok(Answer::Issues(out.into_boxed()))
        }
        a::Answer::Item { item, .. } => Ok(Answer::Issue(issue(config, setup, item))),
        a::Answer::Comments(values) => {
            let mut out = List::with_capacity(u32::try_from(values.len()).expect("domain bound"));
            for value in values {
                out.push(comment(config, setup, value)).expect("sized thread");
            }
            Ok(Answer::Comments(out.into_boxed()))
        }
        a::Answer::Pull(value) => pull_answer(config, setup, request, value),
        a::Answer::Pulls(values) => {
            let id = repository_id(config, request).ok_or(a::Error::Missing(a::What::Repository))?;
            let mut out = List::with_capacity(u32::try_from(values.len()).expect("domain bound"));
            for value in values {
                out.push(pull(config, setup, id, value)).expect("sized pulls");
            }
            Ok(Answer::Pulls(out.into_boxed()))
        }
        a::Answer::Checks(values) => checks(config, setup, request, values),
        a::Answer::Protection(value) => {
            let document = optional_protection(value.as_ref());
            if crate::route(request) == crate::Route::Protections {
                Ok(Answer::Protections(match document {
                    Some(value) => Box::new([value]),
                    None => Box::from([]),
                }))
            } else {
                Ok(Answer::Protection(document.ok_or(a::Error::Missing(a::What::Branch))?))
            }
        }
        a::Answer::Collaborators(values) => collaborators(config, setup, request, values),
        a::Answer::Runs(values) => {
            let mut out = List::with_capacity(u32::try_from(values.len()).expect("domain bound"));
            for value in values {
                out.push(action_run(value)).expect("sized runs");
            }
            Ok(Answer::Runs { total_count: u64::try_from(values.len()).expect("domain bound"), runs: out.into_boxed() })
        }
        a::Answer::Run(value) => Ok(Answer::Run(action_run(value))),
        a::Answer::Job { job, steps } => Ok(job_answer(*job, steps)),
        a::Answer::Jobs(values) => {
            let mut out = List::with_capacity(u32::try_from(values.len()).expect("domain bound"));
            for value in values {
                out.push(d::Job {
                    id: value.id,
                    run_id: value.run,
                    attempt: value.attempt,
                    name: Box::new([]),
                    status: d::StepStatus::Unknown,
                })
                .expect("sized jobs");
            }
            Ok(Answer::Jobs(out.into_boxed()))
        }
        a::Answer::PullFiles { files: values, .. } => Ok(Answer::Files(files(values))),
        a::Answer::Comparison { files: values, commits, .. } => {
            let mut out = List::with_capacity(u32::try_from(commits.len()).expect("domain bound"));
            for value in commits {
                out.push(routes::object(*value)).expect("sized commits");
            }
            Ok(Answer::Compare {
                compare: d::Compare { total_commits: u64::try_from(commits.len()).expect("domain bound") },
                commits: out.into_boxed(),
                files: compared(request, values),
            })
        }
        a::Answer::Located { .. }
        | a::Answer::Permission(_)
        | a::Answer::Commit(_)
        | a::Answer::Cloned { .. }
        | a::Answer::Statuses(_)
        | a::Answer::Comment { .. }
        | a::Answer::Dependencies(_)
        | a::Answer::Labels(_)
        | a::Answer::Tree(_)
        | a::Answer::File(_)
        | a::Answer::Pages { .. }
        | a::Answer::Page(_)
        | a::Answer::Created(_)
        | a::Answer::Commented(_)
        | a::Answer::Reviewed(_)
        | a::Answer::Merged(_)
        | a::Answer::Revision(_)
        | a::Answer::Done
        | a::Answer::Pushed(_)
        | a::Answer::Branch(_) => Err(a::Error::Refused),
    }
}

fn wall(at: Option<skein_lib::Time>) -> Option<Wall> {
    let at = at?;
    Some(Wall::from_nanos(at.as_nanos()))
}
fn optional_object(at: Option<u64>) -> Option<skein_forge::forgejo::id::ObjectId> {
    let at = at?;
    Some(routes::object(at))
}
fn optional_protection(value: Option<&a::Protection>) -> Option<d::BranchProtection> {
    let value = value?;
    Some(protection(value))
}
fn collaborators(
    config: &Config,
    setup: &Store,
    request: &Request,
    values: &[a::Collaborator],
) -> Result<Answer, a::Error> {
    let mut out = List::with_capacity(config.max_response_items);
    let (at, page, limit) = match request {
        Request::Collaborators { at, page } => {
            let (page, limit) = routes::page(config, *page);
            (at, page, limit)
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
        | Request::Permission { .. }
        | Request::Pulls { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
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
        | Request::DeleteBranch { .. } => return Err(a::Error::Refused),
    };
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(limit));
    let mut seen = 0_u64;
    for value in values {
        // Organization ownership and site administration grant access, not collaborator membership.
        if !setup.lists_collaborator(config, at, value.user) {
            continue;
        }
        if seen >= skip && out.len() < limit {
            out.push(setup.resolve_user(config, value.user)).expect("page cap");
        }
        seen = seen.saturating_add(1);
    }
    Ok(Answer::Collaborators(out.into_boxed()))
}

/// A fake step's outcome in Forgejo's words.
#[must_use]
pub fn step(status: a::StepStatus) -> d::StepStatus {
    match status {
        a::StepStatus::Success => d::StepStatus::Success,
        a::StepStatus::Failure => d::StepStatus::Failure,
        a::StepStatus::Cancelled => d::StepStatus::Cancelled,
    }
}

fn job_answer(job: a::Job, steps: &[a::Step]) -> Answer {
    let mut out = List::with_capacity(u32::try_from(steps.len()).expect("domain bound"));
    for value in steps {
        out.push(d::Step { number: value.number, name: value.name.clone(), status: step(value.status) })
            .expect("sized steps");
    }
    Answer::Job {
        job: d::Job {
            id: job.id,
            run_id: job.run,
            attempt: job.attempt,
            name: Box::new([]),
            status: d::StepStatus::Unknown,
        },
        steps: out.into_boxed(),
    }
}

fn action_run(value: &a::Run) -> d::Run {
    d::Run {
        id: value.id,
        index_in_repo: value.number,
        commit_sha: routes::object(value.commit),
        workflow: Box::new([]),
        event: Box::new([]),
        trigger_actor: 0,
        status: d::StepStatus::Unknown,
        html_url: Box::new([]),
        created_at: Wall::EPOCH,
    }
}
