//! A client's promises over decoded observations (fake-forge.md, section 2.5).
//!
//! The profile's client is judged, including another user rebasing its branch.
//! Creations establish ownership within this stream; pre-existing objects have no known owner. The first `Labelled`
//! on an item without an `Opened` or earlier `Labelled` is a baseline and
//! judges nothing. Decisions may arrive before or after their observations.
//!
//! Each repository slot and each of its tables has `bounds.objects` slots,
//! allocated at construction. Labels are remembered per item, including
//! removed labels; editors live in the item and comment tables. Evidence has
//! the same bound, and decisions have `bounds.decisions` slots. Overflow is
//! sticky. Payloads are owned copies of the decoded input's bytes.

use alloc::boxed::Box;
use skein_lib::bytes::{copy_of, find, find_from};
use skein_lib::{Decimal, List, Writer};

use crate::Observation;
use crate::api::Kind;

/// The client, its key delimiters, branch prefixes, permissions and limits.
#[derive(Debug)]
pub struct Profile {
    pub client: u64,
    pub marker: (Box<[u8]>, Box<[u8]>),
    pub owned_branches: Box<[Box<[u8]>]>,
    pub allowed: Box<[Allowance]>,
    pub bounds: Bounds,
}

/// A permitted write on an item the client did not open.
#[derive(Debug)]
pub enum Allowance {
    Comment,
    Labels(Box<[u8]>),
    Review,
    Request,
    Merge,
    Close,
}

/// Capacities fixed when a judge is constructed.
#[derive(Debug)]
pub struct Bounds {
    pub objects: u32,
    pub decisions: u32,
}

/// One decoded observation or cumulative loss checkpoint, in drain order.
#[derive(Debug)]
pub enum Recorded {
    Observed { observation: Observation, forward: Option<bool> },
    Lost(u64),
}

/// The exact pull head considered by the client.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Decision {
    pub repository: Box<[u8]>,
    pub number: u64,
    pub head: u64,
}

/// A complete judgement, or why its evidence is incomplete.
#[derive(PartialEq, Eq, Debug)]
pub enum Verdict {
    Pass,
    Fail(Box<[Breach]>),
    Undecided(Why, Box<[Breach]>),
}

#[derive(PartialEq, Eq, Debug)]
pub enum Why {
    Lost(u64),
    Truncated,
    Full,
    NoDecisions,
    Unforwarded,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Promise {
    Once,
    Forced,
    Foreign,
    Order,
}

/// A promise and its already percent-encoded forge subject.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Breach {
    pub promise: Promise,
    pub subject: Box<[u8]>,
}

impl Breach {
    #[must_use]
    pub fn line(&self) -> Box<[u8]> {
        let name: &[u8] = match self.promise {
            Promise::Once => b"once",
            Promise::Forced => b"forced",
            Promise::Foreign => b"foreign",
            Promise::Order => b"order",
        };
        join(&[b"promise", name, &self.subject])
    }
}

#[derive(Debug)]
struct Item {
    number: u64,
    own: bool,
    editor: Option<u64>,
    baseline: bool,
}

#[derive(Debug)]
struct Comment {
    id: u64,
    own: bool,
    editor: u64,
}

#[derive(Debug)]
struct Branch {
    name: Box<[u8]>,
    own: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Creation {
    Issue,
    Pull,
    Comment,
}

impl Creation {
    fn name(self) -> &'static [u8] {
        match self {
            Creation::Issue => b"issue",
            Creation::Pull => b"pull",
            Creation::Comment => b"comment",
        }
    }
}

#[derive(Debug)]
struct Key {
    kind: Creation,
    bytes: Box<[u8]>,
}

#[derive(Debug)]
struct Label {
    number: u64,
    name: Box<[u8]>,
    present: bool,
}

#[derive(Debug)]
struct Evidence {
    breach: Breach,
    when: u64,
    decision: Option<(u64, u64)>,
}

#[derive(Debug)]
struct Repository {
    name: Option<Box<[u8]>>,
    items: List<Item>,
    comments: List<Comment>,
    branches: List<Branch>,
    keys: List<Key>,
    labels: List<Label>,
    evidence: List<Evidence>,
}

/// A branch deletion waits for the queue's following `Group { ended: true }`.
#[derive(Debug)]
struct Deletion {
    repository: u32,
    at: u64,
    branch: Box<[u8]>,
    when: u64,
}

/// The preceding observation can identify a forge-generated branch creation
/// or base move, or a caller's rebase. Checkpoints do not break observation
/// adjacency.
#[derive(Debug)]
enum Preceding {
    None,
    Merge { repository: Box<[u8]>, branch: Box<[u8]>, commit: u64, by: u64 },
    Group { repository: Box<[u8]>, commit: u64 },
    Rebase { repository: Box<[u8]>, branch: Box<[u8]>, from: u64, to: u64, by: u64 },
}

#[derive(Debug)]
enum Tail {
    Observation,
    Checkpoint,
}

/// Bounded evidence, ownership and decision state for one client.
#[derive(Debug)]
pub struct Judge {
    profile: Profile,
    repositories: List<Repository>,
    decisions: List<Decision>,
    complete: bool,
    lost: u64,
    tail: Tail,
    full: bool,
    unforwarded: bool,
    when: u64,
    preceding: Preceding,
    deletion: Option<Deletion>,
}

impl Judge {
    #[must_use]
    pub fn new(profile: Profile) -> Judge {
        let mut repositories = List::with_capacity(profile.bounds.objects);
        for _ in 0..profile.bounds.objects {
            let capacity = profile.bounds.objects;
            let repository = Repository {
                name: None,
                items: List::with_capacity(capacity),
                comments: List::with_capacity(capacity),
                branches: List::with_capacity(capacity),
                keys: List::with_capacity(capacity),
                labels: List::with_capacity(capacity),
                evidence: List::with_capacity(capacity),
            };
            repositories.push(repository).expect("one repository slot per bound");
        }
        let decisions = List::with_capacity(profile.bounds.decisions);
        Judge {
            profile,
            repositories,
            decisions,
            complete: false,
            lost: 0,
            tail: Tail::Observation,
            full: false,
            unforwarded: false,
            when: 0,
            preceding: Preceding::None,
            deletion: None,
        }
    }

    pub fn observe(&mut self, recorded: &Recorded) {
        match recorded {
            Recorded::Lost(count) => {
                self.lost = *count;
                self.tail = Tail::Checkpoint;
            }
            Recorded::Observed { observation, forward } => {
                self.tail = Tail::Observation;
                self.when = self.when.saturating_add(1);
                self.flush_deletion(observation);
                self.observation(observation, *forward);
                self.preceding = match observation {
                    Observation::Merged { repository, base, commit, by, .. } => Preceding::Merge {
                        repository: repository.clone(),
                        branch: base.clone(),
                        commit: *commit,
                        by: *by,
                    },
                    Observation::Rebased { repository, branch, from, to, by } => Preceding::Rebase {
                        repository: repository.clone(),
                        branch: branch.clone(),
                        from: *from,
                        to: *to,
                        by: *by,
                    },
                    Observation::Group { repository, commit, ended: false, .. } => {
                        Preceding::Group { repository: repository.clone(), commit: *commit }
                    }
                    Observation::Moved { .. }
                    | Observation::Deleted { .. }
                    | Observation::Opened { .. }
                    | Observation::Closed { .. }
                    | Observation::Reopened { .. }
                    | Observation::Labelled { .. }
                    | Observation::Revised { .. }
                    | Observation::Depends { .. }
                    | Observation::Requested { .. }
                    | Observation::Defined { .. }
                    | Observation::Commented { .. }
                    | Observation::Edited { .. }
                    | Observation::Removed { .. }
                    | Observation::Reviewed { .. }
                    | Observation::Reported { .. }
                    | Observation::Enqueued { .. }
                    | Observation::Group { ended: true, .. }
                    | Observation::Dequeued { .. }
                    | Observation::Refused { .. }
                    | Observation::Rejected { .. }
                    | Observation::Wiki { .. } => Preceding::None,
                };
            }
        }
    }

    pub fn decide(&mut self, decision: &Decision) {
        if self.decisions.push(decision.clone()).is_err() {
            self.full = true;
        }
    }

    pub fn decided(&mut self) {
        self.complete = true;
    }

    #[must_use]
    pub fn verdict(&self) -> Verdict {
        let mut capacity = 0_u32;
        let mut order = false;
        for repository in &self.repositories {
            capacity = capacity.checked_add(repository.evidence.len()).expect("allocated evidence fits a u32");
            for evidence in &repository.evidence {
                if evidence.breach.promise == Promise::Order {
                    order = true;
                }
            }
        }
        let mut full = self.full;
        let pending = match &self.deletion {
            Some(deletion) => {
                let repository = self.repositories.get(deletion.repository).expect("pending repository");
                let breach = branch_breach(repository.name(), Promise::Foreign, &deletion.branch);
                if repository.contains(&breach) {
                    None
                } else if repository.evidence.room() == 0 {
                    full = true;
                    None
                } else {
                    capacity = capacity.checked_add(1).expect("allocated evidence fits a u32");
                    Some(breach)
                }
            }
            None => None,
        };
        let mut breaches = List::with_capacity(capacity);
        let mut after = 0;
        // Select in observation order across the fixed repository tables.
        for _ in 0..capacity {
            let mut next: Option<&Evidence> = None;
            for repository in &self.repositories {
                for evidence in &repository.evidence {
                    if evidence.when > after {
                        match next {
                            Some(kept) if kept.when <= evidence.when => {}
                            Some(_) | None => next = Some(evidence),
                        }
                    }
                }
            }
            let Some(next) = next else { break };
            // Several promises can first be breached by the same observation.
            for repository in &self.repositories {
                for evidence in &repository.evidence {
                    if evidence.when == next.when && !self.satisfied(repository.name(), evidence.decision) {
                        breaches.push(evidence.breach.clone()).expect("room for all evidence");
                    }
                }
            }
            after = next.when;
        }
        if let Some(breach) = pending {
            breaches.push(breach).expect("room for the pending deletion");
        }
        let truncated = match self.tail {
            Tail::Observation => true,
            Tail::Checkpoint => false,
        };
        let why = if self.lost > 0 {
            Some(Why::Lost(self.lost))
        } else if truncated {
            Some(Why::Truncated)
        } else if full {
            Some(Why::Full)
        } else if order && !self.complete {
            Some(Why::NoDecisions)
        } else if self.unforwarded {
            Some(Why::Unforwarded)
        } else {
            None
        };
        match why {
            Some(why) => Verdict::Undecided(why, breaches.into_boxed()),
            None if breaches.is_empty() => Verdict::Pass,
            None => Verdict::Fail(breaches.into_boxed()),
        }
    }

    fn satisfied(&self, repository: &[u8], decision: Option<(u64, u64)>) -> bool {
        let Some((number, head)) = decision else { return false };
        for kept in &self.decisions {
            if &*kept.repository == repository && kept.number == number && kept.head == head {
                return true;
            }
        }
        false
    }

    fn repository(&mut self, name: &[u8]) -> Option<u32> {
        let mut vacant = None;
        for index in 0..self.repositories.len() {
            let repository = self.repositories.get(index).expect("repository slot");
            match &repository.name {
                Some(kept) if &**kept == name => return Some(index),
                Some(_) => {}
                None => {
                    if vacant.is_none() {
                        vacant = Some(index);
                    }
                }
            }
        }
        let Some(index) = vacant else {
            self.full = true;
            return None;
        };
        self.repositories.get_mut(index).expect("vacant repository").name = Some(copy_of(name));
        Some(index)
    }

    fn flush_deletion(&mut self, next: &Observation) {
        let Some(deletion) = self.deletion.take() else { return };
        let repository = self.repositories.get_mut(deletion.repository).expect("pending repository");
        match next {
            Observation::Group { repository: name, commit, ended: true, .. }
                if &**name == repository.name() && *commit == deletion.at =>
            {
                return;
            }
            Observation::Moved { .. }
            | Observation::Deleted { .. }
            | Observation::Opened { .. }
            | Observation::Closed { .. }
            | Observation::Reopened { .. }
            | Observation::Labelled { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Commented { .. }
            | Observation::Edited { .. }
            | Observation::Removed { .. }
            | Observation::Reviewed { .. }
            | Observation::Reported { .. }
            | Observation::Rebased { .. }
            | Observation::Merged { .. }
            | Observation::Enqueued { .. }
            | Observation::Group { .. }
            | Observation::Dequeued { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => {}
        }
        let breach = branch_breach(repository.name(), Promise::Foreign, &deletion.branch);
        repository.record(breach, deletion.when, None, &mut self.full);
    }

    #[expect(clippy::too_many_lines, reason = "each observation names the promises it can breach")]
    fn observation(&mut self, observation: &Observation, forward: Option<bool>) {
        let name = match observation {
            Observation::Moved { repository, .. }
            | Observation::Deleted { repository, .. }
            | Observation::Opened { repository, .. }
            | Observation::Closed { repository, .. }
            | Observation::Reopened { repository, .. }
            | Observation::Labelled { repository, .. }
            | Observation::Revised { repository, .. }
            | Observation::Depends { repository, .. }
            | Observation::Requested { repository, .. }
            | Observation::Commented { repository, .. }
            | Observation::Edited { repository, .. }
            | Observation::Removed { repository, .. }
            | Observation::Reviewed { repository, .. }
            | Observation::Merged { repository, .. }
            | Observation::Enqueued { repository, .. }
            | Observation::Rejected { repository, .. } => repository,
            Observation::Rebased { .. }
            | Observation::Defined { .. }
            | Observation::Reported { .. }
            | Observation::Group { .. }
            | Observation::Dequeued { .. }
            | Observation::Refused { .. }
            | Observation::Wiki { .. } => return,
        };
        let Some(index) = self.repository(name) else { return };
        let repository = self.repositories.get_mut(index).expect("named repository");
        let profile = &self.profile;
        let when = self.when;
        match observation {
            Observation::Opened { number, kind, body, labels, by, .. } => {
                if *by == profile.client {
                    let kind = match kind {
                        Kind::Issue => Creation::Issue,
                        Kind::Pull => Creation::Pull,
                    };
                    repository.creation(profile, kind, body, when, &mut self.full);
                }
                if let Some(item) = repository.item(*number, &mut self.full) {
                    item.own = *by == profile.client;
                    item.editor = Some(*by);
                }
                repository.labelled(profile, *number, labels, false, when, &mut self.full);
            }
            Observation::Commented { number, id, body, by, .. } => {
                if *by == profile.client {
                    repository.creation(profile, Creation::Comment, body, when, &mut self.full);
                    repository.write(profile, *number, Write::Comment, when, &mut self.full);
                }
                repository.comment(*id, *by, *by == profile.client, &mut self.full);
            }
            Observation::Revised { number, by, .. } => {
                if *by == profile.client {
                    repository.write(profile, *number, Write::Revise, when, &mut self.full);
                }
                if let Some(item) = repository.item(*number, &mut self.full) {
                    item.editor = Some(*by);
                }
            }
            Observation::Edited { id, by, .. } => {
                let mut own = false;
                for comment in &repository.comments {
                    if comment.id == *id {
                        own = comment.own && comment.editor == profile.client;
                    }
                }
                if *by == profile.client && !own {
                    let breach = numbered_breach(repository.name(), b"comment", *id);
                    repository.record(breach, when, None, &mut self.full);
                }
                repository.edit(*id, *by, &mut self.full);
            }
            Observation::Removed { id, by, .. } => {
                let mut own = false;
                for comment in &repository.comments {
                    if comment.id == *id {
                        own = comment.own;
                    }
                }
                if *by == profile.client && !own {
                    let breach = numbered_breach(repository.name(), b"comment", *id);
                    repository.record(breach, when, None, &mut self.full);
                }
            }
            Observation::Labelled { number, labels, by, .. } => {
                repository.labelled(profile, *number, labels, *by == profile.client, when, &mut self.full);
            }
            Observation::Moved { branch, from, to, by, .. } => {
                let rebase = match &self.preceding {
                    Preceding::Rebase { repository, branch: rebased, from: old, to: new, by: caller } => {
                        repository == name && rebased == branch && Some(*old) == *from && new == to && caller == by
                    }
                    Preceding::Merge { .. } | Preceding::Group { .. } | Preceding::None => false,
                };
                if rebase && repository.owns_branch(profile, branch) {
                    if *by != profile.client {
                        let breach = branch_breach(name, Promise::Foreign, branch);
                        repository.record(breach, when, None, &mut self.full);
                    }
                    return;
                }
                let generated = match &self.preceding {
                    Preceding::Merge { repository, branch: base, commit, by: merger } => {
                        repository == name && base == branch && commit == to && merger == by
                    }
                    Preceding::Group { repository, commit } => repository == name && commit == to && from.is_none(),
                    Preceding::Rebase { .. } | Preceding::None => false,
                };
                if generated {
                    return;
                }
                if *by == profile.client {
                    match forward {
                        Some(false) => {
                            let breach = branch_breach(name, Promise::Forced, branch);
                            repository.record(breach, when, None, &mut self.full);
                        }
                        None if from.is_some() => self.unforwarded = true,
                        Some(true) | None => {}
                    }
                    if from.is_some() && !repository.owns_branch(profile, branch) {
                        let breach = branch_breach(name, Promise::Foreign, branch);
                        repository.record(breach, when, None, &mut self.full);
                    }
                }
                if from.is_none() {
                    repository.branch(branch, *by == profile.client, &mut self.full);
                }
            }
            Observation::Deleted { branch, at, by, .. } => {
                if *by == profile.client && !repository.owns_branch(profile, branch) {
                    self.deletion = Some(Deletion { repository: index, at: *at, branch: branch.clone(), when });
                }
            }
            Observation::Rejected { branch, by, .. } => {
                if *by == profile.client {
                    let breach = branch_breach(name, Promise::Forced, branch);
                    repository.record(breach, when, None, &mut self.full);
                }
            }
            Observation::Merged { number, head, by, .. } | Observation::Enqueued { number, head, by, .. } => {
                if *by == profile.client {
                    repository.write(profile, *number, Write::Merge, when, &mut self.full);
                    let number_bytes = Decimal::of(*number);
                    let head_bytes = Decimal::of(*head);
                    let subject = join(&[&encoded(name), number_bytes.as_bytes(), head_bytes.as_bytes()]);
                    repository.record(
                        Breach { promise: Promise::Order, subject },
                        when,
                        Some((*number, *head)),
                        &mut self.full,
                    );
                }
            }
            Observation::Depends { number, by, .. }
            | Observation::Requested { number, by, .. }
            | Observation::Reviewed { number, by, .. }
            | Observation::Closed { number, by, .. }
            | Observation::Reopened { number, by, .. } => {
                let write = match observation {
                    Observation::Depends { .. } => Write::Depends,
                    Observation::Requested { .. } => Write::Request,
                    Observation::Reviewed { .. } => Write::Review,
                    Observation::Closed { .. } | Observation::Reopened { .. } => Write::Close,
                    Observation::Moved { .. }
                    | Observation::Deleted { .. }
                    | Observation::Opened { .. }
                    | Observation::Labelled { .. }
                    | Observation::Revised { .. }
                    | Observation::Defined { .. }
                    | Observation::Commented { .. }
                    | Observation::Edited { .. }
                    | Observation::Removed { .. }
                    | Observation::Reported { .. }
                    | Observation::Rebased { .. }
                    | Observation::Merged { .. }
                    | Observation::Enqueued { .. }
                    | Observation::Group { .. }
                    | Observation::Dequeued { .. }
                    | Observation::Refused { .. }
                    | Observation::Rejected { .. }
                    | Observation::Wiki { .. } => unreachable!("item write arm"),
                };
                if *by == profile.client {
                    repository.write(profile, *number, write, when, &mut self.full);
                }
            }
            Observation::Rebased { .. }
            | Observation::Defined { .. }
            | Observation::Reported { .. }
            | Observation::Group { .. }
            | Observation::Dequeued { .. }
            | Observation::Refused { .. }
            | Observation::Wiki { .. } => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Write {
    Revise,
    Depends,
    Comment,
    Request,
    Review,
    Close,
    Merge,
}

impl Repository {
    fn name(&self) -> &[u8] {
        self.name.as_deref().expect("a named repository")
    }

    fn contains(&self, breach: &Breach) -> bool {
        for evidence in &self.evidence {
            if evidence.breach == *breach {
                return true;
            }
        }
        false
    }

    fn record(&mut self, breach: Breach, when: u64, decision: Option<(u64, u64)>, full: &mut bool) {
        if !self.contains(&breach) && self.evidence.push(Evidence { breach, when, decision }).is_err() {
            *full = true;
        }
    }

    fn item(&mut self, number: u64, full: &mut bool) -> Option<&mut Item> {
        for index in 0..self.items.len() {
            if self.items.get(index).expect("item slot").number == number {
                return self.items.get_mut(index);
            }
        }
        if self.items.push(Item { number, own: false, editor: None, baseline: false }).is_err() {
            *full = true;
            return None;
        }
        self.items.get_mut(self.items.len().checked_sub(1).expect("item just pushed"))
    }

    fn comment(&mut self, id: u64, editor: u64, own: bool, full: &mut bool) {
        for index in 0..self.comments.len() {
            let comment = self.comments.get_mut(index).expect("comment slot");
            if comment.id == id {
                comment.editor = editor;
                comment.own = own;
                return;
            }
        }
        if self.comments.push(Comment { id, own, editor }).is_err() {
            *full = true;
        }
    }

    fn edit(&mut self, id: u64, editor: u64, full: &mut bool) {
        for index in 0..self.comments.len() {
            let comment = self.comments.get_mut(index).expect("comment slot");
            if comment.id == id {
                comment.editor = editor;
                return;
            }
        }
        self.comment(id, editor, false, full);
    }

    fn branch(&mut self, name: &[u8], own: bool, full: &mut bool) {
        for index in 0..self.branches.len() {
            let branch = self.branches.get_mut(index).expect("branch slot");
            if &*branch.name == name {
                branch.own = own;
                return;
            }
        }
        if self.branches.push(Branch { name: copy_of(name), own }).is_err() {
            *full = true;
        }
    }

    fn owns_branch(&self, profile: &Profile, name: &[u8]) -> bool {
        for prefix in &profile.owned_branches {
            if name.starts_with(prefix) {
                return true;
            }
        }
        for branch in &self.branches {
            if &*branch.name == name {
                return branch.own;
            }
        }
        false
    }

    fn creation(&mut self, profile: &Profile, kind: Creation, body: &[u8], when: u64, full: &mut bool) {
        let Some(start) = find(body, &profile.marker.0) else { return };
        let start = start.checked_add(profile.marker.0.len()).expect("marker within body");
        let Some(end) = find_from(body, &profile.marker.1, start) else { return };
        let bytes = body.get(start..end).expect("markers within body");
        for key in &self.keys {
            if key.kind == kind && &*key.bytes == bytes {
                let subject = join(&[&encoded(self.name()), kind.name(), &encoded(bytes)]);
                self.record(Breach { promise: Promise::Once, subject }, when, None, full);
                return;
            }
        }
        if self.keys.push(Key { kind, bytes: copy_of(bytes) }).is_err() {
            *full = true;
        }
    }

    fn write(&mut self, profile: &Profile, number: u64, write: Write, when: u64, full: &mut bool) {
        let Some(item) = self.item(number, full) else { return };
        let own = item.own;
        let overwritten = write == Write::Revise && item.editor.is_some() && item.editor != Some(profile.client);
        if (!own && !allowed(profile, write)) || overwritten {
            let breach = numbered_breach(self.name(), b"item", number);
            self.record(breach, when, None, full);
        }
    }

    fn labelled(
        &mut self,
        profile: &Profile,
        number: u64,
        labels: &[Box<[u8]>],
        judge: bool,
        when: u64,
        full: &mut bool,
    ) {
        let Some(item) = self.item(number, full) else { return };
        let judge = judge && item.baseline && !item.own;
        item.baseline = true;
        let mut foreign = false;
        for index in 0..self.labels.len() {
            let label = self.labels.get_mut(index).expect("label slot");
            if label.number == number {
                let present = labels.contains(&label.name);
                if judge && present != label.present && !allowed_label(profile, &label.name) {
                    foreign = true;
                }
                label.present = present;
            }
        }
        for name in labels {
            let mut seen = false;
            for label in &self.labels {
                if label.number == number && label.name == *name {
                    seen = true;
                }
            }
            if !seen {
                if judge && !allowed_label(profile, name) {
                    foreign = true;
                }
                if self.labels.push(Label { number, name: name.clone(), present: true }).is_err() {
                    *full = true;
                }
            }
        }
        if foreign {
            let breach = numbered_breach(self.name(), b"item", number);
            self.record(breach, when, None, full);
        }
    }
}

fn allowed(profile: &Profile, write: Write) -> bool {
    for allowance in &profile.allowed {
        let permits = match allowance {
            Allowance::Comment => write == Write::Comment,
            Allowance::Labels(_) => false,
            Allowance::Request => write == Write::Request,
            Allowance::Review => write == Write::Review,
            Allowance::Close => write == Write::Close,
            Allowance::Merge => write == Write::Merge,
        };
        if permits {
            return true;
        }
    }
    false
}

fn allowed_label(profile: &Profile, label: &[u8]) -> bool {
    for allowance in &profile.allowed {
        match allowance {
            Allowance::Labels(prefix) if label.starts_with(prefix) => return true,
            Allowance::Labels(_)
            | Allowance::Comment
            | Allowance::Request
            | Allowance::Review
            | Allowance::Close
            | Allowance::Merge => {}
        }
    }
    false
}

fn branch_breach(repository: &[u8], promise: Promise, branch: &[u8]) -> Breach {
    let subject = if promise == Promise::Foreign {
        join(&[&encoded(repository), b"branch", &encoded(branch)])
    } else {
        join(&[&encoded(repository), &encoded(branch)])
    };
    Breach { promise, subject }
}

fn numbered_breach(repository: &[u8], kind: &[u8], number: u64) -> Breach {
    let number = Decimal::of(number);
    Breach { promise: Promise::Foreign, subject: join(&[&encoded(repository), kind, number.as_bytes()]) }
}

fn join(parts: &[&[u8]]) -> Box<[u8]> {
    let mut len = parts.len().saturating_sub(1);
    for part in parts {
        len = len.checked_add(part.len()).expect("subject length fits in memory");
    }
    let mut writer = Writer::new(len);
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            writer.put(b"/").expect("sized separator");
        }
        writer.put(part).expect("sized subject part");
    }
    writer.finish()
}

fn encoded(bytes: &[u8]) -> Box<[u8]> {
    let mut len = 0_usize;
    for byte in bytes {
        len = len.checked_add(if unreserved(*byte) { 1 } else { 3 }).expect("encoded name fits in memory");
    }
    let mut writer = Writer::new(len);
    for byte in bytes {
        if unreserved(*byte) {
            writer.put(&[*byte]).expect("sized name byte");
        } else {
            let high = hex(byte.checked_div(16).expect("nonzero divisor"));
            let low = hex(byte.checked_rem(16).expect("nonzero divisor"));
            writer.put(&[b'%', high, low]).expect("sized escaped byte");
        }
    }
    writer.finish()
}

fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~".contains(&byte)
}

fn hex(nibble: u8) -> u8 {
    *b"0123456789ABCDEF".get(usize::from(nibble)).expect("a hex nibble")
}
