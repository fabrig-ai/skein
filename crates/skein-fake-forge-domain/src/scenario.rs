//! What a world does to the forge from outside its API (testing.md,
//! 5.1): it sets up repositories and their users, makes a repository
//! unreachable or refusing, commits as a working tree's git does, and has
//! another party advance a branch.
//!
//! Setting up is not observed and starts no CI; what another party does is
//! observed, heard and checked as any change is.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Duration, Env};

use crate::api::{Cue, Error, File, Permission, Setup, What};
use crate::domain::{Config, Domain};
use crate::git::{self, Object};
use crate::store::{Repository, fit_names, fit_numbers, fits};

/// Adds the repository `setup` describes, its default branch at a first
/// commit of its tree, and returns that commit.
pub fn repository(domain: &mut Domain, config: &Config, setup: Setup) -> u64 {
    let limits = &config.limits;
    let Setup { id, name, default, tree, labels, checks, workflows, protection, hooked } = setup;
    assert_ne!(id, 0, "a repository's forge id is nonzero");
    for (_, existing) in &domain.names {
        let repository = domain.repositories.get(*existing).expect("a named repository");
        assert_ne!(repository.id, id, "a repository's forge id is unique");
    }
    assert!(!domain.names.contains_key(&*name), "a repository is added once");
    fits(&name, limits.name_bytes).expect("a repository's name within the limits");
    fits(&default, limits.name_bytes).expect("a branch name within the limits");
    fit_names(&labels, limits.labels, limits).expect("labels within the limits");
    fit_names(&checks.contexts, limits.contexts, limits).expect("contexts within the limits");
    assert!(checks.latency_min <= checks.latency_max, "CI's latency is a range");
    if let Some(cue) = &checks.cue {
        validate_cue(cue, limits);
    }
    assert!(checks.cues.len() <= checks.contexts.len(), "at most one cue per context");
    for (index, entry) in checks.cues.iter().enumerate() {
        assert!(checks.contexts.contains(&entry.context), "a cue names a configured context");
        for previous in checks.cues.iter().take(index) {
            assert_ne!(previous.context, entry.context, "one cue per context");
        }
        validate_cue(&entry.cue, limits);
    }
    if let Some(protection) = &protection {
        fit_numbers(&protection.bypassing_users, limits.bypassing_users).expect("bypassing users within the limits");
        fits(&protection.branch, limits.name_bytes).expect("a branch name within the limits");
        fit_names(&protection.contexts, limits.contexts, limits).expect("contexts within the limits");
    }
    assert!(
        workflows.len() <= usize::try_from(limits.workflows).expect("workflow bound fits"),
        "workflows within the limits"
    );
    let mut workflow_files = skein_lib::Set::with_capacity(limits.workflows);
    for workflow in &workflows {
        fits(&workflow.file, limits.name_bytes).expect("workflow file within the limits");
        assert!(workflow_files.insert(copy_of(&workflow.file)).expect("bounded workflows"), "unique workflow files");
        fit_names(&workflow.jobs, limits.contexts, limits).expect("workflow jobs within the limits");
        assert!(workflow.latency_min <= workflow.latency_max, "workflow latency is a range");
        let mut jobs = skein_lib::Set::with_capacity(limits.contexts);
        for job in &workflow.jobs {
            let length = workflow
                .file
                .len()
                .checked_add(3)
                .expect("bounded context")
                .checked_add(job.len())
                .expect("bounded context");
            assert!(
                u32::try_from(length).expect("bounded context") <= limits.name_bytes,
                "workflow status context fits"
            );
            assert!(jobs.insert(copy_of(job)).expect("bounded jobs"), "unique job names");
        }
        if let Some(cue) = &workflow.cue {
            validate_cue(cue, limits);
        }
    }
    let tree = git::tree(limits, tree).expect("a first tree within the limits");
    let first = git::store(
        domain,
        Object { parent: None, merge_parent: None, tree, message: Box::new([]), author: None, committer: None },
    )
    .expect("room for a first commit");
    let mut repository = Repository::new(limits, copy_of(&name), default, first, checks, protection, hooked);
    repository.id = id;
    repository.workflows = workflows;
    for label in labels {
        if !repository.labels.contains_key(&*label) {
            repository.define_label(label).expect("a repository's labels are within the limits");
        }
    }
    let id = domain.repositories.insert(repository).expect("a repository's room");
    domain.names.insert(name, id).expect("as many names as repositories");
    first
}

/// Adds an unborn repository: its default has no branch or commit yet.
pub fn unborn_repository(domain: &mut Domain, config: &Config, setup: Setup) {
    let name = setup.name.clone();
    let made = domain.made;
    let first = repository(domain, config, setup);
    let id = domain.id(&name);
    let repository = domain.repositories.get_mut(id).expect("the repository just added");
    assert_eq!(repository.branches.remove(&*repository.default), Some(first), "fresh first branch");
    repository.has.remove(&first);
    drop(domain.commits.remove(&first));
    domain.made = made;
}

/// Gives `user` the permission `permission` on `repository`.
pub fn grant(domain: &mut Domain, repository: &[u8], user: u64, permission: Permission) {
    let id = domain.id(repository);
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.permissions.insert(user, permission).expect("a repository's users are within the limits");
}

/// Set repository metadata in a world's initial setup. Its default must
/// name an existing branch; these booleans model provider merge settings.
pub fn settings(domain: &mut Domain, repository: &[u8], settings: crate::api::Settings) {
    let id = domain.id(repository);
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    assert!(repository.branches.contains_key(&*settings.default), "the default branch exists");
    repository.default = settings.default;
    repository.styles =
        crate::store::Styles { merge: settings.merge, rebase: settings.rebase, squash: settings.squash };
}

/// Makes `repository` reachable by git, or not.
pub fn set_reachable(domain: &mut Domain, repository: &[u8], reachable: bool) {
    let id = domain.id(repository);
    domain.repositories.get_mut(id).expect("a repository of the forge").reachable = reachable;
}

/// Makes `repository` refuse what is pushed to it, branches created included,
/// or not.
pub fn set_refusing(domain: &mut Domain, repository: &[u8], refusing: bool) {
    let id = domain.id(repository);
    domain.repositories.get_mut(id).expect("a repository of the forge").refusing = refusing;
}

/// What a working tree's git does when it commits: names a commit of `tree`
/// on `parent` in the forge's one store, where no repository has it until it
/// is pushed. Returns `None` if the tree is `parent`'s, and refuses a tree
/// past the limits, or a store that is full. The opaque message fits the
/// fixture's commit message allowance.
pub fn commit(
    domain: &mut Domain,
    config: &Config,
    parent: u64,
    tree: Box<[File]>,
    message: &[u8],
) -> Result<Option<u64>, Error> {
    let tree = git::tree(&config.limits, tree)?;
    let Some(object) = domain.commits.get(&parent) else {
        return Err(Error::Missing(What::Commit));
    };
    if git::same(&object.tree, &tree) {
        return Ok(None);
    }
    fits(message, message_limit(&config.limits)?)?;
    let commit = git::store(
        domain,
        Object {
            parent: Some(parent),
            merge_parent: None,
            tree,
            message: copy_of(message),
            author: None,
            committer: None,
        },
    )?;
    Ok(Some(commit))
}

/// A worker commits a resolved merge, with both fetched parents. Unlike an
/// ordinary commit, an unchanged tree still records the merge ancestry and
/// the opaque message.
pub fn merge_commit(
    domain: &mut Domain,
    config: &Config,
    parent: u64,
    merge_parent: u64,
    files: Box<[File]>,
    message: &[u8],
) -> Result<u64, Error> {
    if !domain.commits.contains_key(&parent) || !domain.commits.contains_key(&merge_parent) {
        return Err(Error::Missing(What::Commit));
    }
    let tree = git::tree(&config.limits, files)?;
    fits(message, message_limit(&config.limits)?)?;
    git::store(
        domain,
        Object {
            parent: Some(parent),
            merge_parent: Some(merge_parent),
            tree,
            message: copy_of(message),
            author: None,
            committer: None,
        },
    )
}

fn message_limit(limits: &crate::limits::Limits) -> Result<u32, Error> {
    let bytes = limits.title_bytes.checked_add(limits.body_bytes).ok_or(Error::TooLarge)?;
    bytes.checked_add(2).ok_or(Error::TooLarge)
}

/// Another party commits on `branch` of `repository`, writing `path` with
/// `content`, and moves the branch to it, as a push of its own would, past
/// any protection, as the user `by`. Returns the commit, or refuses what is
/// past the limits, as a store or a tree that is full.
pub fn advance(
    domain: &mut Domain,
    env: &Env<Config>,
    repository: &[u8],
    branch: &[u8],
    path: &[u8],
    content: &[u8],
    by: u64,
) -> Result<u64, Error> {
    let limits = &env.limits.limits;
    fits(path, limits.name_bytes)?;
    fits(content, limits.content_bytes)?;
    let id = domain.id(repository);
    let tip = *domain.repositories.get(id).expect("a repository of the forge").branches.get(branch).expect("a branch");
    let mut tree = git::copy_tree(limits, &domain.commits.get(&tip).expect("a commit of the store").tree);
    if !tree.contains_key(path) && tree.len() >= tree.capacity() {
        return Err(Error::TooLarge);
    }
    tree.insert(copy_of(path), copy_of(content)).expect("checked for room above");
    let commit = git::store(
        domain,
        Object { parent: Some(tip), merge_parent: None, tree, message: Box::new([]), author: None, committer: None },
    )?;
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    repository.has.insert(commit).expect("a repository has room for every commit");
    repository.branches.insert(copy_of(branch), commit).expect("the branch is there");
    git::moved(domain, env, id, branch, Some(tip), commit, by);
    Ok(commit)
}

/// Git's commit-tree stores even an unchanged tree, without moving a branch.
pub fn commit_tree(
    domain: &mut Domain,
    config: &Config,
    parent: u64,
    merging: Option<u64>,
    files: Box<[File]>,
    message: &[u8],
) -> Result<u64, Error> {
    if !domain.commits.contains_key(&parent) || !domain.commits.contains_key(&merging.unwrap_or(parent)) {
        return Err(Error::Missing(What::Commit));
    }
    let tree = git::tree(&config.limits, files)?;
    fits(message, message_limit(&config.limits)?)?;
    git::store(
        domain,
        Object {
            parent: Some(parent),
            merge_parent: merging,
            tree,
            message: copy_of(message),
            author: None,
            committer: None,
        },
    )
}

/// Sets where the context's failed jobs fail in the repository.
pub fn set_failing(domain: &mut Domain, repository: &[u8], context: &[u8], failing: crate::api::Failing) {
    let id = domain.id(repository);
    let repository = domain.repositories.get_mut(id).expect("a repository of the forge");
    let mut found = None;
    for (index, name) in repository.checks.contexts.iter().enumerate() {
        if name.as_ref() == context {
            found = Some(index);
            break;
        }
    }
    *repository.failing.get_mut(found.expect("a context of the repository's checks")).expect("one entry per context") =
        failing;
}

/// Starts checking the pull's mergeability for `span` on the forge's clock.
pub fn check(domain: &mut Domain, env: &Env<Config>, repository: &[u8], number: u64, span: Duration) {
    let id = *domain.names.get(repository).expect("a scenario repository");
    let repository = domain.repositories.get_mut(id).expect("a scenario repository");
    let item = repository.items.get_mut(&number).expect("a scenario item");
    item.pull.as_mut().expect("a scenario pull").checked = crate::domain::clock(env).saturating_add(span);
}

fn validate_cue(cue: &Cue, limits: &crate::Limits) {
    match cue {
        Cue::PullFile { path, no_pull, step, log } => {
            assert_ne!(*no_pull, crate::api::Check::Pending, "a push cue has a verdict");
            assert!(crate::ci::template_bound(path) <= limits.name_bytes, "expanded path within the limits");
            fits(step, limits.name_bytes).expect("a step name within the limits");
            assert!(crate::ci::template_bound(log) <= limits.content_bytes, "expanded log within the limits");
        }
        Cue::File { path, green } => {
            fits(path, limits.name_bytes).expect("a path within the limits");
            fits(green, limits.content_bytes).expect("content within the limits");
        }
        Cue::Message { fail } => {
            fits(fail, limits.content_bytes).expect("a cue within the limits");
        }
    }
}
