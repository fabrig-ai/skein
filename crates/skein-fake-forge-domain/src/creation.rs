//! Repository and contents creation in the shared git store (fake-forge.md, 4).
use crate::{
    Config, Domain, Signature,
    api::{Checks, Error, What},
    git::{self, Object},
    store::{Repository, fits},
};
use alloc::boxed::Box;
use skein_lib::{Duration, Env, Map, bytes::copy_of};

/// Makes a repository's first main commit, or leaves an existing name alone.
/// `owner` is the organization owner's account; the protocol keeps its namespace.
pub fn create_repository(
    domain: &mut Domain,
    config: &Config,
    id: u32,
    name: &[u8],
    repository: &[u8],
    owner: u64,
) -> Result<u64, Error> {
    if domain.names.contains_key(name) {
        return Err(Error::Exists);
    }
    let limits = &config.limits;
    fits(name, limits.name_bytes)?;
    let mut readme = skein_lib::Writer::new(repository.len().checked_add(3).ok_or(Error::TooLarge)?);
    readme.put(b"# ").expect("readme prefix");
    readme.put(repository).expect("readme name");
    readme.put(b"\n").expect("readme newline");
    let readme = readme.finish();
    fits(&readme, limits.content_bytes)?;
    fits(b"README.md", limits.name_bytes)?;
    fits(b"main", limits.name_bytes)?;
    if id == 0
        || limits.files == 0
        || limits.branches == 0
        || limits.users == 0
        || domain.repositories.len() == domain.repositories.capacity()
        || domain.commits.len() == domain.commits.capacity()
    {
        return Err(Error::Full);
    }
    for (_, existing) in &domain.names {
        if domain.repositories.get(*existing).expect("named repository").id == id {
            return Err(Error::Exists);
        }
    }
    let mut tree = Map::with_capacity(limits.files);
    tree.insert(copy_of(b"README.md"), readme).expect("room for readme");
    let first = git::store(
        domain,
        Object { parent: None, merge_parent: None, tree, message: Box::new([]), author: None, committer: None },
    )?;
    let checks = Checks {
        cues: Box::new([]),
        contexts: Box::new([]),
        latency_min: Duration::ZERO,
        latency_max: Duration::ZERO,
        silent: 0,
        passes: 1000,
        reruns: 0,
        cue: None,
    };
    let mut stored = Repository::new(limits, copy_of(name), copy_of(b"main"), first, checks, None, false);
    stored.id = id;
    stored.permissions.insert(owner, crate::api::Permission::Admin).expect("owner room");
    let slot = domain.repositories.insert(stored).expect("repository room checked");
    domain.names.insert(copy_of(name), slot).expect("one name per repository");
    Ok(first)
}

/// Commits an absent file on an existing branch and retains both signatures.
/// An existing path returns `Exists` without moving the branch or adding an object.
#[expect(clippy::too_many_arguments, reason = "the contents operation carries its branch, file, message and signature")]
pub fn create_file(
    domain: &mut Domain,
    env: &Env<Config>,
    repository: &[u8],
    branch: &[u8],
    path: &[u8],
    content: &[u8],
    message: &[u8],
    author: Signature,
    by: u64,
) -> Result<u64, Error> {
    let limits = &env.limits.limits;
    fits(path, limits.name_bytes)?;
    fits(content, limits.content_bytes)?;
    let message_bytes = limits.title_bytes.checked_add(limits.body_bytes).ok_or(Error::TooLarge)?;
    fits(message, message_bytes.checked_add(2).ok_or(Error::TooLarge)?)?;
    fits(&author.name, limits.name_bytes)?;
    fits(&author.email, limits.name_bytes)?;
    let id = *domain.names.get(repository).ok_or(Error::Missing(What::Repository))?;
    let stored = domain.repositories.get(id).expect("named repository");
    stored.require(by, crate::api::Permission::Write)?;
    let tip = *stored.branches.get(branch).ok_or(Error::Missing(What::Branch))?;
    let object = domain.commits.get(&tip).expect("branch commit");
    if object.tree.contains_key(path) {
        return Err(Error::Exists);
    }
    if object.tree.len() == object.tree.capacity() {
        return Err(Error::Full);
    }
    let mut tree = git::copy_tree(limits, &object.tree);
    tree.insert(copy_of(path), copy_of(content)).expect("file room checked");
    let commit = git::store(
        domain,
        Object {
            parent: Some(tip),
            merge_parent: None,
            tree,
            message: copy_of(message),
            committer: Some(author.clone()),
            author: Some(author),
        },
    )?;
    let stored = domain.repositories.get_mut(id).expect("named repository");
    stored.has.insert(commit).expect("room for every commit");
    stored.branches.insert(copy_of(branch), commit).expect("existing branch");
    git::moved(domain, env, id, branch, Some(tip), commit, by);
    Ok(commit)
}
