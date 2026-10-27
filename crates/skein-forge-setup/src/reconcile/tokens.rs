//! The two token files are checked before the bot's id (forge-setup.md, 4.3).
use super::{Event, Need, Reconcile, Reply, State, Why};
use crate::call::Auth;
use crate::item::Item;
use alloc::boxed::Box;
use skein_forge::forgejo::{
    document::{Document, Row},
    request::{Page, Request},
};
use skein_lib::bytes;
#[derive(Debug)]
enum Phase {
    Read,
    Check,
    Settings,
    Listing,
    Delete,
    Create,
    Written,
}
#[derive(Debug)]
pub(super) struct Tokens {
    id: u64,
    api_name: Box<[u8]>,
    git_name: Box<[u8]>,
    item: Item,
    cap: Option<u32>,
    page: u32,
    phase: Phase,
}
impl Tokens {
    pub fn new(id: u64, api_name: &[u8], git_name: &[u8]) -> Self {
        Self {
            id,
            api_name: api_name.into(),
            git_name: git_name.into(),
            item: Item::ApiToken,
            cap: None,
            page: 1,
            phase: Phase::Read,
        }
    }
    pub fn cap(&self) -> Option<u32> {
        self.cap
    }
    pub fn name(&self) -> &[u8] {
        match self.item {
            Item::ApiToken => &self.api_name,
            Item::GitToken => &self.git_name,
            Item::AdminToken
            | Item::User
            | Item::WebhookSecret
            | Item::AppId
            | Item::AppSlug
            | Item::AppKey
            | Item::Accepted
            | Item::Pasted => unreachable!("two token items"),
        }
    }
    fn listing(&mut self, machine: &Reconcile) -> Need {
        self.phase = Phase::Listing;
        machine.send(Request::Tokens {
            login: machine.bot_login.clone(),
            page: Page { page: Some(self.page), limit: self.cap },
        })
    }
    fn remake(&mut self, machine: &Reconcile) -> Need {
        self.page = 1;
        if self.cap.is_some() {
            self.listing(machine)
        } else {
            self.phase = Phase::Settings;
            machine.send(Request::ApiSettings)
        }
    }
    fn create(&mut self, machine: &Reconcile) -> Need {
        self.phase = Phase::Create;
        let scopes: Box<[Box<[u8]>]> = if self.item == Item::ApiToken {
            Box::new([
                bytes::copy_of(b"read:user"),
                bytes::copy_of(b"write:issue"),
                bytes::copy_of(b"write:repository"),
            ])
        } else {
            Box::new([bytes::copy_of(b"write:repository"), bytes::copy_of(b"read:user")])
        };
        machine.send(Request::CreateToken {
            login: machine.bot_login.clone(),
            name: bytes::copy_of(self.name()),
            scopes,
        })
    }
    fn next(&mut self, machine: &mut Reconcile) -> Need {
        if self.item == Item::ApiToken {
            self.item = Item::GitToken;
            self.phase = Phase::Read;
            Need::Read(self.item)
        } else {
            machine.shared.cap = self.cap();
            machine.state = State::Id(self.id);
            Need::Read(Item::User)
        }
    }
    fn event(&mut self, machine: &mut Reconcile, event: Event) -> Result<Need, Why> {
        match self.phase {
            Phase::Read => match event {
                Event::Read(Some(secret)) => {
                    self.phase = Phase::Check;
                    Ok(Need::Send { request: Request::User.into(), auth: Auth::Token(secret) })
                }
                Event::Read(None) => Ok(self.remake(machine)),
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => Err(Why::Unexpected),
            },
            Phase::Check => match super::account(event) {
                Ok(user) => Ok(if user.id == self.id { self.next(machine) } else { self.remake(machine) }),
                Err(Why::Refused(401 | 403)) => Ok(self.remake(machine)),
                Err(why) => Err(why),
            },
            Phase::Settings => match reply(event)? {
                Reply::Document(Document::ApiSettings(settings)) if settings.max_response_items > 0 => {
                    self.cap = Some(settings.max_response_items);
                    Ok(self.listing(machine))
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
            Phase::Listing => self.rows(machine, reply(event)?),
            Phase::Delete => match reply(event)? {
                Reply::Empty | Reply::Status(404) => Ok(self.create(machine)),
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            Phase::Create => match reply(event)? {
                Reply::Document(Document::Token(token)) => {
                    let value = token.value.ok_or(Why::Unexpected)?;
                    self.phase = Phase::Written;
                    Ok(Need::Write { item: self.item, bytes: value })
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
            Phase::Written => match event {
                Event::Written => Ok(self.next(machine)),
                Event::Begin | Event::Read(_) | Event::Reply(_) | Event::Drawn(_) => Err(Why::Unexpected),
            },
        }
    }
    fn rows(&mut self, machine: &Reconcile, reply: Reply) -> Result<Need, Why> {
        let rows = match reply {
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
                    if token.name.as_ref() == self.name() {
                        self.phase = Phase::Delete;
                        return Ok(
                            machine.send(Request::DeleteToken { login: machine.bot_login.clone(), id: token.id })
                        );
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
                | Row::Entry(_)
                | Row::Run(_)
                | Row::Job(_)
                | Row::Step(_) => return Err(Why::Unexpected),
            }
        }
        if rows.len() < usize::try_from(self.cap.expect("cap read before listing")).expect("u32 fits") {
            Ok(self.create(machine))
        } else {
            self.page = self.page.checked_add(1).ok_or(Why::Unexpected)?;
            Ok(self.listing(machine))
        }
    }
}
pub(crate) fn reply(event: Event) -> Result<Reply, Why> {
    match event {
        Event::Reply(reply) => Ok(reply),
        Event::Begin | Event::Read(_) | Event::Written | Event::Drawn(_) => Err(Why::Unexpected),
    }
}
pub(crate) fn unexpected(reply: Reply) -> Why {
    match reply {
        Reply::Status(status) => Why::Refused(status),
        Reply::Down => Why::Down,
        Reply::Unsigned => Why::KeyForm,
        Reply::Document(_) | Reply::Rows(_) | Reply::Empty | Reply::GithubDocument(_) | Reply::GithubRows(_) => {
            Why::Unexpected
        }
    }
}
pub(super) fn advance(machine: &mut Reconcile, event: Event) -> Need {
    let mut tokens = machine.tokens.take().expect("token phase owns its walk");
    let need = match tokens.event(machine, event) {
        Ok(need) => need,
        Err(why) => {
            let mut failure = machine.stop(super::Stage::Token, why);
            match &mut failure {
                Need::Stop(f) => f.subject = bytes::copy_of(tokens.name()),
                Need::Send { .. } | Need::Read(_) | Need::Write { .. } | Need::Draw(_) | Need::Done(_) => {
                    unreachable!("stop")
                }
            }
            failure
        }
    };
    machine.tokens = Some(tokens);
    need
}
