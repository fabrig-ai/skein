//! Groups follow fake-forge.md, 10.2: one squash per entry, on the group
//! ahead, with CI on a counted branch and landing in admission order.
use alloc::boxed::Box;
use skein_lib::{Decimal, Env, Id, Writer, bytes::copy_of};

use crate::{
    api::{Answer, Change, Check, Error, Permission, QueueEntry, QueueRemoval, State, What},
    domain::{self, Alarm, Config, Domain},
    git::{self, Object},
    hooks::Hook,
    observe::{Observation, Operation},
    pulls,
    store::{Repository, fits},
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Group {
    pub(crate) commit: u64,
    /// The first settled group verdict survives CI reruns while waiting.
    verdict: Check,
}

impl Group {
    #[expect(clippy::manual_map, reason = "step code uses matches rather than Option::map")]
    pub(crate) fn count(group: Option<Self>) -> Option<u64> {
        match group {
            Some(group) => Some(group.commit),
            None => None,
        }
    }
}

/// fake-forge.md, 10.2: the branch names the head in the shared SHA-1 spelling.
fn branch(base: &[u8], entry: QueueEntry, limit: u32) -> Result<Box<[u8]>, Error> {
    let number = Decimal::of(entry.number);
    let size = b"gh-readonly-queue/".len().checked_add(base.len()).ok_or(Error::TooLarge)?;
    let size = size.checked_add(b"/pr-".len()).ok_or(Error::TooLarge)?;
    let size = size.checked_add(number.as_bytes().len()).ok_or(Error::TooLarge)?;
    let size = size.checked_add(41).ok_or(Error::TooLarge)?;
    if u64::try_from(size).expect("branch size fits") > u64::from(limit) {
        return Err(Error::TooLarge);
    }
    let mut out = Writer::new(size);
    for part in [b"gh-readonly-queue/".as_slice(), base, b"/pr-", number.as_bytes(), b"-"] {
        out.put(part).expect("measured group branch");
    }
    for byte in crate::object_bytes(entry.head) {
        for nibble in [byte.checked_shr(4).expect("four-bit shift"), byte & 15] {
            let digit =
                if nibble < 10 { b'0'.saturating_add(nibble) } else { b'a'.saturating_add(nibble.saturating_sub(10)) };
            out.put(&[digit]).expect("measured SHA-1");
        }
    }
    Ok(out.finish())
}

fn onto(domain: &Domain, id: Id<Repository>, entry: QueueEntry) -> Result<Option<u64>, Error> {
    let repository = domain.repositories.get(id).expect("queue repository");
    let pull = repository.item(entry.number)?.pull.as_ref().expect("queued pull");
    let mut parent = repository.branches.get(&*pull.base).copied();
    for ahead in &repository.queue {
        if ahead.number == entry.number {
            break;
        }
        let prior = repository.item(ahead.number)?.pull.as_ref().expect("queued pull");
        if prior.base == pull.base {
            parent = Group::count(prior.group);
        }
    }
    Ok(parent)
}

fn preflight(domain: &Domain, env: &Env<Config>, id: Id<Repository>, entry: QueueEntry) -> Result<(), Error> {
    let repository = domain.repositories.get(id).expect("queue repository");
    let pull = repository.item(entry.number)?.pull.as_ref().expect("queued pull");
    let name = branch(&pull.base, entry, env.limits.limits.name_bytes)?;
    fits(&name, env.limits.limits.name_bytes)?;
    if repository.branches.contains_key(&*name) {
        return Err(Error::Exists);
    }
    if repository.branches.len() == repository.branches.capacity() || domain.room().commits == 0 {
        return Err(Error::Full);
    }
    Ok(())
}

pub(crate) fn enqueue(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    by: u64,
    number: u64,
    head: u64,
) -> Result<Answer, Error> {
    let repository = domain.repositories.get(id).expect("queue repository");
    repository.require(by, Permission::Write)?;
    let item = repository.item(number)?;
    let pull = item.pull.as_ref().ok_or(Error::Missing(What::Pull))?;
    if item.state == State::Closed {
        return Err(Error::Closed);
    }
    if pull.commit != head {
        return Err(Error::Stale);
    }
    for entry in &repository.queue {
        if entry.number == number {
            return Err(Error::AlreadyQueued(*entry));
        }
    }
    if let Some(protection) = &repository.protection
        && protection.branch == pull.base
        // Inferred: a user's merge bypass also admits it without approvals;
        // group checks run after admission on the queue's commit.
        && !protection.bypassing_users.contains(&by)
        && !pulls::approved(item, pull, protection)
    {
        return Err(Error::Protected);
    }
    let entry = QueueEntry { number, head, position: repository.queue.len().checked_add(1).ok_or(Error::Full)? };
    if repository.queue.room() == 0 {
        return Err(Error::Full);
    }
    preflight(domain, env, id, entry)?;
    // Reject oversized trees before admission. Conflicts are queue removals.
    if let Some(parent) = onto(domain, id, entry)? {
        match pulls::merged(domain, &env.limits.limits, head, parent) {
            Ok(_) | Err(Error::Conflict) => {}
            Err(error) => return Err(error),
        }
    }
    let repository = domain.repositories.get_mut(id).expect("queue repository");
    repository.queue.push(entry).expect("admission room checked");
    let pull = repository.item_mut(number)?.pull.as_mut().expect("queued pull");
    pull.dequeued = None;
    pull.queued_by = by;
    repository.touch(number, domain::clock(env));
    let observation =
        Observation::Enqueued { repository: copy_of(&repository.name), number, head, position: entry.position, by };
    domain::changed(domain, env, id, observation, Hook::queued(number, by, crate::api::QueueHook::Enqueued));
    rebuild(domain, env, id);
    domain.timers.arm(Alarm::Queue(id), env.now).expect("one queue wake per repository");
    pulls::view(domain, &env.limits.limits, domain.repositories.get(id).expect("queue repository"), number)
}

fn make(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    entry: QueueEntry,
    parent: u64,
) -> Result<(), Error> {
    preflight(domain, env, id, entry)?;
    let tree = pulls::merged(domain, &env.limits.limits, entry.head, parent)?;
    let repository = domain.repositories.get(id).expect("queue repository");
    let pull = repository.item(entry.number)?.pull.as_ref().expect("queued pull");
    let name = branch(&pull.base, entry, env.limits.limits.name_bytes)?;
    let by = pull.queued_by;
    // fake-forge.md, 10.2: COMMIT_OR_PR_TITLE uses the sole commit's
    // message, otherwise the pull's title, relative to the pull's base.
    let base = repository.branches.get(&*pull.base).copied().ok_or(Error::Missing(What::Branch))?;
    let head = domain.commits.get(&entry.head).expect("queued head");
    let sole = match head.parent {
        Some(parent) => git::is_ancestor(domain, parent, base),
        None => false,
    } && match head.merge_parent {
        Some(parent) => git::is_ancestor(domain, parent, base),
        None => true,
    };
    let message = if sole { copy_of(&head.message) } else { copy_of(&repository.item(entry.number)?.title) };
    let commit = git::store(
        domain,
        Object { parent: Some(parent), merge_parent: None, tree, message, author: None, committer: None },
    )?;
    let repository = domain.repositories.get_mut(id).expect("queue repository");
    repository.has.insert(commit).expect("room for every store commit");
    repository.branches.insert(copy_of(&name), commit).expect("group branch room checked");
    repository.item_mut(entry.number)?.pull.as_mut().expect("queued pull").group =
        Some(Group { commit, verdict: Check::Pending });
    group_changed(domain, env, id, entry.number, commit, false, false, by);
    git::moved(domain, env, id, &name, None, commit, by);
    Ok(())
}

#[expect(clippy::too_many_arguments, reason = "a group change carries its identity and outcome")]
fn group_changed(
    domain: &mut Domain,
    env: &Env<Config>,
    id: Id<Repository>,
    number: u64,
    commit: u64,
    ended: bool,
    landed: bool,
    by: u64,
) {
    let repository = domain.repositories.get(id).expect("queue repository");
    let observation = Observation::Group { repository: copy_of(&repository.name), number, commit, ended, landed };
    domain::changed(domain, env, id, observation, Hook::group(commit, by, ended));
}

fn end_group(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, entry: QueueEntry, landed: bool) {
    let repository = domain.repositories.get_mut(id).expect("queue repository");
    let pull = repository.item_mut(entry.number).expect("queued item").pull.as_mut().expect("queued pull");
    let Some(group) = pull.group.take() else {
        return;
    };
    let by = pull.queued_by;
    let name = branch(&pull.base, entry, env.limits.limits.name_bytes).expect("admitted group name fits");
    repository.branches.remove(&*name);
    let observation =
        Observation::Deleted { repository: copy_of(&repository.name), branch: copy_of(&name), at: group.commit, by };
    domain::changed(domain, env, id, observation, Hook::push(&name, None, by));
    group_changed(domain, env, id, entry.number, group.commit, true, landed, by);
}

fn remove(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, index: u32, reason: QueueRemoval) {
    let repository = domain.repositories.get(id).expect("queue repository");
    let entry = *repository.queue.get(index).expect("queued entry");
    let base = copy_of(&repository.item(entry.number).expect("queued item").pull.as_ref().expect("queued pull").base);
    end_group(domain, env, id, entry, false);
    let repository = domain.repositories.get_mut(id).expect("queue repository");
    erase(repository, index);
    let pull = repository.item_mut(entry.number).expect("queued item").pull.as_mut().expect("queued pull");
    pull.dequeued = Some(reason);
    let by = pull.queued_by;
    repository.touch(entry.number, domain::clock(env));
    let observation = Observation::Dequeued { repository: copy_of(&repository.name), number: entry.number, reason };
    domain::changed(
        domain,
        env,
        id,
        observation,
        Hook::queued(entry.number, by, crate::api::QueueHook::Dequeued(reason)),
    );
    // Every dependent group is destroyed before any new CI starts.
    let count = domain.repositories.get(id).expect("queue repository").queue.len();
    for next in index..count {
        let repository = domain.repositories.get(id).expect("queue repository");
        let entry = *repository.queue.get(next).expect("queued entry");
        let pull = repository.item(entry.number).expect("queued item").pull.as_ref().expect("queued pull");
        if pull.base == base {
            end_group(domain, env, id, entry, false);
        }
    }
    positions(domain, id);
}

fn positions(domain: &mut Domain, id: Id<Repository>) {
    let repository = domain.repositories.get_mut(id).expect("queue repository");
    for index in 0..repository.queue.len() {
        repository.queue.get_mut(index).expect("queued entry").position =
            index.checked_add(1).expect("bounded position");
    }
}

fn rebuild(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>) {
    let count = domain.repositories.get(id).expect("queue repository").queue.len();
    let mut index = 0;
    for _ in 0..count {
        let repository = domain.repositories.get(id).expect("queue repository");
        let Some(&entry) = repository.queue.get(index) else {
            break;
        };
        let pull = repository.item(entry.number).expect("queued item").pull.as_ref().expect("queued pull");
        let by = pull.queued_by;
        if pull.group.is_none()
            && let Ok(Some(parent)) = onto(domain, id, entry)
        {
            match make(domain, env, id, entry, parent) {
                Ok(()) => {}
                Err(Error::Conflict) => {
                    remove(domain, env, id, index, QueueRemoval::MergeConflict);
                    continue;
                }
                Err(error) => {
                    // The fake's choice, not GitHub's: no reservation. A full
                    // store or oversized rebuilt tree waits for the next queue
                    // event, with the refusal visible to the world's referee.
                    if error == Error::Full || error == Error::TooLarge {
                        domain.tally.full = domain.tally.full.saturating_add(1);
                    }
                    let repository = domain.repositories.get(id).expect("queue repository");
                    domain.observations.push(Observation::Refused {
                        repository: copy_of(&repository.name),
                        what: Operation::Enqueue,
                        number: Some(entry.number),
                        commit: Some(entry.head),
                        error,
                        by,
                    });
                }
            }
        }
        index = index.saturating_add(1);
    }
}

/// CI calls this after its verdict, before any rerun can fire.
pub(crate) fn verdict(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, commit: u64) {
    let repository = domain.repositories.get(id).expect("queue repository");
    for entry in &repository.queue {
        let pull = repository.item(entry.number).expect("queued item").pull.as_ref().expect("queued pull");
        if let Some(group) = pull.group
            && group.commit == commit
        {
            process(domain, env, id);
            return;
        }
    }
}

fn settled(repository: &Repository, number: u64, commit: u64) -> Check {
    let pull = repository.item(number).expect("queued item").pull.as_ref().expect("queued pull");
    let Some(protection) = &repository.protection else {
        return Check::Passed;
    };
    if protection.branch != pull.base {
        return Check::Passed;
    }
    let mut pending = false;
    for context in &protection.contexts {
        let state = match repository.statuses.get(&commit) {
            Some(statuses) => match statuses.get(&**context) {
                Some(status) => status.state,
                None => Check::Pending,
            },
            None => Check::Pending,
        };
        match state {
            Check::Failed => return Check::Failed,
            Check::Pending => pending = true,
            Check::Passed | Check::Skipped => {}
        }
    }
    if pending { Check::Pending } else { Check::Passed }
}

pub(crate) fn process(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>) {
    let capacity = domain.repositories.get(id).expect("queue repository").queue.capacity();
    // Each pass removes an entry, or returns. Each rebuild visits at most
    // limits.items entries; groups use the existing branch and commit caps.
    for _ in 0..capacity {
        rebuild(domain, env, id);
        let count = domain.repositories.get(id).expect("queue repository").queue.len();
        let mut changed = false;
        for index in 0..count {
            let repository = domain.repositories.get(id).expect("queue repository");
            let entry = *repository.queue.get(index).expect("queued entry");
            let Some(mut group) =
                repository.item(entry.number).expect("queued item").pull.as_ref().expect("queued pull").group
            else {
                continue;
            };
            if group.verdict == Check::Pending {
                group.verdict = settled(repository, entry.number, group.commit);
                domain
                    .repositories
                    .get_mut(id)
                    .expect("queue repository")
                    .item_mut(entry.number)
                    .expect("queued item")
                    .pull
                    .as_mut()
                    .expect("queued pull")
                    .group = Some(group);
            }
            match group.verdict {
                Check::Failed => {
                    remove(domain, env, id, index, QueueRemoval::CiFailure);
                    changed = true;
                    break;
                }
                Check::Passed if index == 0 => {
                    land(domain, env, id, entry, group.commit);
                    changed = true;
                    break;
                }
                Check::Passed | Check::Pending | Check::Skipped => {}
            }
        }
        if !changed {
            return;
        }
    }
}

fn land(domain: &mut Domain, env: &Env<Config>, id: Id<Repository>, entry: QueueEntry, commit: u64) {
    let repository = domain.repositories.get_mut(id).expect("queue repository");
    let item = repository.item_mut(entry.number).expect("queued item");
    item.state = State::Closed;
    let pull = item.pull.as_mut().expect("queued pull");
    pull.merged = Some(commit);
    pull.queue_merged = true;
    let base = copy_of(&pull.base);
    let by = pull.queued_by;
    let from = repository.branches.insert(copy_of(&base), commit).expect("base branch exists");
    repository.touch(entry.number, domain::clock(env));
    let observation = Observation::Merged {
        repository: copy_of(&repository.name),
        number: entry.number,
        base: copy_of(&base),
        head: entry.head,
        commit,
        by,
    };
    domain::changed(domain, env, id, observation, Hook::item(Change::Pull, entry.number, by));
    git::moved(domain, env, id, &base, from, commit, by);
    end_group(domain, env, id, entry, true);
    erase(domain.repositories.get_mut(id).expect("queue repository"), 0);
    positions(domain, id);
}

/// Shift in place: the queue keeps its startup allocation.
fn erase(repository: &mut Repository, index: u32) {
    let last = repository.queue.len().checked_sub(1).expect("nonempty queue");
    for current in index..last {
        let next = *repository.queue.get(current.checked_add(1).expect("bounded index")).expect("queued entry");
        *repository.queue.get_mut(current).expect("queued entry") = next;
    }
    repository.queue.truncate(last);
}
