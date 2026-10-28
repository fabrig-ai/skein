//! What the forge holds at most (programming-model.md, section 7), and the
//! memory that takes (6.3).

use alloc::boxed::Box;
use core::mem::size_of;

use skein_lib::{Deadlines, Id, List, Map, Queue, Set, Slab, Time};

use crate::actions::{Failing, Job, Kept as RunKept, Run, Step};
use crate::api::{
    ChangedFile, CheckSummary, Collaborator, Comment, ContextCue, File, Head, PageName, Permission, Pull, Review,
    Status as StatusView, Summary, TreeEntry, Workflow,
};
use crate::domain::{Alarm, Call};
use crate::faults::Window;
use crate::git::Object;
use crate::hooks::Delivery;
use crate::observe::Observation;
use crate::store::{Comment as Posted, Item, Kept, Page, Repository, Status};

/// The forge's limits, part of its configuration. What would pass one is
/// refused: a name, title, body or content too long as too large, and one
/// more of anything a repository holds as full.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Repositories on the forge.
    pub repositories: u32,
    /// Users with a permission on a repository, and users calling within a
    /// rate window.
    pub users: u32,
    /// Bypassing users in the repository's one protection rule.
    pub bypassing_users: u32,
    /// Labels defined in a repository.
    pub labels: u32,
    /// Items in a repository; comments on an item, and items it depends on.
    pub items: u32,
    pub comments: u32,
    pub dependencies: u32,
    /// Reviews of a pull request.
    pub reviews: u32,
    /// Branches of a repository.
    pub branches: u32,
    /// Commits in the forge's store, and files in a commit's tree.
    pub commits: u32,
    pub files: u32,
    /// Commits of a repository with statuses, and contexts on each.
    pub statuses: u32,
    pub contexts: u32,
    /// Dispatch-only workflow definitions and retained dispatched runs per repository.
    pub workflows: u32,
    pub workflow_runs: u32,
    /// Wiki pages of a repository.
    pub pages: u32,
    /// The most bytes of a name: a repository's, a branch's, a label's, a
    /// context's, a wiki page's, or a path.
    pub name_bytes: u32,
    pub title_bytes: u32,
    /// The most bytes of an item's, a comment's or a review's body.
    pub body_bytes: u32,
    /// The most bytes of a file's content, or a wiki page's.
    pub content_bytes: u32,
    /// The most entries a listing answers at once: items, comments, or wiki
    /// page names.
    pub page_size: u32,
    /// Calls held at once, until they are answered and reclaimed. A call
    /// beyond them is answered at once as unavailable.
    pub calls: u32,
    /// Webhook deliveries in flight. A change beyond them is not delivered.
    pub hooks: u32,
    /// Observations kept until a world drains them. Beyond them, they are
    /// dropped and counted.
    pub observations: u32,
}

/// The timers armed at most: one per call, one per delivery, and one per
/// context of each commit with statuses, one per dispatched job, and one queue
/// wake per repository.
pub(crate) fn timers(limits: &Limits) -> Option<u32> {
    let checks = limits.repositories.checked_mul(limits.statuses)?.checked_mul(limits.contexts)?;
    let workflows = limits.repositories.checked_mul(limits.workflow_runs)?.checked_mul(limits.contexts)?;
    limits
        .calls
        .checked_add(limits.hooks)?
        .checked_add(checks)?
        .checked_add(workflows)?
        .checked_add(limits.repositories)
}

/// The comments a repository holds at most.
pub(crate) fn comments(limits: &Limits) -> Option<u32> {
    limits.items.checked_mul(limits.comments)
}

/// Whether these limits are representable: every edge bound they size is finite
/// and every capacity they derive fits (programming-model.md, 6.3).
/// Timer, comment, directory and run capacities fit their u32 counters.
#[must_use]
pub fn valid(limits: &Limits) -> bool {
    timers(limits).is_some()
        && comments(limits).is_some()
        && directory_capacity(limits).is_some()
        && limits.files.checked_add(1).is_some()
        && limits.statuses.checked_add(limits.workflow_runs).is_some()
}

fn directory_capacity(limits: &Limits) -> Option<u32> {
    limits.files.checked_mul(limits.name_bytes)?.checked_add(1)
}

/// The most memory the forge holds under `limits`, in bytes (6.3), or `None`
/// if it does not fit a `u64`.
///
/// It counts the containers, their bookkeeping included, the payloads, the
/// answers held until they go out, and the scratch a merge takes, not
/// allocator overhead. What a call brings is moved into the store or dropped
/// in the step it arrives in.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !valid(limits) {
        return None;
    }
    let repositories = Slab::<Repository>::worst_case(limits.repositories)?
        .checked_add(Map::<Box<[u8]>, Id<Repository>>::worst_case(limits.repositories)?)?
        .checked_add(times(limits.repositories, repository(limits)?)?)?;
    let message = u64::from(limits.title_bytes).checked_add(u64::from(limits.body_bytes))?.checked_add(2)?;
    let commits = Map::<u64, Object>::worst_case(limits.commits)?.checked_add(times(
        limits.commits,
        tree(limits)?.checked_add(message)?.checked_add(u64::from(limits.name_bytes).checked_mul(4)?)?,
    )?)?;
    // A call holds its answer until it goes out, or what it brought until it
    // lands, late.
    let held = answer(limits)?.max(call(limits)?);
    let calls = Slab::<Call>::worst_case(limits.calls)?.checked_add(times(limits.calls, held)?)?;
    // A webhook carries a branch name or an item's body.
    let hooks = Slab::<Delivery>::worst_case(limits.hooks)?
        .checked_add(times(limits.hooks, u64::from(limits.name_bytes).checked_add(u64::from(limits.body_bytes))?)?)?;
    let windows = Map::<crate::faults::RateKey, Window>::worst_case(limits.users)?;
    let timers = Deadlines::<Alarm>::worst_case(timers(limits)?)?;
    let observations = Queue::<Observation>::worst_case(limits.observations)?
        .checked_add(times(limits.observations, observation(limits)?)?)?;
    // A merge walks both histories into sets, using a bounded work queue
    // that visits each commit once, and makes a tree it may
    // refuse; a branch moving or a status reported lists the pull requests
    // that follow it; what a call brings is held until it is stored or
    // dropped; a write replaces what an item held, an observation is made
    // before a full queue drops it.
    // Queue transitions also hold their generated branch, the base name,
    // and branch names copied into move/delete observations and hooks.
    // Groups and their commits occupy the existing branch/commit budgets;
    // Group metadata is inline in Item and Pull, counted by their slots.
    let directories = limits.files.checked_mul(limits.name_bytes)?.checked_add(1)?;
    let directory_scratch = Map::<(u32, u32), [u8; 20]>::worst_case(directories)?
        .checked_add(List::<((u32, u32), [u8; 20])>::worst_case(limits.files.checked_add(1)?)?)?
        .checked_add(Map::<Box<[u8]>, (crate::api::TreeEntryKind, u32, [u8; 20])>::worst_case(limits.files)?)?
        .checked_add(times(limits.files, u64::from(limits.name_bytes))?)?
        .checked_add(times(limits.files, tree_entry(limits)?)?)?;
    let run_scratch = Map::<u64, u64>::worst_case(limits.statuses.checked_add(limits.workflow_runs)?)?
        .checked_add(kept_run_heap(limits, limits.workflow_runs > 0)?)?
        .checked_add(job_heap(limits)?)?
        .checked_add(u64::from(limits.name_bytes).checked_mul(2)?)?;
    let queue_scratch = u64::from(limits.name_bytes).checked_mul(4)?;
    let merge_scratch = queue_scratch
        .checked_add(tree(limits)?.checked_mul(2)?.max(changes(limits)?))?
        .checked_add(Set::<u64>::worst_case(limits.commits)?.checked_mul(2)?)?
        .checked_add(Queue::<u64>::worst_case(limits.commits)?)?
        .checked_add(List::<u64>::worst_case(limits.commits)?)?
        .checked_add(List::<u64>::worst_case(limits.items)?)?
        .checked_add(call(limits)?)?
        .checked_add(observation(limits)?)?;
    // Comparisons own changed files; replay owns trees instead. Item construction,
    // status snapshots and listing indexes do not retain merge scratch.
    let item_scratch = item(limits)?
        .checked_add(List::<StatusView>::worst_case(limits.contexts)?)?
        .checked_add(call(limits)?)?
        .checked_add(observation(limits)?)?
        .checked_add(Set::<(Time, u64)>::worst_case(limits.items)?)?
        .checked_add(List::<u64>::worst_case(limits.items)?)?;
    // Tree reads, run reads/dispatches and merges use separate scratch lifetimes.
    let scratch = merge_scratch
        .max(item_scratch)
        .max(directory_scratch.max(self::directory_scratch(limits)?).checked_add(call(limits)?)?)
        .max(run_scratch.checked_add(call(limits)?)?.checked_add(observation(limits)?)?);
    repositories
        .checked_add(commits)?
        .checked_add(calls)?
        .checked_add(hooks)?
        .checked_add(windows)?
        .checked_add(timers)?
        .checked_add(observations)?
        .checked_add(scratch)?
        .checked_add(u64::from(limits.name_bytes.max(limits.content_bytes)))
}

/// What one repository owns beyond its slot.
fn repository(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let comments = comments(limits)?;
    // Its name twice (the index's key too), its default branch, and its
    // setup: contexts, protection, the repository cue's path, step and log,
    // and one named cue per context. Templates fit at their expanded size.
    let cue = name.checked_mul(2)?.checked_add(u64::from(limits.content_bytes))?;
    let overrides =
        times(limits.contexts, u64::try_from(size_of::<ContextCue>()).ok()?.checked_add(name)?.checked_add(cue)?)?;
    let setup = name
        .checked_mul(6)?
        .checked_add(overrides)?
        .checked_add(names(limits.contexts, limits)?.checked_mul(2)?)?
        .checked_add(times(limits.contexts, u64::try_from(size_of::<Failing>()).ok()?)?)?
        .checked_add(u64::from(limits.content_bytes))?
        // One boxed list of domain user numbers belongs to the protection.
        .checked_add(times(limits.bypassing_users, u64::try_from(size_of::<u64>()).ok()?)?)?;
    let permissions = Map::<u64, Permission>::worst_case(limits.users)?;
    let labels = Set::<Box<[u8]>>::worst_case(limits.labels)?.checked_add(times(limits.labels, name)?)?;
    let items = Map::<u64, Item>::worst_case(limits.items)?
        .checked_add(Set::<(Time, u64)>::worst_case(limits.items)?)?
        .checked_add(Map::<u64, u64>::worst_case(comments)?)?
        .checked_add(times(limits.items, item(limits)?)?)?;
    // At most one retained admission per item, with no owned payload bytes.
    let queue = List::<crate::api::QueueEntry>::worst_case(limits.items)?;
    let branches = Map::<Box<[u8]>, u64>::worst_case(limits.branches)?.checked_add(times(limits.branches, name)?)?;
    let has = Set::<u64>::worst_case(limits.commits)?;
    let context = name.checked_add(u64::from(limits.body_bytes))?.checked_add(link_bytes(limits)?)?;
    let contexts =
        Map::<Box<[u8]>, Status>::worst_case(limits.contexts)?.checked_add(times(limits.contexts, context)?)?;
    let statuses = Map::<u64, Map<Box<[u8]>, Status>>::worst_case(limits.statuses)?
        .checked_add(times(limits.statuses, contexts)?)?;
    let runs = Map::<u64, RunKept>::worst_case(limits.statuses)?
        .checked_add(Map::<u64, RunKept>::worst_case(limits.workflow_runs)?)?
        .checked_add(times(limits.statuses, kept_run_heap(limits, false)?)?)?
        .checked_add(times(limits.workflow_runs, kept_run_heap(limits, true)?)?)?;
    let workflow = u64::try_from(size_of::<Workflow>())
        .ok()?
        .checked_add(name.checked_mul(2)?)?
        .checked_add(names(limits.contexts, limits)?)?
        .checked_add(u64::from(limits.content_bytes))?;
    let workflows = times(limits.workflows, workflow)?;
    let page = name.checked_add(u64::from(limits.content_bytes))?;
    let wiki = Map::<Box<[u8]>, Page>::worst_case(limits.pages)?.checked_add(times(limits.pages, page)?)?;
    setup
        .checked_add(permissions)?
        .checked_add(labels)?
        .checked_add(items)?
        .checked_add(queue)?
        .checked_add(branches)?
        .checked_add(has)?
        .checked_add(statuses)?
        .checked_add(runs)?
        .checked_add(workflows)?
        .checked_add(wiki)
}

/// What one item owns beyond its entry: its text, its labels, its comments,
/// and a pull request's branches and reviews.
fn item(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let body = u64::from(limits.body_bytes);
    let labels = Set::<Box<[u8]>>::worst_case(limits.labels)?.checked_add(times(limits.labels, name)?)?;
    let comments = Map::<u64, Posted>::worst_case(limits.comments)?.checked_add(times(limits.comments, body)?)?;
    let dependencies = Set::<u64>::worst_case(limits.dependencies)?;
    let pull = name
        .checked_mul(2)?
        .checked_add(Set::<u64>::worst_case(limits.users)?)?
        .checked_add(List::<Kept>::worst_case(limits.reviews)?)?
        .checked_add(times(limits.reviews, body)?)?;
    u64::from(limits.title_bytes)
        .checked_add(body)?
        .checked_add(labels)?
        .checked_add(comments)?
        .checked_add(dependencies)?
        .checked_add(pull)
}

/// What one commit's tree owns.
fn tree(limits: &Limits) -> Option<u64> {
    let file = u64::from(limits.name_bytes).checked_add(u64::from(limits.content_bytes))?;
    Map::<Box<[u8]>, Box<[u8]>>::worst_case(limits.files)?.checked_add(times(limits.files, file)?)
}

/// The most an answer held for a call owns.
fn answer(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let body = u64::from(limits.body_bytes);
    let content = u64::from(limits.content_bytes);
    let summary = summary(limits)?;
    let listed = summary.checked_add(u64::try_from(size_of::<Summary>()).ok()?)?;
    let comment = body.checked_add(u64::try_from(size_of::<Comment>()).ok()?)?;
    let review = body.checked_add(u64::try_from(size_of::<Review>()).ok()?)?;
    let status = name
        .checked_add(u64::from(limits.body_bytes))?
        .checked_add(link_bytes(limits)?)?
        .checked_add(u64::try_from(size_of::<StatusView>()).ok()?)?;
    let file = name.checked_add(content)?.checked_add(u64::try_from(size_of::<File>()).ok()?)?;
    let named = name.checked_add(u64::try_from(size_of::<PageName>()).ok()?)?;
    let head = name.checked_add(u64::try_from(size_of::<Head>()).ok()?)?;
    let items = times(limits.page_size, listed)?;
    let item = summary.checked_add(times(limits.page_size, comment)?)?;
    let number = u64::try_from(size_of::<u64>()).ok()?;
    let pull = name
        .checked_mul(2)?
        .checked_add(times(limits.users, number)?)?
        .checked_add(times(limits.reviews, review)?)?
        .checked_add(times(limits.contexts, status)?)?;
    let dependencies = times(limits.dependencies, number)?;
    let labels = names(limits.labels, limits)?;
    let tree = times(limits.files, file)?;
    let pages = times(limits.page_size, named)?.checked_add(name)?;
    let page = name.checked_add(content)?;
    let cloned = name.checked_add(times(limits.branches, head)?)?;
    let comparison = changes(limits)?.checked_add(times(limits.commits, number)?)?;
    let checks = times(
        limits.contexts,
        status
            .checked_add(u64::from(limits.body_bytes))?
            .checked_add(link_bytes(limits)?)?
            .checked_add(u64::try_from(size_of::<CheckSummary>()).ok()?)?,
    )?;
    let threads = times(limits.comments, comment)?;
    let pulls = times(limits.page_size, pull.checked_add(u64::try_from(size_of::<Pull>()).ok()?)?)?;
    let runs = times(
        limits.page_size.min(limits.statuses.checked_add(limits.workflow_runs)?),
        u64::try_from(size_of::<Run>()).ok()?.checked_add(run_payload(limits, limits.workflow_runs > 0)?)?,
    )?;
    let jobs = times(limits.contexts, u64::try_from(size_of::<Job>()).ok()?.checked_add(job_heap(limits)?)?)?;
    let log = name.checked_mul(6)?.checked_add(92)?.checked_add(content)?;
    let job = u64::try_from(size_of::<Job>()).ok()?.checked_add(job_heap(limits)?.checked_mul(2)?)?;
    let git_tree = times(limits.page_size, tree_entry(limits)?)?;
    // Inspection clones the bypass list as well as branch and contexts.
    let protection =
        name.checked_add(names(limits.contexts, limits)?)?.checked_add(times(limits.bypassing_users, number)?)?;
    let collaborators = times(limits.users, u64::try_from(size_of::<Collaborator>()).ok()?)?;
    Some(
        items
            .max(item)
            .max(pull)
            .max(tree)
            .max(git_tree)
            .max(pages)
            .max(page)
            .max(cloned)
            .max(dependencies)
            .max(labels)
            .max(comparison)
            .max(checks)
            .max(protection)
            .max(collaborators)
            .max(threads)
            .max(pulls)
            .max(runs)
            .max(jobs)
            .max(job)
            .max(log),
    )
}

fn changes(limits: &Limits) -> Option<u64> {
    let file = u64::from(limits.name_bytes)
        .checked_add(u64::from(limits.content_bytes).checked_mul(2)?)?
        .checked_add(u64::try_from(size_of::<ChangedFile>()).ok()?)?;
    times(limits.files, file)
}

/// The most a call within the limits brings: the repository's name, and
/// the largest of an issue's text and labels, a pull request's text and
/// branches, a wiki page, a set of numbers, or a working tree's commit.
fn call(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let number = u64::try_from(size_of::<u64>()).ok()?;
    let issue = summary(limits)?;
    let pull =
        u64::from(limits.title_bytes).checked_add(u64::from(limits.body_bytes))?.checked_add(name.checked_mul(2)?)?;
    let page = name.checked_add(u64::from(limits.content_bytes))?;
    let numbers = times(limits.users.max(limits.dependencies), number)?;
    let file =
        name.checked_add(u64::from(limits.content_bytes))?.checked_add(u64::try_from(size_of::<File>()).ok()?)?;
    let commit = times(limits.files, file)?;
    let status = name.checked_add(u64::from(limits.body_bytes))?.checked_add(link_bytes(limits)?)?;
    let dispatch = name.checked_mul(2)?.checked_add(inputs(limits)?)?;
    name.checked_add(issue.max(pull).max(page).max(numbers).max(commit).max(status).max(dispatch))
}

/// What a summary of an item owns: its text and its labels.
fn summary(limits: &Limits) -> Option<u64> {
    u64::from(limits.title_bytes).checked_add(u64::from(limits.body_bytes))?.checked_add(names(limits.labels, limits)?)
}

/// The most one observation owns.
fn observation(limits: &Limits) -> Option<u64> {
    let name = u64::from(limits.name_bytes);
    let body = u64::from(limits.body_bytes);
    // A pull request opened names its repository, head and base.
    let opened = name.checked_mul(3)?.checked_add(summary(limits)?)?;
    let posted = name.checked_add(body)?;
    let labelled = name.checked_add(names(limits.labels, limits)?)?;
    let wiki = name.checked_mul(2)?.checked_add(u64::from(limits.content_bytes))?;
    let number = u64::try_from(size_of::<u64>()).ok()?;
    let depends = name.checked_add(times(limits.dependencies, number)?)?;
    let requested = name.checked_add(times(limits.users, number)?)?;
    Some(opened.max(posted).max(labelled).max(wiki).max(depends).max(requested).max(name.checked_mul(2)?))
}

/// What a boxed list of `count` names owns.
fn names(count: u32, limits: &Limits) -> Option<u64> {
    times(count, u64::from(limits.name_bytes).checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?)?)
}

fn times(count: u32, each: u64) -> Option<u64> {
    u64::from(count).checked_mul(each)
}

fn directory_scratch(limits: &Limits) -> Option<u64> {
    Map::<Box<[u8]>, crate::api::EntryKind>::worst_case(limits.files)?
        .checked_add(List::<crate::api::Entry>::worst_case(limits.files)?)?
        .checked_add(u64::from(limits.files).checked_mul(u64::from(limits.name_bytes))?.checked_mul(2)?)
}
fn link_bytes(limits: &Limits) -> Option<u64> {
    Some(u64::from(limits.body_bytes).max(u64::from(limits.name_bytes).checked_add(51)?))
}
fn inputs(limits: &Limits) -> Option<u64> {
    times(
        limits.contexts,
        u64::from(limits.name_bytes)
            .checked_add(u64::from(limits.body_bytes))?
            .checked_add(u64::try_from(size_of::<(Box<[u8]>, Box<[u8]>)>()).ok()?)?,
    )
}
fn job_heap(limits: &Limits) -> Option<u64> {
    u64::from(limits.name_bytes)
        .checked_mul(8)?
        .checked_add(118)?
        .checked_add(u64::from(limits.content_bytes))?
        .checked_add(times(3, u64::try_from(size_of::<Step>()).ok()?)?)
}
fn run_payload(limits: &Limits, dispatched: bool) -> Option<u64> {
    if dispatched { u64::from(limits.name_bytes).checked_add(inputs(limits)?) } else { Some(0) }
}
fn kept_run_heap(limits: &Limits, dispatched: bool) -> Option<u64> {
    run_payload(limits, dispatched)?
        .checked_add(List::<Job>::worst_case(limits.contexts)?)?
        .checked_add(times(limits.contexts, if dispatched { job_heap(limits)? } else { u64::from(limits.name_bytes) })?)
}
fn tree_entry(limits: &Limits) -> Option<u64> {
    u64::from(limits.name_bytes).checked_add(u64::try_from(size_of::<TreeEntry>()).ok()?)
}
