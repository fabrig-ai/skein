//! Takes over an owned Forgejo's handed admin token (forge-setup.md, 4.7).
use crate::call::Auth;
use crate::{
    given::{self, Given, Refused},
    item::Item,
    reconcile::{self, Event, Failure, Need, Outcome, Reply, Stage, Why, reply, unexpected},
};
use alloc::boxed::Box;
use skein_forge::forgejo::{
    document::{Document, Row},
    request::{Page, Request},
};
use skein_lib::{Queue, Secret};

/// Room the host reserves before stepping.
pub const MAX_OUT: u32 = 1;

#[derive(Debug)]
enum State {
    Begin,
    Admin,
    Written,
    Settings,
    Listing,
    Delete,
    Ended,
}

/// One takeover; the handed token is held only until the plan ends.
#[derive(Debug)]
pub struct TakeOver {
    forge: u16,
    admin_name: Box<[u8]>,
    login: Box<[u8]>,
    taken: Option<Secret>,
    cap: Option<u32>,
    page: u32,
    deleted: Option<u64>,
    state: State,
}

impl TakeOver {
    /// Checks the owner's given values and holds the handed token for this run.
    pub fn new(given: &Given, admin_token: Secret) -> Result<Self, Refused> {
        given::check(given)?;
        Ok(Self {
            forge: given.forge,
            admin_name: given.admin_name.clone(),
            login: Box::new([]),
            taken: Some(admin_token),
            cap: None,
            page: 1,
            deleted: None,
            state: State::Begin,
        })
    }

    fn token(&self) -> &Secret {
        self.taken.as_ref().expect("live takeover holds its token")
    }

    fn send(&self, request: Request) -> Need {
        Need::Send { request: request.into(), auth: Auth::Token(self.token().clone()) }
    }

    fn stop(&mut self, step: Stage, why: Why) -> Need {
        self.state = State::Ended;
        self.taken = None;
        Need::Stop(Failure { forge: self.forge, step, subject: self.admin_name.clone(), why })
    }

    fn done(&mut self) -> Need {
        self.state = State::Ended;
        self.taken = None;
        Need::Done(Outcome::Nothing)
    }

    fn listing(&mut self) -> Need {
        self.state = State::Listing;
        self.send(Request::Tokens { login: self.login.clone(), page: Page { page: Some(self.page), limit: self.cap } })
    }

    fn advance(&mut self, state: State, event: Event) -> Need {
        match state {
            State::Begin => match event {
                Event::Begin => {
                    if self.token().is_empty() {
                        return self.stop(Stage::Admin, Why::Missing);
                    }
                    self.state = State::Admin;
                    self.send(Request::User)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(Stage::Admin, Why::Unexpected)
                }
            },
            State::Admin => match reconcile::account(event) {
                Ok(user) => {
                    if !user.is_admin {
                        return self.stop(Stage::Admin, Why::NotAdmin);
                    }
                    self.login = user.login;
                    self.state = State::Written;
                    Need::Write { item: Item::AdminToken, bytes: self.token().clone() }
                }
                Err(why) => self.stop(Stage::Admin, why),
            },
            State::Written => match event {
                Event::Written => {
                    self.state = State::Settings;
                    self.send(Request::ApiSettings)
                }
                Event::Begin | Event::Read(_) | Event::Reply(_) | Event::Drawn(_) => {
                    self.stop(Stage::Admin, Why::Unexpected)
                }
            },
            State::Settings | State::Listing | State::Delete => match self.tokens(state, event) {
                Ok(need) => need,
                Err(why) => self.stop(Stage::Token, why),
            },
            State::Ended => unreachable!("terminal machine is not advanced"),
        }
    }

    fn tokens(&mut self, state: State, event: Event) -> Result<Need, Why> {
        match state {
            State::Settings => match reply(event)? {
                Reply::Document(Document::ApiSettings(settings)) if settings.max_response_items > 0 => {
                    self.cap = Some(settings.max_response_items);
                    Ok(self.listing())
                }
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            State::Listing => self.rows(reply(event)?),
            State::Delete => match reply(event)? {
                Reply::Empty | Reply::Status(404) => Ok(self.listing()),
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            State::Begin | State::Admin | State::Written | State::Ended => unreachable!("token phases only"),
        }
    }

    fn rows(&mut self, answer: Reply) -> Result<Need, Why> {
        let rows = match answer {
            Reply::Rows(rows) => rows,
            other @ (Reply::Document(_)
            | Reply::Empty
            | Reply::Status(_)
            | Reply::Down
            | Reply::GithubDocument(_)
            | Reply::GithubRows(_)
            | Reply::Unsigned) => {
                return Err(unexpected(other));
            }
        };
        for row in &rows {
            match row {
                Row::Token(token) => {
                    let secret = self.token();
                    let suffix = secret.expose().get(secret.len().saturating_sub(8)..).expect("token suffix");
                    if token.last_eight.as_ref() != suffix {
                        if self.deleted == Some(token.id) {
                            return Err(Why::Unexpected);
                        }
                        self.deleted = Some(token.id);
                        self.state = State::Delete;
                        return Ok(self.send(Request::DeleteToken { login: self.login.clone(), id: token.id }));
                    }
                }
                Row::Reachable(_)
                | Row::Found(_)
                | Row::Hook(_)
                | Row::Team(_)
                | Row::User(_)
                | Row::Issue(_)
                | Row::Comment(_)
                | Row::Status(_)
                | Row::Review(_)
                | Row::ReviewComment(_)
                | Row::Branch(_)
                | Row::Protection(_)
                | Row::Pull(_)
                | Row::File(_)
                | Row::Commit(_)
                | Row::CommitMessage(_)
                | Row::Entry(_)
                | Row::Run(_)
                | Row::Job(_)
                | Row::Step(_) => return Err(Why::Unexpected),
            }
        }
        if rows.len() < usize::try_from(self.cap.expect("cap read before listing")).expect("u32 fits") {
            Ok(self.done())
        } else {
            self.page = self.page.checked_add(1).ok_or(Why::Unexpected)?;
            Ok(self.listing())
        }
    }
}

/// Consumes one event and emits at most one need; terminal machines ignore later events.
pub fn step(machine: &mut TakeOver, event: Event, out: &mut Queue<Need>) {
    let state = core::mem::replace(&mut machine.state, State::Ended);
    match state {
        State::Ended => (),
        State::Begin | State::Admin | State::Written | State::Settings | State::Listing | State::Delete => {
            let need = machine.advance(state, event);
            out.push(need);
        }
    }
}
