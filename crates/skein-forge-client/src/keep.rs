//! Bounded keeping-up of live forge resources (forge-client.md, section 5).
//!
//! The working set holds live rows, scan positions, inbox delivery stamps,
//! and observed heads. It never discovers resources from labels or reads
//! task policy. Admission adds a complete watch set; hints advance due
//! reads; answers update durable positions and publish changed facts;
//! departure erases rows after their pending reads settle.
pub(crate) mod answer;
pub(crate) mod echo;
pub(crate) mod poll;
pub(crate) mod restore;
use crate::calls::{self, Owner};
use crate::domain::Alarm;
use crate::{
    Cached, Delivery, Domain, Echo, Entry, Fact, Key, Limits, LiveRecord, Position, Priority, RepositoryRecord,
    Request, Resource, Stored, Watch, What,
};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Change, Commit, Error, Op, Read, Repository};
use skein_lib::{Duration, Env, List, Map, Queue, Time, Token};
#[derive(Debug)]
pub(crate) struct Keep {
    live: Map<Resource, Live>,
    repos: Map<Repository, Repo>,
    sequence: u64,
    ready: bool,
    failed: bool,
    fresh_active: bool,
}
#[derive(Debug)]
struct Live {
    record: LiveRecord,
    alarm: u64,
    active: bool,
    fresh: Fresh,
    writer: bool,
    presence: Presence,
    pushed_by: Option<(Commit, u64)>,
    read: Sent,
    hinted: bool,
    state: State,
    interval: Duration,
    echo_capacity: u32,
    inbox: Inbox,
}
// Which side of this client's own push the branch read in flight was sent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sent {
    Current,
    // A read sent before a push this client reported answers from before
    // that push, so it is read again.
    BeforePush,
}
// Presence is live-only; a restart observes absence again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Presence {
    Unknown,
    Present,
    Missing,
}
#[derive(Debug)]
struct Inbox {
    repair_at: Time,
    needs_repair: bool,
    reviews_needed: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Idle,
    Due(Phase, Priority),
    Busy(Phase, Priority),
    Waiting(Phase, Priority),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    Object,
    Pull,
    Reviews(u32),
    Branch,
    BranchCompare { before: Commit, after: Commit },
}
#[derive(Debug)]
struct Repo {
    record: RepositoryRecord,
    active: bool,
    fresh: Fresh,
    state: Scan,
    hinted: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fresh {
    Pending,
    Complete,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Scan {
    Idle,
    Due(Progress),
    Busy(Progress),
    Waiting(Progress),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Progress {
    since: Time,
    page: u32,
    newest: Time,
    begun: Option<Time>,
    tied: Option<Time>,
    moved: bool,
    bootstrap: bool,
}
impl Progress {
    fn first(mark: Option<Time>) -> Progress {
        let since = mark.unwrap_or(Time::from_nanos(u64::MAX));
        Progress { since, page: 1, newest: since, begun: None, tied: None, moved: false, bootstrap: mark.is_none() }
    }
}
impl Keep {
    pub(crate) fn new(l: &Limits) -> Keep {
        Keep {
            live: Map::with_capacity(l.resources),
            repos: Map::with_capacity(l.repositories),
            sequence: 0,
            ready: false,
            failed: false,
            fresh_active: false,
        }
    }
    pub(crate) fn is_ready(&self) -> bool {
        if !self.ready || self.failed {
            return false;
        }
        for (_, row) in &self.live {
            match row.state {
                State::Due(_, _) if row.active => return true,
                State::Due(_, _) | State::Idle | State::Busy(_, _) | State::Waiting(_, _) => {}
            }
        }
        for (_, row) in &self.repos {
            match row.state {
                Scan::Due(_) if row.active => return true,
                Scan::Due(_) | Scan::Idle | Scan::Busy(_) | Scan::Waiting(_) => {}
            }
        }
        false
    }
    pub(crate) const fn failed(&self) -> bool {
        self.failed
    }
    pub(crate) fn cached(&self, resource: &Resource) -> Option<&Cached> {
        match self.live.get(resource) {
            Some(row) if row.active => Some(&row.record.cached),
            Some(_) | None => None,
        }
    }
}
pub(crate) fn worst_case(l: &Limits) -> Option<u64> {
    l.resources.checked_mul(2)?.checked_add(l.repositories.checked_mul(2)?)?.checked_add(1)?;
    let per = u64::try_from(size_of::<Cached>())
        .ok()?
        .checked_add(u64::from(l.op_bytes).checked_mul(6)?)?
        .checked_add(u64::from(l.answer_bytes).checked_mul(2)?)?
        .checked_add(u64::from(l.rows).checked_mul(u64::try_from(size_of::<Echo>()).ok()?)?)?
        .checked_add(u64::from(l.inbox).checked_mul(u64::try_from(size_of::<Delivery>()).ok()?.checked_mul(2)?)?)?;
    Map::<Resource, Live>::worst_case(l.resources)?
        .checked_add(Map::<Repository, Repo>::worst_case(l.repositories)?)?
        .checked_add(List::<Resource>::worst_case(l.resources)?.checked_mul(2)?)?
        .checked_add(List::<Repository>::worst_case(l.repositories)?.checked_mul(2)?)?
        .checked_add(u64::from(l.resources).checked_mul(per)?)?
        // A response is checked before cloning. Replacement can briefly own
        // old cache, incoming data and its retained copy. Delivered row lists
        // reserve rows, independently of the byte cap on one answer.
        .checked_add(u64::from(l.answer_bytes).checked_mul(2)?)?
        .checked_add(u64::from(l.op_bytes).checked_mul(2)?)?
        .checked_add(List::<api::Comment>::worst_case(l.rows)?)?
        .checked_add(List::<api::Review>::worst_case(l.rows)?)?
        .checked_add(List::<Echo>::worst_case(l.rows)?)?
        .checked_add(List::<Delivery>::worst_case(l.inbox)?)
}
fn valid_resource(r: &Resource, l: &Limits) -> bool {
    match &r.what {
        What::Branch(name) => !name.is_empty() && name.len() <= usize::try_from(l.op_bytes).expect("u32 fits usize"),
        What::Repository | What::Pull(_) | What::Issue(_) => true,
    }
}
fn first(resource: &Resource) -> Phase {
    match resource.what {
        What::Branch(_) => Phase::Branch,
        What::Pull(_) | What::Issue(_) | What::Repository => Phase::Object,
    }
}
fn live(record: LiveRecord, l: &Limits, alarm: u64) -> Live {
    let phase = first(&record.watch.resource);
    let needs_repair = !record.comment_scanning;
    Live {
        record,
        alarm,
        active: true,
        fresh: Fresh::Complete,
        writer: false,
        presence: Presence::Unknown,
        pushed_by: None,
        read: Sent::Current,
        hinted: false,
        state: State::Due(phase, Priority::Keep),
        interval: l.poll,
        echo_capacity: l.rows,
        inbox: Inbox { repair_at: Time::ZERO, needs_repair, reviews_needed: false },
    }
}
fn repository(record: RepositoryRecord) -> Repo {
    let progress = Progress::first(record.mark);
    Repo { record, active: true, fresh: Fresh::Complete, state: Scan::Due(progress), hinted: false }
}
fn has(watches: &[Watch], resource: &Resource) -> bool {
    for watch in watches {
        if watch.resource == *resource {
            return true;
        }
    }
    false
}
fn has_repo(watches: &[Watch], repo: Repository) -> bool {
    for watch in watches {
        if watch.resource.repository == repo {
            return true;
        }
    }
    false
}
fn busy(state: State) -> bool {
    match state {
        State::Busy(_, _) => true,
        State::Idle | State::Due(_, _) | State::Waiting(_, _) => false,
    }
}
fn scan_busy(state: Scan) -> bool {
    match state {
        Scan::Busy(_) => true,
        Scan::Idle | Scan::Due(_) | Scan::Waiting(_) => false,
    }
}
fn admission(k: &Keep, l: &Limits, watches: &[Watch]) -> Result<(), Error> {
    if watches.len() > usize::try_from(l.resources).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    if !k.ready || k.failed {
        return Err(Error::Busy);
    }
    for (index, watch) in watches.iter().enumerate() {
        if !valid_resource(&watch.resource, l) {
            return Err(Error::TooLarge);
        }
        if watch.participating {
            match watch.resource.what {
                What::Pull(_) | What::Issue(_) => {}
                What::Repository | What::Branch(_) => return Err(Error::Refused),
            }
        }
        for prior in watches.get(..index).expect("prior slice") {
            if prior.resource == watch.resource {
                return Err(Error::Refused);
            }
        }
    }
    let mut resources = u32::try_from(watches.len()).expect("bounded length");
    for (key, row) in &k.live {
        if !has(watches, key) && busy(row.state) {
            resources = resources.saturating_add(1);
        }
    }
    let mut repositories = 0_u32;
    for (index, watch) in watches.iter().enumerate() {
        if !has_repo(watches.get(..index).expect("prior slice"), watch.resource.repository) {
            repositories = repositories.saturating_add(1);
        }
    }
    for (key, row) in &k.repos {
        if !has_repo(watches, *key) && scan_busy(row.state) {
            repositories = repositories.saturating_add(1);
        }
    }
    if resources > l.resources || repositories > l.repositories { Err(Error::Busy) } else { Ok(()) }
}
pub(crate) fn replace(
    d: &mut Domain,
    env: &Env<Limits>,
    owner: Token,
    watches: Box<[Watch]>,
    out: &mut Queue<Request>,
) {
    if let Err(error) = admission(&d.keep, &env.limits, &watches) {
        d.fact(Fact::Refused);
        out.push(Request::Kept { owner, result: Err(error) });
        return;
    }
    remove_live(d, &watches, out);
    remove_repos(d, &watches, out);
    for watch in watches {
        let key = watch.resource.clone();
        match d.keep.live.get_mut(&key) {
            Some(row) => {
                let changed = row.record.watch.participating != watch.participating || !row.active;
                row.active = true;
                row.record.watch = watch;
                if changed {
                    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
                    row.hinted = true;
                }
            }
            None => {
                let record = LiveRecord {
                    watch,
                    position: Position { at: Time::ZERO, comment: 0, review: 0, review_page: 1 },
                    listed: Time::ZERO,
                    comment_after: 0,
                    comment_page: 1,
                    comment_scanning: true,
                    comment_repair: true,
                    review_page: 1,
                    comments: Box::new([]),
                    reviews: Box::new([]),
                    cached: Box::new(Cached { item: None, pull: None, tip: None }),
                    pushed: None,
                    echoes: Box::new([]),
                };
                out.push(Request::Save { record: Stored::Live(record.clone()) });
                d.keep
                    .live
                    .insert(key.clone(), live(record, &env.limits, next_id(&mut d.keep.sequence)))
                    .expect("batch admitted");
            }
        }
        let repo = key.repository;
        match d.keep.repos.get_mut(&repo) {
            Some(row) => {
                if !row.active {
                    out.push(Request::Save { record: Stored::Repository(row.record) });
                }
                row.active = true;
            }
            None => {
                let record = RepositoryRecord { repository: repo, mark: None, clock: Time::ZERO };
                out.push(Request::Save { record: Stored::Repository(record) });
                d.keep.repos.insert(repo, repository(record)).expect("batch admitted");
            }
        }
    }
    out.push(Request::Kept { owner, result: Ok(()) });
}
fn remove_live(d: &mut Domain, watches: &[Watch], out: &mut Queue<Request>) {
    let mut removed = List::with_capacity(d.keep.live.len());
    for key in keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if !has(watches, key) {
            if row.active {
                out.push(Request::Erase { key: Key::Live(key.clone()) });
            }
            row.active = false;
            d.alarms.cancel(Alarm::Resource(row.alarm));
            if busy(row.state) && calls::cancel_resource(&mut d.calls, key) {
                row.state = State::Idle;
            }
            if !busy(row.state) {
                removed.push(key.clone()).expect("bounded removals");
            }
        }
    }
    for key in removed.into_boxed() {
        d.keep.live.remove(&key).expect("removed row exists");
    }
}
fn remove_repos(d: &mut Domain, watches: &[Watch], out: &mut Queue<Request>) {
    let mut removed = List::with_capacity(d.keep.repos.len());
    for key in repos(&d.keep) {
        let row = d.keep.repos.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if !has_repo(watches, *key) {
            if row.active {
                out.push(Request::Erase { key: Key::Repository(*key) });
            }
            row.active = false;
            d.alarms.cancel(Alarm::Repository(*key));
            if scan_busy(row.state) && calls::cancel_repository(&mut d.calls, *key) {
                row.state = Scan::Idle;
            }
            if !scan_busy(row.state) {
                removed.push(*key).expect("bounded removals");
            }
        }
    }
    for key in removed.into_boxed() {
        d.keep.repos.remove(&key).expect("removed row exists");
    }
}
fn keys(k: &Keep) -> Box<[Resource]> {
    let mut keys = List::with_capacity(k.live.len());
    for (key, _) in &k.live {
        keys.push(key.clone()).expect("bounded live keys");
    }
    keys.into_boxed()
}
fn repos(k: &Keep) -> Box<[Repository]> {
    let mut keys = List::with_capacity(k.repos.len());
    for (key, _) in &k.repos {
        keys.push(*key).expect("bounded repository keys");
    }
    keys.into_boxed()
}
fn next_id(sequence: &mut u64) -> u64 {
    *sequence = sequence.checked_add(1).expect("alarm identities do not exhaust in a process");
    *sequence
}

fn priority(row: &Live) -> Priority {
    match row.state {
        State::Due(_, priority) | State::Busy(_, priority) | State::Waiting(_, priority) => priority,
        State::Idle => unreachable!("a pipeline advances while active"),
    }
}
pub(crate) fn active_resource(k: &Keep, key: &Resource) -> bool {
    match k.live.get(key) {
        Some(row) => row.active,
        None => false,
    }
}
pub(crate) fn active_repository(k: &Keep, key: Repository) -> bool {
    match k.repos.get(&key) {
        Some(row) => row.active,
        None => false,
    }
}

fn valid_participation(watch: &Watch) -> bool {
    match watch.resource.what {
        What::Pull(_) | What::Issue(_) => true,
        What::Repository | What::Branch(_) => !watch.participating,
    }
}
