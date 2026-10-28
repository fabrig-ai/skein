//! Prove the kept App, repair its hook and sweep installations (forge-setup.md, 5.6).
use super::{
    app::{self, Kept},
    checks::{self, Lacking},
    given::{self, Given, Refused},
    hook::{self, Hook},
    installations::{self, Sweep},
};
use crate::{
    item::{self, Item},
    reconcile::{self, Event, Need, Outcome, Reply, Stage, Why},
};
use alloc::boxed::Box;
use core::num::NonZeroU64;
use skein_forge::github::{
    document::{App, Document},
    request::Request,
};
use skein_lib::{List, Queue, Wall};

#[derive(Debug)]
struct Checked {
    kept: Kept,
    accepted: Box<[u64]>,
    slug: Box<[u8]>,
    lacking: Box<[Lacking]>,
}
#[derive(Debug)]
enum State {
    Begin,
    Id,
    Key(NonZeroU64),
    Accepted(Kept),
    App { kept: Kept, accepted: Box<[u64]> },
    Hook { child: Hook, checked: Checked },
    Sweep { child: Sweep, lacking: Box<[Lacking]> },
    Ended,
}

/// One reconcile against a construction-time wall snapshot. The host supplies
/// time and signs App requests; this machine never reads a clock or holds a JWT.
#[derive(Debug)]
pub struct Reconcile {
    given: Given,
    wall: Wall,
    state: State,
}
impl Reconcile {
    /// Checks the owner's values before copying them. Items are read only on Begin.
    pub fn new(given: &Given, wall: Wall) -> Result<Self, Refused> {
        given::check(given)?;
        Ok(Self { given: given.clone(), wall, state: State::Begin })
    }
    fn stop(&self, subject: &[u8], why: Why) -> Need {
        app::stop(self.given.forge, Stage::App, subject, why)
    }
    fn hook(&mut self, mut child: Hook, checked: Checked, event: Event) -> Need {
        let mut out = Queue::uncapped();
        hook::step(&mut child, event, &mut out);
        match out.pop() {
            Some(hook::Progress::Pending(need @ Need::Stop(_))) => need,
            Some(hook::Progress::Pending(
                need @ (Need::Read(_) | Need::Write { .. } | Need::Send { .. } | Need::Draw(_)),
            )) => {
                self.state = State::Hook { child, checked };
                need
            }
            Some(hook::Progress::Complete) => {
                let child = Sweep::new(&self.given, checked.kept, checked.accepted, self.wall);
                self.sweep(child, checked.lacking, Event::Begin)
            }
            Some(hook::Progress::Pending(Need::Done(_))) | None => self.stop(&checked.slug, Why::Unexpected),
        }
    }
    fn sweep(&mut self, mut child: Sweep, lacking: Box<[Lacking]>, event: Event) -> Need {
        let mut out = Queue::uncapped();
        installations::step(&mut child, event, &mut out);
        match out.pop() {
            Some(installations::Progress::Pending(need @ Need::Stop(_))) => need,
            Some(installations::Progress::Pending(
                need @ (Need::Read(_) | Need::Write { .. } | Need::Send { .. } | Need::Draw(_)),
            )) => {
                self.state = State::Sweep { child, lacking };
                need
            }
            Some(installations::Progress::Complete) => Need::Done(Outcome::Lacking { lacking }),
            Some(installations::Progress::Pending(Need::Done(_))) | None => {
                self.stop(b"installations", Why::Unexpected)
            }
        }
    }
    fn proved(&mut self, kept: Kept, accepted: Box<[u64]>, event: Event) -> Need {
        let app = match document(event) {
            Ok(app) => app,
            Err(Why::KeyForm) => return self.stop(b"key", Why::KeyForm),
            Err(why) => return self.stop(&app::decimal(kept.id()), why),
        };
        if app.id != kept.id() {
            return self.stop(&app::decimal(kept.id()), Why::OtherApp);
        }
        if item::app_slug(&app.slug, self.given.limits.name_bytes).is_none() {
            return self.stop(&app::decimal(kept.id()), Why::Unexpected);
        }
        let lacking = checks::lacks(&app);
        let child = Hook::new(self.given.forge, kept.clone(), &app.slug, !self.given.read_only);
        let checked = Checked { kept, accepted, slug: app.slug, lacking };
        self.hook(child, checked, Event::Begin)
    }
    fn advance(&mut self, state: State, event: Event) -> Need {
        match state {
            State::Begin => match event {
                Event::Begin => {
                    self.state = State::Id;
                    Need::Read(Item::AppId)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(b"id", Why::Unexpected)
                }
            },
            State::Id => match event {
                Event::Read(None) => self.stop(b"id", Why::Missing),
                Event::Read(Some(value)) => match item::app_id(value.expose()) {
                    Some(id) => {
                        self.state = State::Key(id);
                        Need::Read(Item::AppKey)
                    }
                    None => self.stop(b"id", Why::Unexpected),
                },
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(b"id", Why::Unexpected),
            },
            State::Key(id) => match event {
                Event::Read(None) => self.stop(b"key", Why::Missing),
                Event::Read(Some(key)) => {
                    if item::app_key(key.expose(), self.given.limits.key_bytes).is_none() {
                        return self.stop(b"key", Why::Unexpected);
                    }
                    self.state = State::Accepted(Kept::new(id, key));
                    Need::Read(Item::Accepted)
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(b"key", Why::Unexpected),
            },
            State::Accepted(kept) => match event {
                Event::Read(read) => {
                    let accepted = match read {
                        Some(value) => item::accepted(value.expose(), self.given.limits.installations),
                        None => Some(Box::<[u64]>::default()),
                    };
                    let Some(accepted) = accepted else {
                        return self.stop(b"accepted", Why::Unexpected);
                    };
                    let auth = kept.auth();
                    self.state = State::App { kept, accepted };
                    Need::Send { request: Request::App.into(), auth }
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(b"accepted", Why::Unexpected)
                }
            },
            State::App { kept, accepted } => self.proved(kept, accepted, event),
            State::Hook { child, checked } => self.hook(child, checked, event),
            State::Sweep { child, lacking } => self.sweep(child, lacking, event),
            State::Ended => unreachable!("ended reconcile is not advanced"),
        }
    }
}
fn document(event: Event) -> Result<App, Why> {
    match reconcile::reply(event)? {
        Reply::GithubDocument(Document::App(app)) => Ok(app),
        Reply::Unsigned => Err(Why::KeyForm),
        Reply::Status(status) => Err(Why::Refused(status)),
        Reply::Down => Err(Why::Down),
        Reply::Document(_) | Reply::Rows(_) | Reply::Empty | Reply::GithubDocument(_) | Reply::GithubRows(_) => {
            Err(Why::Unexpected)
        }
    }
}

/// Consumes one event and emits its next need. Internal child
/// completions advance the plan; only the final acknowledgment emits Lacking.
/// Every terminal path drops the App key, missing-access result and child scratch.
pub fn step(machine: &mut Reconcile, event: Event, out: &mut Queue<Need>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    match state {
        State::Ended => (),
        State::Begin
        | State::Id
        | State::Key(_)
        | State::Accepted(_)
        | State::App { .. }
        | State::Hook { .. }
        | State::Sweep { .. } => out.push(machine.advance(state, event)),
    }
}

/// The host's terminal interruption releases all live state and ends once.
pub(crate) fn halt(machine: &mut Reconcile, why: Why, out: &mut Queue<Need>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    let need = match state {
        State::Begin | State::Id => machine.stop(b"id", why),
        State::Key(_) => machine.stop(b"key", why),
        State::Accepted(_) => machine.stop(b"accepted", why),
        State::App { kept, .. } => machine.stop(&app::decimal(kept.id()), why),
        State::Hook { checked, .. } => machine.stop(&checked.slug, why),
        State::Sweep { .. } => app::stop(machine.given.forge, Stage::Installation, b"installations", why),
        State::Ended => return,
    };
    out.push(need);
}

/// Heap owned by the boxed machine, copied Given, up to three simultaneous key
/// copies during hook repair, Accepted, bounded Lacking and its comparison
/// scratch, hook slug/URL copies, the sweep's sets. The
/// host counts its own reply and output storage separately.
pub(crate) fn worst_case(given: &Given) -> Option<u64> {
    let limits = &given.limits;
    let mut total = u64::try_from(size_of::<Reconcile>()).ok()?;
    for text in [&given.word, &given.web, &given.url, &given.redirect_url, &given.setup_url] {
        total = total.checked_add(u64::try_from(text.len()).ok()?)?;
    }
    let accepted = List::<u64>::worst_case(limits.installations)?;
    total = total.checked_add(checks::worst_case()?)?.checked_add(accepted)?.checked_add(accepted)?;
    // Hook: checked and child each keep a key; the pending auth keeps a third.
    // Checked slug, child slug, and desired URL retain three bounded names.
    let hook = u64::from(limits.key_bytes)
        .checked_mul(3)?
        .checked_add(u64::from(limits.name_bytes).checked_mul(3)?)?
        .checked_add(u64::try_from(b"https://.invalid/".len()).ok()?)?;
    let sweep = installations::worst_case(limits)?;
    // Decimal construction and its copied failure subject each use at most 20.
    total.checked_add(40)?.checked_add(hook.max(sweep))
}
