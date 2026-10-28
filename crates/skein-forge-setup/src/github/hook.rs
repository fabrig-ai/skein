//! Read and repair the App's URL-only hook (forge-setup.md, 5.5, 5.6 and 5.8).
use super::app::{self, Kept};
use crate::reconcile::{self, Event, Need, Reply, Stage, Why};
use alloc::boxed::Box;
use skein_forge::github::{
    document::{Document, Hook as Config},
    request::Request,
};
use skein_lib::{Queue, Writer, bytes};

#[derive(Debug)]
enum State {
    Begin(Kept),
    Read { kept: Kept, url: Box<[u8]> },
    Patch,
    Ended,
}

/// A child's one pending host need, or its internal completion. The parent
/// forwards Pending and continues on Complete; the child emits no public Done.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Progress {
    Pending(Need),
    Complete,
}

/// One bounded hook check; only the read phase retains the App's credential
/// for a possible patch (forge-setup.md, 5.5, 5.6, step 3, and 5.8, step 4).
#[derive(Debug)]
pub(crate) struct Hook {
    forge: u16,
    slug: Box<[u8]>,
    repair: bool,
    state: State,
}
impl Hook {
    /// Takes a slug already admitted by `item::app_slug`. The parent's admitted
    /// name bound bounds both the subject and the measured hook URL (5.5).
    pub(crate) fn new(forge: u16, kept: Kept, slug: &[u8], repair: bool) -> Self {
        Self { forge, slug: bytes::copy_of(slug), repair, state: State::Begin(kept) }
    }
    fn stop(&self, why: Why) -> Progress {
        let subject = if why == Why::KeyForm { b"key" } else { self.slug.as_ref() };
        Progress::Pending(app::stop(self.forge, Stage::App, subject, why))
    }
}

fn url(slug: &[u8]) -> Option<Box<[u8]>> {
    let length = b"https://".len().checked_add(slug.len())?.checked_add(b".invalid/".len())?;
    let mut out = Writer::new(length);
    out.put(b"https://").ok()?;
    out.put(slug).ok()?;
    out.put(b".invalid/").ok()?;
    Some(out.finish())
}

fn config(event: Event) -> Result<Config, Why> {
    match reconcile::reply(event)? {
        Reply::GithubDocument(Document::HookConfig(config)) => Ok(config),
        Reply::Unsigned => Err(Why::KeyForm),
        Reply::Down => Err(Why::Down),
        Reply::Status(status) => Err(Why::Refused(status)),
        Reply::Document(_) | Reply::Rows(_) | Reply::Empty | Reply::GithubDocument(_) | Reply::GithubRows(_) => {
            Err(Why::Unexpected)
        }
    }
}

/// Consumes one event and emits its next need; the parent forwards its one
/// pending need. Completion and failure release credentials, and ended children
/// stay silent (forge-setup.md, 5.6 and 5.8). No hook secret is read or sent.
pub(crate) fn step(machine: &mut Hook, event: Event, out: &mut Queue<Progress>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    let progress = match state {
        State::Begin(kept) => match event {
            Event::Begin => match url(&machine.slug) {
                Some(url) => {
                    let auth = kept.auth();
                    machine.state = State::Read { kept, url };
                    Progress::Pending(Need::Send { request: Request::HookConfig.into(), auth })
                }
                None => machine.stop(Why::Unexpected),
            },
            Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => machine.stop(Why::Unexpected),
        },
        State::Read { kept, url } => match config(event) {
            Ok(config) => {
                if config.url == url || !machine.repair {
                    Progress::Complete
                } else {
                    let auth = kept.auth();
                    machine.state = State::Patch;
                    Progress::Pending(Need::Send { request: Request::SetHook { url }.into(), auth })
                }
            }
            Err(why) => machine.stop(why),
        },
        State::Patch => match config(event) {
            Ok(_) => Progress::Complete,
            Err(why) => machine.stop(why),
        },
        State::Ended => return,
    };
    out.push(progress);
}
