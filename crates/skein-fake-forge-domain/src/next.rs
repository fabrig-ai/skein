//! Reads added for the connector (protocol/forge.md, section 3.2).
//! Comparisons deliberately do not page; excessive data is an explicit
//! refusal, never a short answer claiming to be complete.

use crate::api::{Answer, ChangedFile, Check, CheckSummary, Collaborator, Error, Settings, State, What};
use crate::domain::Domain;
use crate::git;
use crate::limits::Limits;
use crate::pulls;
use crate::store::Repository;
use alloc::boxed::Box;
use skein_lib::bytes::copy_of;
use skein_lib::{Id, List, Map};

pub(crate) fn changed(
    domain: &Domain,
    limits: &Limits,
    base: Option<u64>,
    head: u64,
) -> Result<Box<[ChangedFile]>, Error> {
    let empty = Map::with_capacity(0);
    let before = match base {
        Some(base) => &domain.object(base).ok_or(Error::Missing(What::Commit))?.tree,
        None => &empty,
    };
    let after = &domain.object(head).ok_or(Error::Missing(What::Commit))?.tree;
    let mut files = List::with_capacity(limits.files);
    for (path, old) in before {
        let new = after.get(&**path);
        if new != Some(old) {
            if files.room() == 0 {
                return Err(Error::TooLarge);
            }
            #[expect(clippy::manual_map, reason = "programming-model.md forbids closure-taking Option::map")]
            let after = match new {
                Some(content) => Some(copy_of(content)),
                None => None,
            };
            files
                .push(ChangedFile { path: copy_of(path), before: Some(copy_of(old)), after })
                .expect("changed-file room checked before copying");
        }
    }
    for (path, new) in after {
        if !before.contains_key(&**path) {
            if files.room() == 0 {
                return Err(Error::TooLarge);
            }
            files
                .push(ChangedFile { path: copy_of(path), before: None, after: Some(copy_of(new)) })
                .expect("changed-file room checked before copying");
        }
    }
    Ok(files.into_boxed())
}

pub(crate) fn pull_files(
    domain: &Domain,
    limits: &Limits,
    repository: &Repository,
    number: u64,
    page: u32,
    limit: u32,
) -> Result<Answer, Error> {
    let item = repository.item(number)?;
    let pull = item.pull.as_ref().ok_or(Error::Missing(What::Pull))?;
    let onto = match item.state {
        State::Closed => match pull.merged {
            Some(merged) => domain.object(merged).expect("a stored merge").parent,
            None => repository.branches.get(&*pull.base).copied(),
        },
        State::Open => repository.branches.get(&*pull.base).copied(),
    }
    .ok_or(Error::Missing(What::Branch))?;
    let files = changed(domain, limits, git::merge_base(domain, onto, pull.commit), pull.commit)?;
    let size = if limit == 0 { limits.page_size } else { limit.min(limits.page_size) };
    let skip = u64::from(page.saturating_sub(1)).saturating_mul(u64::from(size));
    let mut answer = List::with_capacity(size);
    let mut more = false;
    for (index, file) in files.into_iter().enumerate() {
        if u64::try_from(index).expect("bounded file index") < skip {
            continue;
        }
        if answer.len() == size {
            more = true;
            break;
        }
        answer.push(file).expect("a page's room checked");
    }
    Ok(Answer::PullFiles { head: pull.commit, files: answer.into_boxed(), more })
}

pub(crate) fn compare(
    domain: &Domain,
    limits: &Limits,
    repository: &Repository,
    base: u64,
    head: u64,
) -> Result<Answer, Error> {
    if !repository.has.contains(&base) || !repository.has.contains(&head) {
        return Err(Error::Missing(What::Commit));
    }
    // Finish the merge-base walk before building the comparison histories,
    // keeping scratch within the two sets and queue in worst_case.
    let merge_base = git::merge_base(domain, base, head);
    let files = changed(domain, limits, Some(base), head)?;
    let old = git::ancestors(domain, base);
    let new = git::ancestors(domain, head);
    let mut commits = List::with_capacity(limits.commits);
    for &commit in &new {
        if !old.contains(&commit) {
            commits.push(commit).expect("one visit per commit");
        }
    }
    Ok(Answer::Comparison {
        merge_base,
        base,
        head,
        contains_base: git::is_ancestor(domain, base, head),
        files,
        commits: commits.into_boxed(),
    })
}

pub(crate) fn checks(repository: &Repository, limits: &Limits, commit: u64, ci_user: u64) -> Result<Answer, Error> {
    if !repository.has.contains(&commit) {
        return Err(Error::Missing(What::Commit));
    }
    let mut summaries = List::with_capacity(limits.contexts);
    for status in pulls::statuses(repository, limits, commit) {
        let description = copy_of(&status.description);
        let link = copy_of(&status.link);
        let job = if status.state == Check::Failed && status.author == ci_user {
            crate::actions::reference(repository, commit, &status.context)
        } else {
            None
        };
        summaries.push(CheckSummary { status, description, link, job }).expect("one summary per admitted context");
    }
    Ok(Answer::Checks(summaries.into_boxed()))
}

pub(crate) fn job(repository: &Repository, id: u64, ci_user: u64) -> Result<Answer, Error> {
    let (_, job) = crate::actions::latest(repository, id)?;
    let run = crate::actions::run(repository, job.run)?;
    if run.run.workflow.is_some() {
        return Ok(Answer::Job { steps: job.steps.clone(), job });
    }
    let (mut job, context, failing) = failed_job(repository, id, ci_user)?;
    job.steps = crate::actions::steps(context, failing);
    Ok(Answer::Job { steps: job.steps.clone(), job })
}

pub(crate) fn log(
    repository: &Repository,
    id: u64,
    attempt: u32,
    step: Option<u32>,
    ci_user: u64,
) -> Result<Answer, Error> {
    let (_, dispatched) = crate::actions::latest(repository, id)?;
    if crate::actions::run(repository, dispatched.run)?.run.workflow.is_some() {
        if dispatched.attempt != attempt {
            return Err(Error::Missing(What::Job));
        }
        let mut out = List::with_capacity(
            u32::try_from(dispatched.name.len()).expect("bounded job").saturating_mul(6).saturating_add(91),
        );
        for slice in &dispatched.steps {
            if step.is_none() || step == Some(slice.number) {
                for &byte in &slice.log {
                    out.push(byte).expect("bounded job log");
                }
            }
        }
        if step.is_some() && step > Some(2) {
            return Err(Error::Missing(What::Job));
        }
        return Ok(Answer::File(out.into_boxed()));
    }
    let (job, context, failing) = failed_job(repository, id, ci_user)?;
    if job.attempt != attempt {
        return Err(Error::Missing(What::Job));
    }
    Ok(Answer::File(crate::actions::log(context, failing, step)?))
}

fn failed_job(
    repository: &Repository,
    id: u64,
    ci_user: u64,
) -> Result<(crate::api::Job, &[u8], crate::api::Failing), Error> {
    let (commit, job) = crate::actions::latest(repository, id)?;
    let index = usize::try_from(job.index).expect("a bounded context");
    let context = repository.checks.contexts.get(index).expect("a configured context");
    let status = repository
        .statuses
        .get(&commit)
        .ok_or(Error::Missing(What::Job))?
        .get(context.as_ref())
        .ok_or(Error::Missing(What::Job))?;
    if status.author != ci_user || status.state != Check::Failed {
        return Err(Error::Missing(What::Job));
    }
    Ok((job, context, *repository.failing.get(index).expect("a configured context")))
}

pub(crate) fn settings(repository: &Repository) -> Answer {
    Answer::Settings(Settings {
        default: copy_of(&repository.default),
        merge: repository.styles.merge,
        rebase: repository.styles.rebase,
        squash: repository.styles.squash,
    })
}

pub(crate) fn collaborators(repository: &Repository) -> Answer {
    let mut users = List::with_capacity(repository.permissions.len());
    for (&user, &permission) in &repository.permissions {
        users.push(Collaborator { user, permission }).expect("one entry per collaborator");
    }
    Answer::Collaborators(users.into_boxed())
}

pub(crate) fn update(
    domain: &mut Domain,
    env: &skein_lib::Env<crate::Config>,
    id: Id<Repository>,
    user: u64,
    number: u64,
    style: crate::api::UpdateStyle,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get(id).expect("a repository of the forge");
    repository.require(user, crate::api::Permission::Write)?;
    let enabled = match style {
        crate::api::UpdateStyle::Merge => repository.styles.merge,
        crate::api::UpdateStyle::Rebase => repository.styles.rebase,
    };
    if !enabled {
        return Err(Error::Refused);
    }
    let item = repository.item(number)?;
    if item.state == State::Closed {
        return Err(Error::Closed);
    }
    let pull = item.pull.as_ref().ok_or(Error::Missing(What::Pull))?;
    let onto = *repository.branches.get(&*pull.base).ok_or(Error::Missing(What::Branch))?;
    let head = pull.commit;
    if git::is_ancestor(domain, onto, head) {
        return Ok(Answer::Done);
    }
    let branch = copy_of(&pull.head);
    if repository.is_protected(&branch) {
        return Err(Error::Protected);
    }
    let merged = match style {
        crate::api::UpdateStyle::Merge => {
            let tree = pulls::merged(domain, &env.limits.limits, onto, head)?;
            git::store(
                domain,
                crate::Object {
                    parent: Some(head),
                    merge_parent: Some(onto),
                    tree,
                    message: Box::new([]),
                    author: None,
                    committer: None,
                },
            )?
        }
        crate::api::UpdateStyle::Rebase => rebase(domain, &env.limits.limits, onto, head)?,
    };
    let history = git::ancestors(domain, merged);
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    for &commit in &history {
        repository.has.insert(commit).expect("repository can retain every commit");
    }
    repository.branches.insert(copy_of(&branch), merged).expect("the head branch exists");
    match style {
        crate::api::UpdateStyle::Rebase => domain.observations.push(crate::Observation::Rebased {
            repository: copy_of(&repository.name),
            branch: copy_of(&branch),
            from: head,
            to: merged,
            by: user,
        }),
        crate::api::UpdateStyle::Merge => {}
    }
    git::moved(domain, env, id, &branch, Some(head), merged, user);
    Ok(Answer::Done)
}

/// Validate every replay before storing any commit, so a conflict or full store changes nothing.
fn rebase(domain: &mut Domain, limits: &Limits, onto: u64, head: u64) -> Result<u64, Error> {
    let base_history = git::ancestors(domain, onto);
    let history = git::ancestors(domain, head);
    let mut commits = List::with_capacity(limits.commits);
    for &commit in &history {
        if !base_history.contains(&commit) {
            commits.push(commit).expect("bounded history");
        }
    }
    if commits.len() > domain.commits.capacity().saturating_sub(domain.commits.len()) {
        return Err(Error::Full);
    }
    let empty = Map::with_capacity(0);
    let mut tree = git::copy_tree(limits, &domain.object(onto).expect("base tip").tree);
    for &commit in &commits {
        let object = domain.object(commit).expect("branch commit");
        let ancestor = match object.parent {
            Some(parent) => &domain.object(parent).expect("parent").tree,
            None => &empty,
        };
        tree = pulls::merge_trees(limits, ancestor, &tree, &object.tree)?;
    }
    drop(tree);
    let mut parent = onto;
    for &commit in &commits {
        let object = domain.object(commit).expect("branch commit");
        let ancestor = match object.parent {
            Some(parent) => &domain.object(parent).expect("parent").tree,
            None => &empty,
        };
        let tree =
            pulls::merge_trees(limits, ancestor, &domain.object(parent).expect("replayed parent").tree, &object.tree)
                .expect("every patch validated");
        let message = copy_of(&object.message);
        let author = object.author.clone();
        let committer = object.committer.clone();
        parent = git::store(
            domain,
            crate::Object { parent: Some(parent), merge_parent: None, tree, message, author, committer },
        )
        .expect("all commit slots admitted");
    }
    Ok(parent)
}
