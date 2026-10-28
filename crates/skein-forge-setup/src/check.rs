//! A connected Forgejo's tokens must belong to one user (forge-setup.md, 4.6).
use crate::call::Auth;
use crate::{
    item::{self, Item},
    reconcile::{self, Event, Failure, Need, Outcome, Stage, Why},
};
use alloc::boxed::Box;
use skein_forge::forgejo::request::Request;
use skein_lib::{Queue, Secret, bytes};

#[derive(Debug)]
enum State {
    Begin,
    ApiRead,
    ApiUser,
    GitRead,
    GitUser,
    UserRead,
    UserWritten,
    Ended,
}

/// One connected forge's check; tokens move straight from reads into sends.
#[derive(Debug)]
pub struct Check {
    forge: u16,
    state: State,
    user: Option<(u64, Box<[u8]>)>,
}

impl Check {
    /// Starts a check of the tokens given by the person.
    #[must_use]
    pub fn new(forge: u16) -> Self {
        Self { forge, state: State::Begin, user: None }
    }

    fn stop(&mut self, subject: &[u8], why: Why) -> Need {
        self.state = State::Ended;
        self.user = None;
        Need::Stop(Failure { forge: self.forge, step: Stage::Token, subject: bytes::copy_of(subject), why })
    }

    fn done(&mut self) -> Need {
        self.state = State::Ended;
        let (id, login) = self.user.take().expect("API identity checked before kept user");
        Need::Done(Outcome::Identity { id, login })
    }

    fn read_token(&mut self, event: Event, subject: &[u8], next: State) -> Need {
        match event {
            Event::Read(Some(token)) => {
                self.state = next;
                Need::Send { request: Request::User.into(), auth: Auth::Token(token) }
            }
            Event::Read(None) => self.stop(subject, Why::Missing),
            Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(subject, Why::Unexpected),
        }
    }

    fn id(&self) -> u64 {
        self.user.as_ref().expect("API identity checked before git token").0
    }

    fn advance(&mut self, state: State, event: Event) -> Need {
        match state {
            State::Begin => match event {
                Event::Begin => {
                    self.state = State::ApiRead;
                    Need::Read(Item::ApiToken)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(b"api", Why::Unexpected)
                }
            },
            State::ApiRead => self.read_token(event, b"api", State::ApiUser),
            State::ApiUser => match reconcile::account(event) {
                Ok(user) => {
                    if user.is_admin {
                        return self.stop(b"api", Why::BotIsAdmin);
                    }
                    if user.id == 0 {
                        return self.stop(b"api", Why::Unexpected);
                    }
                    self.user = Some((user.id, user.login));
                    self.state = State::GitRead;
                    Need::Read(Item::GitToken)
                }
                Err(why) => self.stop(b"api", why),
            },
            State::GitRead => self.read_token(event, b"git", State::GitUser),
            State::GitUser => match reconcile::account(event) {
                Ok(user) => {
                    if user.id != self.id() {
                        return self.stop(b"git", Why::OtherUser);
                    }
                    self.state = State::UserRead;
                    Need::Read(Item::User)
                }
                Err(why) => self.stop(b"git", why),
            },
            State::UserRead => match event {
                Event::Read(None) => {
                    self.state = State::UserWritten;
                    Need::Write { item: Item::User, bytes: Secret::new(bytes::copy_of(&self.id().to_le_bytes())) }
                }
                Event::Read(Some(value)) => {
                    if match item::user(value.expose()) {
                        Some(id) => id.get() == self.id(),
                        None => false,
                    } {
                        self.done()
                    } else {
                        self.stop(b"api", Why::OtherUser)
                    }
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => self.stop(b"api", Why::Unexpected),
            },
            State::UserWritten => match event {
                Event::Written => self.done(),
                Event::Begin | Event::Read(_) | Event::Reply(_) | Event::Drawn(_) => self.stop(b"api", Why::Unexpected),
            },
            State::Ended => unreachable!("terminal machine is not advanced"),
        }
    }
}

/// Consumes one event and emits at most one need; terminal machines ignore later events.
pub fn step(machine: &mut Check, event: Event, out: &mut Queue<Need>) {
    let event = reconcile::normal(event);
    let state = core::mem::replace(&mut machine.state, State::Ended);
    match state {
        State::Ended => (),
        State::Begin
        | State::ApiRead
        | State::ApiUser
        | State::GitRead
        | State::GitUser
        | State::UserRead
        | State::UserWritten => {
            let need = machine.advance(state, event);
            out.push(need);
        }
    }
}
