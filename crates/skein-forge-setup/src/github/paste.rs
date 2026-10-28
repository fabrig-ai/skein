//! Connect or replace a pasted App while preserving identity (forge-setup.md, 5.8).
use super::{
    app::{self, Kept},
    checks::{self, Lacking},
    given::{self, Given, Refused},
    hook::{self, Hook},
    key,
    write::Write,
};
use crate::{
    call::Auth,
    item::{self, Item},
    reconcile::{self, Event, Need, Outcome, Reply, Stage, Why},
};
use alloc::boxed::Box;
use core::num::NonZeroU64;
use skein_forge::github::{document::Document, request::Request};
use skein_lib::{Queue, Secret};

#[derive(Debug)]
struct Checked {
    key: Secret,
    slug: Box<[u8]>,
    bot: u64,
    lacking: Box<[Lacking]>,
}
#[derive(Debug)]
enum State {
    Begin,
    Pasted,
    Id(Secret),
    User(Secret),
    App { key: Secret, user: Option<NonZeroU64> },
    Bot { key: Secret, slug: Box<[u8]>, lacking: Box<[Lacking]>, user: Option<NonZeroU64> },
    Hook { child: Hook, checked: Checked },
    Writing { write: Write, slug: Box<[u8]>, lacking: Box<[Lacking]> },
    Ended,
}
/// Reads Pasted once, proves both identities before hook repair, and waits for
/// every durable identity write before Done(Lacking) (forge-setup.md, 5.8, 6.4).
#[derive(Debug)]
pub struct Paste {
    given: Given,
    app: NonZeroU64,
    state: State,
}
impl Paste {
    /// Checks the given values before copying them; no callback state is needed (5.8).
    pub fn new(given: &Given, app: NonZeroU64) -> Result<Self, Refused> {
        given::check(given)?;
        Ok(Self { given: given.clone(), app, state: State::Begin })
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
                let write = Write::new(self.app.get(), checked.slug.clone(), checked.key, checked.bot);
                self.writing(write, checked.slug, checked.lacking, Event::Begin)
            }
            Some(hook::Progress::Pending(Need::Done(_))) | None => self.stop(&checked.slug, Why::Unexpected),
        }
    }
    fn writing(&mut self, mut write: Write, slug: Box<[u8]>, lacking: Box<[Lacking]>, event: Event) -> Need {
        match write.advance(event) {
            Ok(Some(need)) => {
                self.state = State::Writing { write, slug, lacking };
                need
            }
            Ok(None) => Need::Done(Outcome::Lacking { lacking }),
            Err(why) => self.stop(&slug, why),
        }
    }
    fn proved(&mut self, key: Secret, user: Option<NonZeroU64>, event: Event) -> Need {
        let subject = app::decimal(self.app.get());
        let app = match reconcile::reply(event) {
            Ok(Reply::GithubDocument(Document::App(app))) => app,
            Ok(Reply::Unsigned) => return self.stop(b"key", Why::KeyForm),
            Ok(
                reply @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)),
            ) => {
                return self.stop(&subject, reconcile::unexpected(reply));
            }
            Err(why) => return self.stop(&subject, why),
        };
        if app.id != self.app.get() {
            return self.stop(&subject, Why::OtherApp);
        }
        if item::app_slug(&app.slug, self.given.limits.name_bytes).is_none() {
            return self.stop(&subject, Why::Unexpected);
        }
        let lacking = checks::lacks(&app);
        let request = Request::BotUser { slug: app.slug.clone() }.into();
        self.state = State::Bot { key, slug: app.slug, lacking, user };
        Need::Send { request, auth: Auth::None }
    }
    fn advance(&mut self, state: State, event: Event) -> Need {
        match state {
            State::Begin => match event {
                Event::Begin => {
                    self.state = State::Pasted;
                    Need::Read(Item::Pasted)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(b"key", Why::Unexpected)
                }
            },
            State::Pasted => match event {
                Event::Read(None) => self.stop(b"key", Why::Missing),
                Event::Read(Some(pem)) => {
                    if pem.is_empty() {
                        return self.stop(b"key", Why::Missing);
                    }
                    let key = key::pem(&pem, self.given.limits.key_bytes);
                    drop(pem);
                    let Some(key) = key else { return self.stop(b"key", Why::KeyForm) };
                    self.state = State::Id(key);
                    Need::Read(Item::AppId)
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(b"key", Why::Unexpected),
            },
            State::Id(key) => match event {
                Event::Read(read) => {
                    match optional_id(read) {
                        Ok(Some(id)) => {
                            if id != self.app {
                                return self.stop(&app::decimal(self.app.get()), Why::OtherApp);
                            }
                        }
                        Ok(None) => (),
                        Err(why) => return self.stop(b"id", why),
                    }
                    self.state = State::User(key);
                    Need::Read(Item::User)
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(b"id", Why::Unexpected),
            },
            State::User(key) => match event {
                Event::Read(read) => {
                    let user = match optional_id(read) {
                        Ok(user) => user,
                        Err(why) => return self.stop(b"user", why),
                    };
                    let auth = Kept::new(self.app, key.clone()).auth();
                    self.state = State::App { key, user };
                    Need::Send { request: Request::App.into(), auth }
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(b"user", Why::Unexpected)
                }
            },
            State::App { key, user } => self.proved(key, user, event),
            State::Bot { key, slug, lacking, user } => match app::bot(event) {
                Ok(bot) => {
                    if let Some(user) = user
                        && user != bot
                    {
                        return self.stop(&slug, Why::OtherUser);
                    }
                    let checked = Checked { key, slug, bot: bot.get(), lacking };
                    let child = Hook::new(
                        self.given.forge,
                        Kept::new(self.app, checked.key.clone()),
                        &checked.slug,
                        !self.given.read_only,
                    );
                    self.hook(child, checked, Event::Begin)
                }
                Err(why) => self.stop(&slug, why),
            },
            State::Hook { child, checked } => self.hook(child, checked, event),
            State::Writing { write, slug, lacking } => self.writing(write, slug, lacking, event),
            State::Ended => unreachable!("ended paste is not advanced"),
        }
    }
}
/// A kept identity is optional, but malformed bytes are Unexpected (forge-setup.md, 5.8).
fn optional_id(read: Option<Secret>) -> Result<Option<NonZeroU64>, Why> {
    match read {
        Some(value) => match item::user(value.expose()) {
            Some(id) => Ok(Some(id)),
            None => Err(Why::Unexpected),
        },
        None => Ok(None),
    }
}
/// Consumes one event and emits its next need, emitting at most one need.
/// Terminal paths release credentials; ended machines are silent (5.8).
pub fn step(machine: &mut Paste, event: Event, out: &mut Queue<Need>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    match state {
        State::Ended => (),
        State::Begin
        | State::Pasted
        | State::Id(_)
        | State::User(_)
        | State::App { .. }
        | State::Bot { .. }
        | State::Hook { .. }
        | State::Writing { .. } => out.push(machine.advance(state, event)),
    }
}
/// Releases every credential and interrupts the current step once (5.8, 6.3).
pub(crate) fn halt(machine: &mut Paste, why: Why, out: &mut Queue<Need>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    let need = match state {
        State::Begin | State::Pasted => machine.stop(b"key", why),
        State::Id(_) => machine.stop(b"id", why),
        State::User(_) => machine.stop(b"user", why),
        State::App { .. } => machine.stop(&app::decimal(machine.app.get()), why),
        State::Bot { slug, .. } | State::Writing { slug, .. } => machine.stop(&slug, why),
        State::Hook { checked, .. } => machine.stop(&checked.slug, why),
        State::Ended => return,
    };
    out.push(need);
}
