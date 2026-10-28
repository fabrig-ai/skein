//! Verify one installation before retaining its id (forge-setup.md, 5.4).
use super::{
    app,
    given::{self, Given, Refused},
};
use crate::{
    item::{self, Item},
    reconcile::{Event, Need, Outcome, Reply, Stage, Why},
};
use alloc::boxed::Box;
use core::num::NonZeroU64;
use skein_forge::github::{document::Document, request::Request};
use skein_lib::{List, Queue};

#[derive(Debug)]
enum Phase {
    Begin,
    Id,
    Key(NonZeroU64),
    Accepted(app::Kept),
    Installation(Box<[u64]>),
    Written,
    Ended,
}
/// The host admits and takes the state before this machine begins (5.7).
#[derive(Debug)]
pub struct Accept {
    forge: u16,
    installation: NonZeroU64,
    key_bytes: u32,
    installations: u32,
    phase: Phase,
}
impl Accept {
    pub fn new(given: &Given, installation: NonZeroU64) -> Result<Self, Refused> {
        given::check(given)?;
        Ok(Self {
            forge: given.forge,
            installation,
            key_bytes: given.limits.key_bytes,
            installations: given.limits.installations,
            phase: Phase::Begin,
        })
    }
    fn stop(&self, why: Why) -> Need {
        app::stop(self.forge, Stage::Installation, &app::decimal(self.installation.get()), why)
    }
    fn verified(&mut self, ids: &[u64]) -> Need {
        let id = self.installation.get();
        if ids.contains(&id) {
            return Need::Done(Outcome::Nothing);
        }
        if u32::try_from(ids.len()).expect("bounded accepted set") == self.installations {
            return self.stop(Why::Unexpected);
        }
        let mut updated = List::with_capacity(self.installations);
        let mut inserted = false;
        for kept in ids {
            if !inserted && id < *kept {
                updated.push(id).expect("room for new installation");
                inserted = true;
            }
            updated.push(*kept).expect("bounded accepted set");
        }
        if !inserted {
            updated.push(id).expect("room for new installation");
        }
        self.phase = Phase::Written;
        Need::Write { item: Item::Accepted, bytes: item::accepted_item(updated.as_slice()) }
    }
    fn advance(&mut self, phase: Phase, event: Event) -> Need {
        match phase {
            Phase::Begin => match event {
                Event::Begin => {
                    self.phase = Phase::Id;
                    Need::Read(Item::AppId)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    app::stop(self.forge, Stage::App, b"id", Why::Unexpected)
                }
            },
            Phase::Id => match event {
                Event::Read(read) => match app::id_of(read) {
                    Some(id) => {
                        self.phase = Phase::Key(id);
                        Need::Read(Item::AppKey)
                    }
                    None => app::stop(self.forge, Stage::App, b"id", Why::Missing),
                },
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    app::stop(self.forge, Stage::App, b"id", Why::Unexpected)
                }
            },
            Phase::Key(id) => match event {
                Event::Read(Some(key)) if item::app_key(key.expose(), self.key_bytes).is_some() => {
                    self.phase = Phase::Accepted(app::Kept::new(id, key));
                    Need::Read(Item::Accepted)
                }
                Event::Read(_) => app::stop(self.forge, Stage::App, b"key", Why::Missing),
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    app::stop(self.forge, Stage::App, b"key", Why::Unexpected)
                }
            },
            Phase::Accepted(app) => match event {
                Event::Read(read) => {
                    let ids = match read {
                        Some(value) => item::accepted(value.expose(), self.installations),
                        None => Some(Box::<[u64]>::default()),
                    };
                    let Some(ids) = ids else {
                        return self.stop(Why::Unexpected);
                    };
                    self.phase = Phase::Installation(ids);
                    Need::Send {
                        request: Request::Installation { id: self.installation.get() }.into(),
                        auth: app.auth(),
                    }
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(Why::Unexpected),
            },
            Phase::Installation(ids) => match event {
                Event::Reply(Reply::GithubDocument(Document::Installation(installation)))
                    if installation.id == self.installation.get() =>
                {
                    self.verified(&ids)
                }
                Event::Reply(Reply::Status(404)) => self.stop(Why::Missing),
                Event::Reply(Reply::Status(status)) => self.stop(Why::Refused(status)),
                Event::Reply(Reply::Unsigned) => app::stop(self.forge, Stage::App, b"key", Why::KeyForm),
                Event::Reply(Reply::Down) => self.stop(Why::Down),
                Event::Reply(
                    Reply::Document(_)
                    | Reply::Rows(_)
                    | Reply::Empty
                    | Reply::GithubDocument(_)
                    | Reply::GithubRows(_),
                )
                | Event::Begin
                | Event::Read(_)
                | Event::Written
                | Event::Drawn(_) => self.stop(Why::Unexpected),
            },
            Phase::Written => match event {
                Event::Written => Need::Done(Outcome::Nothing),
                Event::Begin | Event::Read(_) | Event::Reply(_) | Event::Drawn(_) => self.stop(Why::Unexpected),
            },
            Phase::Ended => unreachable!("ended acceptance is not advanced"),
        }
    }
}
/// Consumes one event and emits its next need; ended machines are silent.
pub fn step(machine: &mut Accept, event: Event, out: &mut Queue<Need>) {
    let phase = core::mem::replace(&mut machine.phase, Phase::Ended);
    match phase {
        Phase::Ended => (),
        Phase::Begin | Phase::Id | Phase::Key(_) | Phase::Accepted(_) | Phase::Installation(_) | Phase::Written => {
            let need = machine.advance(phase, event);
            out.push(need);
        }
    }
}
