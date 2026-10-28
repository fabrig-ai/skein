//! Prune complete listings and uninstall the strays seen (forge-setup.md, 5.6, step 4).
use super::{
    app::{self, Kept},
    given::Given,
    limits::Limits,
};
use crate::{
    item::{self, Item},
    reconcile::{self, Event, Need, Reply, Stage, Why},
};
use alloc::boxed::Box;
use skein_forge::github::{document::Row, pin::PAGE_CAP, request::Request};
use skein_lib::{Duration, List, Queue, Set, Wall};

#[derive(Debug)]
struct Reading {
    kept: Kept,
    accepted: Box<[u64]>,
    seen: Set<u64>,
    live: Set<u64>,
    strays: Set<u64>,
    page: u32,
}
#[derive(Debug)]
struct Deleting {
    kept: Kept,
    strays: Set<u64>,
    crowded: bool,
}
#[derive(Debug)]
enum State {
    Begin { kept: Kept, accepted: Box<[u64]> },
    Read(Reading),
    Written(Deleting),
    Delete { work: Deleting, id: u64 },
    Ended,
}

/// A pending host need or an internal completion, as in the hook child.
/// The parent continues on Complete; the child emits no public Done.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Progress {
    Pending(Need),
    Complete,
}

/// A bounded sweep against one parent-supplied wall snapshot. Only ids survive
/// each page; credentials and all scratch belong to the live states (5.6).
#[derive(Debug)]
pub(crate) struct Sweep {
    forge: u16,
    read_only: bool,
    pages: u32,
    installations: u32,
    life: Duration,
    wall: Wall,
    state: State,
}
impl Sweep {
    /// Takes checked Given, the retained App and ids decoded by `item::accepted`.
    pub(crate) fn new(given: &Given, kept: Kept, accepted: Box<[u64]>, wall: Wall) -> Self {
        Self {
            forge: given.forge,
            read_only: given.read_only,
            pages: given.limits.pages,
            installations: given.limits.installations,
            life: given.limits.state_life,
            wall,
            state: State::Begin { kept, accepted },
        }
    }
    fn stop(&self, subject: &[u8], why: Why) -> Progress {
        let need = if why == Why::KeyForm {
            app::stop(self.forge, Stage::App, b"key", why)
        } else {
            app::stop(self.forge, Stage::Installation, subject, why)
        };
        Progress::Pending(need)
    }
    fn page(&mut self, mut read: Reading, event: Event) -> Progress {
        let rows = match rows(event) {
            Ok(rows) => rows,
            Err(why) => return self.stop(b"installations", why),
        };
        if rows.is_empty() {
            return self.discovered(read);
        }
        for row in rows {
            let row = match row {
                Row::Installation(row) => row,
                Row::Log(_)
                | Row::Reachable(_)
                | Row::Branch(_)
                | Row::SkippedBranch
                | Row::Issue(_)
                | Row::Comment(_)
                | Row::Pull(_)
                | Row::Context(_)
                | Row::Run(_)
                | Row::Collaborator(_)
                | Row::Remark(_)
                | Row::Review(_)
                | Row::Commit(_)
                | Row::Rule(_)
                | Row::Entry { .. }
                | Row::File(_) => {
                    return self.stop(b"installations", Why::Unexpected);
                }
            };
            if row.id == 0 || read.seen.insert(row.id) != Ok(true) {
                return self.stop(b"installations", Why::Unexpected);
            }
            if read.accepted.binary_search(&row.id).is_ok() {
                if read.live.insert(row.id).is_err() {
                    return self.stop(b"installations", Why::Unexpected);
                }
            } else if !self.read_only {
                let age = self.wall.as_nanos().checked_sub(row.created.as_nanos());
                if let Some(age) = age
                    && age > self.life.as_nanos()
                    && read.strays.insert(row.id).is_err()
                {
                    return self.stop(b"installations", Why::Unexpected);
                }
            }
        }
        // A nonempty response at the bound permits deletes, but no prune.
        if read.page >= self.pages {
            return self.delete_next(Deleting { kept: read.kept, strays: read.strays, crowded: true });
        }
        let Some(page) = read.page.checked_add(1) else {
            return self.stop(b"installations", Why::Unexpected);
        };
        read.page = page;
        let auth = read.kept.auth();
        self.state = State::Read(read);
        Progress::Pending(Need::Send { request: Request::Installations { page }.into(), auth })
    }
    fn discovered(&mut self, read: Reading) -> Progress {
        let Reading { kept, accepted, live, strays, seen: _, page: _ } = read;
        let work = Deleting { kept, strays, crowded: false };
        if live.iter().eq(accepted.iter()) {
            return self.delete_next(work);
        }
        let mut retained = List::with_capacity(live.len());
        for id in &live {
            retained.push(*id).expect("one id per live accepted entry");
        }
        let bytes = item::accepted_item(retained.as_slice());
        self.state = State::Written(work);
        Progress::Pending(Need::Write { item: Item::Accepted, bytes })
    }
    fn delete_next(&mut self, mut work: Deleting) -> Progress {
        match work.strays.pop_first() {
            Some(id) => {
                let auth = work.kept.auth();
                self.state = State::Delete { work, id };
                Progress::Pending(Need::Send { request: Request::Uninstall { id }.into(), auth })
            }
            None => {
                if work.crowded {
                    self.stop(b"installations", Why::Crowded)
                } else {
                    Progress::Complete
                }
            }
        }
    }
}

fn rows(event: Event) -> Result<Box<[Row]>, Why> {
    match reconcile::reply(event)? {
        Reply::GithubRows(rows) => {
            if rows.len() > usize::try_from(PAGE_CAP).expect("page cap fits usize") {
                return Err(Why::Unexpected);
            }
            Ok(rows)
        }
        Reply::Unsigned => Err(Why::KeyForm),
        Reply::Down => Err(Why::Down),
        Reply::Status(status) => Err(Why::Refused(status)),
        Reply::Document(_) | Reply::Rows(_) | Reply::Empty | Reply::GithubDocument(_) => Err(Why::Unexpected),
    }
}
fn deleted(event: Event) -> Result<(), Why> {
    match reconcile::reply(event)? {
        Reply::Empty | Reply::Status(404) => Ok(()),
        Reply::Unsigned => Err(Why::KeyForm),
        Reply::Down => Err(Why::Down),
        Reply::Status(status) => Err(Why::Refused(status)),
        Reply::Document(_) | Reply::Rows(_) | Reply::GithubDocument(_) | Reply::GithubRows(_) => Err(Why::Unexpected),
    }
}

/// Consumes one event and emits its next need, emitting at most one need
/// or internal completion. Every end drops the key and scratch; later events
/// are silent. Invalid or failed listing replies cannot prune or delete (5.6,
/// step 4).
pub(crate) fn step(machine: &mut Sweep, event: Event, out: &mut Queue<Progress>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    let progress = match state {
        State::Begin { kept, accepted } => match event {
            Event::Begin => match machine.pages.checked_mul(PAGE_CAP) {
                Some(capacity) => {
                    let auth = kept.auth();
                    machine.state = State::Read(Reading {
                        kept,
                        accepted,
                        seen: Set::with_capacity(capacity),
                        live: Set::with_capacity(machine.installations),
                        strays: Set::with_capacity(capacity),
                        page: 1,
                    });
                    Progress::Pending(Need::Send { request: Request::Installations { page: 1 }.into(), auth })
                }
                None => machine.stop(b"installations", Why::Unexpected),
            },
            Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                machine.stop(b"installations", Why::Unexpected)
            }
        },
        State::Read(read) => machine.page(read, event),
        State::Written(work) => match event {
            Event::Written => machine.delete_next(work),
            Event::Begin | Event::Read(_) | Event::Reply(_) | Event::Drawn(_) => {
                machine.stop(b"installations", Why::Unexpected)
            }
        },
        State::Delete { work, id } => match deleted(event) {
            Ok(()) => machine.delete_next(work),
            Err(why) => machine.stop(&app::decimal(id), why),
        },
        State::Ended => return,
    };
    out.push(progress);
}

/// Heap for both discovery sets, live accepted ids, retained ids and their
/// encoded item, two App key copies (kept and outgoing auth), and the decimal
/// scratch and copied failure subject. Parent-owned replies and output queues
/// are counted there.
pub(crate) fn worst_case(limits: &Limits) -> Option<u64> {
    let capacity = limits.pages.checked_mul(PAGE_CAP)?;
    Set::<u64>::worst_case(capacity)?
        .checked_mul(2)?
        .checked_add(Set::<u64>::worst_case(limits.installations)?)?
        .checked_add(List::<u64>::worst_case(limits.installations)?.checked_mul(3)?)?
        .checked_add(u64::from(limits.key_bytes).checked_mul(2)?)?
        .checked_add(40)
}
