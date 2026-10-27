//! The forge's API, as its domain layer sees it once the protocol layer has
//! parsed a request: the Forgejo-shaped vocabulary (fake-forge.md, section 2), and the git a
//! worker speaks to it.
//!
//! Names are the forge's own. A repository is named by its full name
//! (`owner/name`), and every call is about one; an item, an issue or a pull
//! request, by its number in its repository, the two sharing one numbering; a
//! comment by an id that only grows, across the forge; a user by an id; a
//! commit by its count in the forge's one store, so a seed replays to the same
//! names. Text is bytes, compared byte for byte.
//!
//! Creating carries no key: a client that must find what it created puts its
//! own marker in a body, and reads.

use alloc::boxed::Box;

use skein_lib::{Duration, Time};

/// A user's permission on a repository. Each grants what the ones before it
/// do.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Permission {
    None,
    /// Read, open issues and pull requests, comment and review.
    Read,
    /// Label, merge, push, delete branches, edit the wiki, report statuses,
    /// edit and delete anyone's comments.
    Write,
    Admin,
}

/// What a call asks of a repository.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Op {
    Read(Read),
    Write(Write),
    Git(Git),
}

/// The reads change nothing; `ListingPermission` lets the protocol combine grants before filtering.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    /// Lists uploads of a known run, empty until its report is available.
    Artifacts {
        run: u64,
    },
    /// Downloads one upload by its id, the run id in this fake's one-report model.
    ArtifactById {
        id: u64,
    },
    /// A run's one uploaded artifact; absent until an unfaulted completion.
    Artifact {
        run: u64,
        name: Box<[u8]>,
    },
    /// The bounded range after before through after, oldest first.
    Commits {
        before: u64,
        after: u64,
    },
    /// The scenario's id for the named repository.
    Locate,
    /// The page `page` (from one) of `limit` items (the most a page holds,
    /// if zero or more) in `state` (either, if `None`) of `kind` (either, if
    /// `None`) that carry every label of `labels`, were opened by `author`
    /// (anyone, if `None`: Forgejo's `created_by`) and were updated at or
    /// after `since`, at the forge's resolution, in `order`: least updated
    /// first, then by number, as Forgejo pages them. Answered by
    /// [`Answer::Items`]. Pages are offsets into an order that moves: see
    /// the `reads` module for what a pass over them finds.
    Items {
        state: Option<State>,
        kind: Option<Kind>,
        labels: Box<[Box<[u8]>]>,
        author: Option<u64>,
        order: Order,
        since: Time,
        page: u32,
        limit: u32,
    },
    /// The item `number` and a page of its comments with ids above `after`.
    /// Answered by [`Answer::Item`].
    Item {
        number: u64,
        after: u64,
    },
    /// The comment `id`. Answered by [`Answer::Comment`].
    Comment {
        id: u64,
    },
    /// The items the item `number` depends on, which block it. Answered by
    /// [`Answer::Dependencies`].
    Dependencies {
        number: u64,
    },
    /// The labels defined. Answered by [`Answer::Labels`].
    Labels,
    /// The pull request `number`, its reviews and the statuses on its head.
    /// Answered by [`Answer::Pull`].
    Pull {
        number: u64,
    },
    /// Files and whole before/after contents at the pull's exact head,
    /// paged from one. The diff starts at its merge base, as Forgejo does.
    PullFiles {
        number: u64,
        page: u32,
        limit: u32,
    },
    /// Compare exact commits. Forgejo ignores these page arguments;
    /// this fake returns the whole bounded comparison or `TooLarge`.
    Compare {
        base: u64,
        head: u64,
        page: u32,
        limit: u32,
    },
    /// Detailed commit statuses, with their description and job link.
    Checks {
        commit: u64,
    },
    /// The job at its latest attempt, with its steps; commit CI exposes failed jobs.
    Job {
        job: u64,
    },
    /// One job attempt's plaintext output, as the pinned Forgejo serves it.
    /// A step selects its slice; none joins all three slices in order.
    Log {
        job: u64,
        attempt: u32,
        step: Option<u32>,
    },
    /// Actions routes, bounded by retained CI and dispatched runs.
    Runs {
        workflow: Option<Box<[u8]>>,
        commit: Option<u64>,
        number: Option<u64>,
        page: u32,
        limit: u32,
    },
    /// One directory, paged from one (fake-forge.md, 2.3).
    GitTree {
        object: TreeOf,
        page: u32,
        limit: u32,
    },
    Run {
        run: u64,
    },
    Jobs {
        run: u64,
    },
    /// The whole thread, oldest first.
    Comments {
        number: u64,
    },
    /// The oldest matching pull, open or not.
    FirstPull {
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// Open pulls, oldest first, paged from one.
    OpenPulls {
        page: u32,
        limit: u32,
    },
    /// Requires admin permission, including when no protection exists.
    Protection {
        branch: Box<[u8]>,
    },
    Settings,
    Collaborators,
    /// The open pull request, else the oldest, that merges `head` into
    /// `base`. Answered by [`Answer::Pull`].
    PullFor {
        head: Box<[u8]>,
        base: Box<[u8]>,
    },
    /// The latest status of each context on `commit`, which the repository
    /// has. Answered by [`Answer::Statuses`].
    Statuses {
        commit: u64,
    },
    /// The permission of `user`. Answered by [`Answer::Permission`].
    Permission {
        user: u64,
    },
    /// Reads the caller's direct grant for a protocol listing that adds organization access.
    ListingPermission,
    /// Where `branch` is. Answered by [`Answer::Commit`].
    Branch {
        branch: Box<[u8]>,
    },
    /// The files of `commit`, which the repository has. Answered by
    /// [`Answer::Tree`].
    Tree {
        commit: u64,
    },
    /// One file of `commit`. Answered by [`Answer::File`].
    File {
        commit: u64,
        path: Box<[u8]>,
    },
    /// A page of the wiki's page names, in their order, from after `after`.
    /// Answered by [`Answer::Pages`].
    Pages {
        after: Option<Box<[u8]>>,
    },
    /// The wiki page `name`. Answered by [`Answer::Page`].
    Page {
        name: Box<[u8]>,
    },
}

/// The writes, each answered by [`Answer::Done`] unless it says otherwise.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Write {
    /// Replaces the repository's branch protection. Requires admin permission.
    /// The rule and its arrays must fit the repository's configured bounds.
    SetProtection {
        protection: Option<Protection>,
    },
    /// Opens an issue titled, carrying `labels`, which are defined. Answered
    /// by [`Answer::Created`] with its number. The labels of a user who may
    /// not label are dropped.
    CreateIssue {
        title: Box<[u8]>,
        body: Box<[u8]>,
        labels: Box<[Box<[u8]>]>,
    },
    /// Comments on the item `number`, to say something. Answered by
    /// [`Answer::Commented`] with the comment's id.
    Comment {
        number: u64,
        body: Box<[u8]>,
    },
    /// Edits the title, the body, or both, of the item `number`: its
    /// author's, or anyone's with write permission. A title says something.
    EditItem {
        number: u64,
        title: Option<Box<[u8]>>,
        body: Option<Box<[u8]>>,
    },
    /// Makes the items the item `number` depends on exactly `dependencies`,
    /// items of the same repository other than it, none of which depends on
    /// it. Needs write permission.
    SetDependencies {
        number: u64,
        dependencies: Box<[u64]>,
    },
    /// Edits the comment `id`, to say something: its author's, or anyone's
    /// with write permission.
    EditComment {
        id: u64,
        body: Box<[u8]>,
    },
    /// Deletes the comment `id`: its author's, or anyone's with write
    /// permission.
    DeleteComment {
        id: u64,
    },
    /// Makes the labels of the item `number` exactly `labels`, which are
    /// defined. Needs write permission.
    SetLabels {
        number: u64,
        labels: Box<[Box<[u8]>]>,
    },
    /// Adds `labels`, which are defined, to the item `number`, leaving those
    /// it carries. Needs write permission.
    AddLabels {
        number: u64,
        labels: Box<[Box<[u8]>]>,
    },
    /// Removes `labels` from the item `number`, those it does not carry
    /// aside, as Forgejo removes one label a call. Needs write permission.
    RemoveLabels {
        number: u64,
        labels: Box<[Box<[u8]>]>,
    },
    /// Defines the label `name`. Needs write permission.
    DefineLabel {
        name: Box<[u8]>,
    },
    /// Opens a pull request to merge `head` into `base`. Answered by
    /// [`Answer::Created`] with its number.
    OpenPull {
        title: Box<[u8]>,
        body: Box<[u8]>,
        head: Box<[u8]>,
        base: Box<[u8]>,
        /// Open even when the head adds no commits to the base.
        opens_empty: bool,
    },
    /// Makes the users asked to review the open pull request `number`
    /// exactly `reviewers`, each of whom may read and none of whom is its
    /// author. A request ends when its reviewer reviews. Needs write
    /// permission.
    RequestReviewers {
        number: u64,
        reviewers: Box<[u64]>,
    },
    RemoveReviewers {
        number: u64,
        reviewers: Box<[u64]>,
    },
    SetReviewers {
        number: u64,
        reviewers: Box<[u64]>,
    },
    /// Reviews the open pull request `number` at its head, with `verdict`;
    /// or, with none, starts a pending review (Forgejo's `PENDING`), which
    /// no read shows until its author submits it. Answered by
    /// [`Answer::Reviewed`] with its id.
    Review {
        number: u64,
        verdict: Option<Verdict>,
        body: Box<[u8]>,
    },
    /// Submits the pending review `review` of the pull request `number`,
    /// the user's own, with `verdict`, at the head it was started on. It
    /// keeps its id, so it is shown among the reviews before any submitted
    /// after it was started.
    Submit {
        number: u64,
        review: u64,
        verdict: Verdict,
    },
    /// Merges the pull request `number` if its head is still `head`, in the
    /// repository's style or by fast-forward only. Answered by [`Answer::Merged`].
    /// Needs write permission.
    Merge {
        number: u64,
        head: u64,
        how: MergeHow,
    },
    /// Admits the exact head to the merge queue. Answered by the pull's
    /// updated view; an existing entry is refused as `AlreadyQueued`. Needs write permission.
    Enqueue {
        number: u64,
        head: u64,
    },
    /// Updates the pull's branch by merging its base or replaying its own
    /// commits onto the base, starting CI at the new head. A conflict changes nothing.
    Update {
        number: u64,
        style: UpdateStyle,
    },
    /// Create a repository branch at an existing full commit id through
    /// the API, with the same permissions and protection as git creation.
    CreateBranch {
        branch: Box<[u8]>,
        commit: u64,
    },
    /// Closes or reopens the item `number`: its author's, or anyone's with
    /// write permission.
    Close {
        number: u64,
    },
    Reopen {
        number: u64,
    },
    /// Deletes `branch`, which is neither the default nor protected, and
    /// closes the open pull requests from or into it, as Forgejo does. Needs
    /// write permission.
    DeleteBranch {
        branch: Box<[u8]>,
    },
    /// Reports the status `state` of `context` on `commit`, which the
    /// repository has. Needs write permission.
    Status {
        commit: u64,
        context: Box<[u8]>,
        state: Check,
        description: Box<[u8]>,
        link: Box<[u8]>,
    },
    /// Runs the workflow at the branch's current head (fake-forge.md, 2.3).
    Dispatch {
        workflow: Box<[u8]>,
        branch: Box<[u8]>,
        inputs: Box<[WorkflowInput]>,
    },
    /// Runs one completed run's job at its next attempt.
    Rerun {
        job: u64,
    },
    /// Creates or replaces the wiki page `name`. Answered by
    /// [`Answer::Revision`]. Needs write permission.
    PutPage {
        name: Box<[u8]>,
        content: Box<[u8]>,
    },
    /// Deletes the wiki page `name`. Needs write permission.
    DeletePage {
        name: Box<[u8]>,
    },
}

/// What git does against the forge, the remote side. Fetching needs read
/// permission; pushing and creating, write.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Git {
    /// Every branch, and the default. Answered by [`Answer::Cloned`].
    Clone,
    /// What `want` names, and the commits before it. Answered by
    /// [`Answer::Commit`].
    Fetch { want: Want },
    /// Moves `branch` to `commit`, a commit of the store, as a fast-forward,
    /// or creates it. When `expected` is given the branch must be exactly
    /// there; a moved or missing head is rejected without transferring any
    /// objects. `None` keeps the unconditional fast-forward behavior.
    /// Answered by [`Answer::Pushed`].
    Push { branch: Box<[u8]>, commit: u64, expected: Option<u64> },
    /// Creates `branch` at `commit`, which the repository has, only if it is
    /// nowhere. Answered by [`Answer::Branch`].
    Create { branch: Box<[u8]>, commit: u64 },
}

/// What a fetch asks for.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Want {
    Branch(Box<[u8]>),
    Commit(u64),
    Default,
}

/// The answer to a call that succeeded.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    Artifacts(Box<[ArtifactInfo]>),
    Artifact {
        commit: u64,
        artifact: Artifact,
    },
    Commits {
        commits: Box<[CommitMessage]>,
    },
    Located {
        id: u32,
    },
    /// A page of items, whether a later page has more, and the forge's time
    /// as it made the page, at its resolution (what Forgejo's `Date` header
    /// says).
    Items {
        items: Box<[Summary]>,
        more: bool,
        now: Time,
    },
    /// An item and a page of its comments, oldest first, and whether more
    /// follow.
    Item {
        item: Summary,
        comments: Box<[Comment]>,
        more: bool,
    },
    Pull(Pull),
    Pulls(Box<[Pull]>),
    Comments(Box<[Comment]>),
    Runs(Box<[Run]>),
    Run(Run),
    Jobs(Box<[Job]>),
    /// A job and its steps, each with the log served separately by the protocol.
    Job {
        job: Job,
        steps: Box<[Step]>,
    },
    PullFiles {
        head: u64,
        files: Box<[ChangedFile]>,
        more: bool,
    },
    Comparison {
        /// The newest commit shared by both histories, when they intersect.
        merge_base: Option<u64>,
        base: u64,
        head: u64,
        /// Whether the requested base is an ancestor of the head.
        contains_base: bool,
        files: Box<[ChangedFile]>,
        commits: Box<[u64]>,
    },
    Checks(Box<[CheckSummary]>),
    Protection(Option<Protection>),
    Settings(Settings),
    Collaborators(Box<[Collaborator]>),
    /// In their contexts' order.
    Statuses(Box<[Status]>),
    Permission(Permission),
    /// A comment, and the item it is on.
    Comment {
        number: u64,
        comment: Comment,
    },
    /// Item numbers, in their order.
    Dependencies(Box<[u64]>),
    /// Label names, in their order.
    Labels(Box<[Box<[u8]>]>),
    /// Where a branch is, or what a fetch fetched.
    Commit(u64),
    Tree(Box<[File]>),
    GitTree {
        entries: Box<[TreeEntry]>,
        truncated: bool,
    },
    File(Box<[u8]>),
    /// A page of wiki page names, and the last of them if there may be more.
    Pages {
        pages: Box<[PageName]>,
        next: Option<Box<[u8]>>,
    },
    Page(Page),
    /// The number of the item opened.
    Created(u64),
    /// The id of the comment posted.
    Commented(u64),
    /// The id of the review made.
    Reviewed(u64),
    /// The commit the merge made on the base.
    Merged(u64),
    /// The wiki page's new revision.
    Revision(u64),
    Done,
    Cloned {
        default: Box<[u8]>,
        branches: Box<[Head]>,
    },
    Pushed(Pushed),
    Branch(Created),
}

/// Why a call failed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    /// The forge failed, or was too busy: nothing was done. Worth retrying.
    Unavailable,
    /// The forge timed out answering: what was asked may have been done.
    Timeout,
    /// The user made too many calls: none is taken until `reset`.
    RateLimited {
        reset: Time,
    },
    /// The user's permission does not allow it, or the comment or item is
    /// someone else's.
    Forbidden,
    Missing(What),
    /// A name, title, body, content, tree or label list beyond the forge's
    /// limits.
    TooLarge,
    /// The repository holds as much of it as it may.
    Full,
    /// A label of that name is defined, or an open pull request has the same
    /// head and base.
    Exists,
    /// The pull already has this retained admission; no second entry was made.
    AlreadyQueued(QueueEntry),
    /// An item would depend on itself, or on an item that depends on it.
    Circular,
    /// The head has nothing the base does not have.
    NothingToMerge,
    /// A title or a comment's body that must say something is empty.
    Empty,
    /// The pull request is closed, or merged.
    Closed,
    /// The pull request's head is no longer the one named.
    Stale,
    /// The head and the base changed a path differently.
    Conflict,
    /// The branch's protection refuses it: the default branch, or a merge
    /// lacking the statuses or approvals it requires, or a push or deletion.
    Protected,
    /// The repository cannot be reached.
    Unreachable,
    /// The repository refuses what is pushed to it.
    Refused,
}

/// What the forge does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum What {
    Repository,
    Item,
    /// The item is an issue, not a pull request.
    Pull,
    Comment,
    Label,
    Branch,
    Commit,
    File,
    Page,
    Review,
    Job,
}

pub use crate::actions::{Conclusion, Failing, Job, Run, RunEvent, RunState, Step, StepStatus, WORKFLOW_STEP};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MergeHow {
    Style,
    FastForwardOnly,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum UpdateStyle {
    Merge,
    Rebase,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TreeOf {
    Commit(u64),
    Tree([u8; 20]),
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct TreeEntry {
    pub name: Box<[u8]>,
    pub mode: u32,
    pub kind: EntryKind,
    pub object: [u8; 20],
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EntryKind {
    Blob,
    Tree,
}

/// The order of an item listing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Order {
    LeastUpdated,
    NewestCreated,
}

/// Whether an item is an issue or a pull request.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Kind {
    Issue,
    Pull,
}

/// Whether an item is open. A merged pull request is closed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum State {
    Open,
    Closed,
}

/// An item as a listing shows it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Summary {
    pub number: u64,
    pub kind: Kind,
    pub state: State,
    /// Whether the pull merged; false for issues.
    pub merged: bool,
    pub title: Box<[u8]>,
    pub body: Box<[u8]>,
    pub author: u64,
    /// In their order.
    pub labels: Box<[Box<[u8]>]>,
    pub created: Time,
    /// Moves on every change to the item: its state, labels, comments,
    /// reviews, head, and the statuses on its head.
    pub updated: Time,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Comment {
    pub id: u64,
    pub author: u64,
    pub body: Box<[u8]>,
    pub created: Time,
    /// When it was last edited.
    pub edited: Option<Time>,
}

/// A pull request: its branches, its head commit (which follows the head
/// branch while it is open), whether and how it merged, its reviews oldest
/// first, and the latest status of each context on its head.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a pull snapshot carries independent queue, checking and merge facts"
)]
pub struct Pull {
    pub number: u64,
    pub state: State,
    pub head: Box<[u8]>,
    pub base: Box<[u8]>,
    pub commit: u64,
    /// Where the base branch is, unless it was deleted.
    pub base_commit: Option<u64>,
    /// The newest commit head and base share.
    pub merge_base: Option<u64>,
    /// The commit its merge made.
    pub merged: Option<u64>,
    /// Whether its base currently requires queue admission.
    pub queue_required: bool,
    /// Its retained merge-queue entry, if admitted.
    pub queued: Option<QueueEntry>,
    /// The current group commit, if its queue entry has been built.
    pub group: Option<u64>,
    /// The reason for its last ejection, cleared by another admission.
    pub dequeued: Option<QueueRemoval>,
    /// Whether the merge fact came from the queue.
    pub queue_merged: bool,
    /// Whether a head or base move is still checking mergeability.
    pub checking: bool,
    /// Whether the head merges into the base without a conflict.
    pub mergeable: bool,
    /// The users asked to review it who have not yet, in their order.
    pub reviewers: Box<[u64]>,
    pub reviews: Box<[Review]>,
    pub statuses: Box<[Status]>,
}

/// Why an entry left the queue without landing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum QueueRemoval {
    MergeConflict,
    CiFailure,
}

impl QueueRemoval {
    /// GitHub's spelling (forge-github.md, 4.1).
    #[must_use]
    pub fn github(self) -> &'static [u8] {
        match self {
            Self::MergeConflict => b"MERGE_CONFLICT",
            Self::CiFailure => b"CI_FAILURE",
        }
    }
}

/// An admission in repository order, numbered from one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct QueueEntry {
    pub number: u64,
    pub head: u64,
    pub position: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Review {
    /// An id that only grows, across the forge, given as it was started.
    pub id: u64,
    pub author: u64,
    pub verdict: Verdict,
    /// The head it reviewed.
    pub commit: u64,
    pub body: Box<[u8]>,
    pub at: Time,
    /// Whether its author had write permission when they reviewed: only
    /// official approvals count towards a protection's.
    pub official: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Verdict {
    Approve,
    RequestChanges,
    Comment,
}

/// A context's status on a commit.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Status {
    pub description: Box<[u8]>,
    pub link: Box<[u8]>,
    pub context: Box<[u8]>,
    pub state: Check,
    pub author: u64,
    pub at: Time,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Check {
    Pending,
    Passed,
    Failed,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    pub content: Box<[u8]>,
}

/// The complete change of a file, including additions and deletions. The
/// protocol layer may render these trees as a diff for an ordinary client.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ChangedFile {
    pub path: Box<[u8]>,
    pub before: Option<Box<[u8]>>,
    pub after: Option<Box<[u8]>>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CheckSummary {
    pub status: Status,
    pub description: Box<[u8]>,
    pub link: Box<[u8]>,
    pub job: Option<JobRef>,
}
/// Forgejo Actions identity for one check's job attempt.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct JobRef {
    pub run: u64,
    pub job: u64,
    pub attempt: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Settings {
    pub default: Box<[u8]>,
    pub merge: bool,
    pub rebase: bool,
    pub squash: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Collaborator {
    pub user: u64,
    pub permission: Permission,
}

/// A wiki page. Its revision grows with every write to the repository's wiki.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub name: Box<[u8]>,
    pub content: Box<[u8]>,
    pub author: u64,
    pub revision: u64,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PageName {
    pub name: Box<[u8]>,
    pub revision: u64,
}

/// A branch and where it is.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Head {
    pub branch: Box<[u8]>,
    pub commit: u64,
}

/// How a push went: the branch is at the commit, or it is not an ancestor of
/// it and was left where it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Pushed {
    Pushed,
    Rejected,
}

/// How a creation went: the branch was made, or it existed and was left where
/// it is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Created {
    Created,
    Exists,
}

/// A pull's queue transition retained at the change, before its delayed hook.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum QueueHook {
    Enqueued,
    Dequeued(QueueRemoval),
}

/// What a webhook says changed. Those about an item name it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    /// An issue opened, closed, reopened or labelled.
    Issue,
    /// A pull request opened, closed, reopened, labelled or merged, or its
    /// head moved.
    Pull,
    /// A comment posted, edited or deleted.
    Comment,
    Review,
    /// A job ended, passed or failed; never a person's status.
    Job,
    /// A queue group made or ended, naming its commit.
    Group {
        ended: bool,
    },
    /// A branch moved, made or deleted.
    Push,
}

/// A job's end: the run's job, the commit and the verdict.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ended {
    pub job: JobRef,
    pub commit: u64,
    pub state: Check,
}

// A scenario: what a world sets the forge up with.

/// A repository to add: its name, its default branch at a first commit of
/// `tree`, its labels, its CI, its protected branch, and whether a subscriber
/// hears of its changes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Setup {
    /// The forge's repository id, nonzero and unique within the scenario.
    pub id: u32,
    pub name: Box<[u8]>,
    pub default: Box<[u8]>,
    pub tree: Box<[File]>,
    pub labels: Box<[Box<[u8]>]>,
    pub checks: Checks,
    pub workflows: Box<[Workflow]>,
    pub protection: Option<Protection>,
    pub hooked: bool,
}

/// A repository's CI: the contexts it reports on every commit that becomes a
/// branch's or a pull request's head, pending at once and then passed or
/// failed after a latency drawn from `latency_min..=latency_max`, or never;
/// and some of them run again. Chances are per mille.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Checks {
    pub contexts: Box<[Box<[u8]>]>,
    pub latency_min: Duration,
    pub latency_max: Duration,
    /// The chance that a context never reports past pending.
    pub silent: u32,
    /// The chance that a context passes, of those that report.
    pub passes: u32,
    /// The chance that a context that reported is run again, once: pending
    /// again at once, and a verdict drawn again after another latency, as a
    /// re-run job reports on Forgejo.
    pub reruns: u32,
    /// When set, the file or message cue decides instead of `passes`.
    pub cue: Option<Cue>,
}

/// A verdict cued by a commit's file content or message.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Cue {
    /// Fails the ids listed one per line in the file at the run's commit.
    Tests { path: Box<[u8]> },
    /// Passes when the file at `path` contains `green`, and fails otherwise.
    File { path: Box<[u8]>, green: Box<[u8]> },
    /// Fails when the commit message contains `fail`, and passes otherwise.
    Message { fail: Box<[u8]> },
}

/// A protected branch: nothing is pushed to it, creates it or deletes it,
/// and a pull request merges into it only with each of `contexts` passed on
/// its exact head and `approvals` official approvals, its author aside: of
/// that head if `dismiss_stale`, and of any head if not (Forgejo's default).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent protection facts")]
pub struct Protection {
    /// Facts shown by the layers; no CODEOWNERS engine or linear-history enforcement.
    pub linear: bool,
    pub code_owners: bool,
    /// Domain user numbers whose merges skip checks and approvals, bounded by `Limits::bypassing_users`.
    pub bypassing_users: Box<[u64]>,
    /// Requires admission through the merge queue; false for ordinary protection.
    pub queue: bool,
    pub branch: Box<[u8]>,
    pub contexts: Box<[Box<[u8]>]>,
    pub approvals: u32,
    pub dismiss_stale: bool,
}

/// A workflow run only when dispatched (fake-forge.md, 2.3).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Workflow {
    /// The test ids failed by the workflow, beside its cue (fake-forge.md, 2.3).
    pub fails: Box<[Box<[u8]>]>,
    /// The artifact a completed run uploads, naming the ids it failed.
    pub report: Option<Report>,
    /// The dispatch input selecting ids to run alone.
    pub select: Option<Box<[u8]>>,
    /// Its file under `.forgejo/workflows/`, as a dispatch names it.
    pub file: Box<[u8]>,
    pub jobs: Box<[Box<[u8]>]>,
    pub latency_min: Duration,
    pub latency_max: Duration,
    pub silent: u32,
    pub passes: u32,
    pub cue: Option<Cue>,
}

/// One named dispatch input; keys fit a name and values a body (fake-forge.md, 2.3).
pub type WorkflowInput = (Box<[u8]>, Box<[u8]>);

/// A workflow's report artifact.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Report {
    pub artifact: Box<[u8]>,
    pub format: ReportFormat,
    pub deflate: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ReportFormat {
    Junit,
    Lines,
}
/// One uploaded artifact, before its wire archive is encoded.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Artifact {
    pub name: Box<[u8]>,
    pub path: Box<[u8]>,
    pub bytes: Box<[u8]>,
    pub deflate: bool,
}
/// A range's full commit metadata.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CommitMessage {
    pub id: u64,
    pub parents: Box<[u64]>,
    pub message: Box<[u8]>,
}

/// A completed run's upload metadata; the fake uses its run id as the artifact id.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ArtifactInfo {
    pub id: u64,
    pub name: Box<[u8]>,
    pub size_in_bytes: u64,
}
