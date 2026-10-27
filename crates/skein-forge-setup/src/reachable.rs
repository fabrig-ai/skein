//! One page of a forge's repositories for the picker (forge-setup.md, 4.8).
use crate::call::Auth;
use crate::github::{self, app};
use crate::{
    given::{self, Given, Place, Refused},
    item::Item,
    reconcile::{self, Event, Failure, Need, Outcome, Reply, Stage, Why, reply, unexpected},
};
use alloc::boxed::Box;
use core::num::NonZeroU64;
use skein_forge::forgejo::{
    document::{Document, Row},
    request::{Page, Request},
};
use skein_forge::github::{document as github_document, request as github_request};
use skein_lib::{Decimal, List, Queue, Secret, bytes};

/// Room the host reserves before stepping.
pub const MAX_OUT: u32 = 1;

/// One repository of a picker's page (forge-setup.md, 4.8, step 4).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reached {
    pub id: u64,
    pub owner_id: u64,
    /// The owner's login.
    pub owner: Box<[u8]>,
    pub name: Box<[u8]>,
    pub private: bool,
}

#[derive(Debug)]
enum State {
    Begin,
    Read,
    ApiRead,
    Admin,
    Settings,
    Search,
    GithubBegin { installations: u32 },
    GithubAccepted { installations: u32 },
    GithubId,
    GithubKey(NonZeroU64),
    GithubMint(app::Kept),
    GithubPage,
    Ended,
}

/// Whose token reads the page (forge-setup.md, 4.8, step 1).
#[derive(Debug)]
enum Token {
    Admin,
    Api,
}

/// One picker page; credentials live only as long as their phases need them.
#[derive(Debug)]
pub struct Reachable {
    forge: u16,
    admin_name: Box<[u8]>,
    installation: Option<u64>,
    page: u32,
    limit: u32,
    state: State,
    token: Option<Secret>,
    listing: Token,
}

impl Reachable {
    /// Refuses invalid given values or a page of zero; pages start at one.
    pub fn new(given: &Given, installation: Option<u64>, page: u32) -> Result<Self, Refused> {
        given::check(given)?;
        if page == 0 {
            return Err(Refused { place: Place::Page, other: None });
        }
        Ok(Self {
            forge: given.forge,
            admin_name: given.admin_name.clone(),
            installation,
            page,
            limit: given.limits.reachable,
            state: State::Begin,
            token: None,
            listing: Token::Admin,
        })
    }

    /// GitHub's picker for an accepted installation; pages start at one.
    pub fn github(given: &github::given::Given, installation: u64, page: u32) -> Result<Self, github::given::Refused> {
        github::given::check(given)?;
        if page == 0 {
            return Err(github::given::Refused { place: github::given::Place::Page, other: None });
        }
        Ok(Self {
            forge: given.forge,
            admin_name: Box::new([]),
            installation: Some(installation),
            page,
            limit: core::cmp::min(given.limits.reachable, skein_forge::github::pin::PAGE_CAP),
            state: State::GithubBegin { installations: given.limits.installations },
            token: None,
            listing: Token::Admin,
        })
    }

    fn send(&self, request: Request) -> Need {
        Need::Send {
            request: request.into(),
            auth: Auth::Token(self.token.clone().expect("listing token read before requests")),
        }
    }

    fn stop(&mut self, step: Stage, subject: Box<[u8]>, why: Why) -> Need {
        self.state = State::Ended;
        self.token = None;
        Need::Stop(Failure { forge: self.forge, step, subject, why })
    }

    fn advance(&mut self, state: State, event: Event) -> Need {
        match state {
            State::Begin => match event {
                Event::Begin => {
                    if let Some(id) = self.installation {
                        let subject = bytes::copy_of(Decimal::of(id).as_bytes());
                        return self.stop(Stage::Installation, subject, Why::Unexpected);
                    }
                    self.state = State::Read;
                    Need::Read(Item::AdminToken)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(Stage::Admin, self.admin_name.clone(), Why::Unexpected)
                }
            },
            State::Read => match event {
                Event::Read(Some(token)) => {
                    self.token = Some(token);
                    self.state = State::Admin;
                    self.send(Request::User)
                }
                Event::Read(None) => {
                    self.listing = Token::Api;
                    self.state = State::ApiRead;
                    Need::Read(Item::ApiToken)
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(Stage::Admin, self.admin_name.clone(), Why::Unexpected)
                }
            },
            State::ApiRead => match event {
                Event::Read(Some(token)) => {
                    self.token = Some(token);
                    self.state = State::Settings;
                    self.send(Request::ApiSettings)
                }
                Event::Read(None) => self.stop(Stage::Token, bytes::copy_of(b"api"), Why::Missing),
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.stop(Stage::Token, bytes::copy_of(b"api"), Why::Unexpected)
                }
            },
            State::Admin => match reconcile::account(event) {
                Ok(user) => {
                    if !user.is_admin {
                        return self.stop(Stage::Admin, self.admin_name.clone(), Why::NotAdmin);
                    }
                    self.state = State::Settings;
                    self.send(Request::ApiSettings)
                }
                Err(why) => self.stop(Stage::Admin, self.admin_name.clone(), why),
            },
            State::Settings | State::Search => match self.repositories(state, event) {
                Ok(need) => need,
                Err(why) => {
                    let subject = match self.listing {
                        Token::Admin => self.admin_name.clone(),
                        Token::Api => bytes::copy_of(b"api"),
                    };
                    self.stop(Stage::Token, subject, why)
                }
            },
            State::GithubBegin { .. }
            | State::GithubAccepted { .. }
            | State::GithubId
            | State::GithubKey(_)
            | State::GithubMint(_)
            | State::GithubPage => self.github_advance(state, event),
            State::Ended => unreachable!("terminal machine is not advanced"),
        }
    }

    fn github_advance(&mut self, state: State, event: Event) -> Need {
        match state {
            State::GithubBegin { installations } => match event {
                Event::Begin => {
                    self.state = State::GithubAccepted { installations };
                    Need::Read(Item::Accepted)
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.github_installation_stop(Why::Unexpected)
                }
            },
            State::GithubAccepted { installations } => match event {
                Event::Read(read) => {
                    let accepted = match read {
                        Some(value) => crate::item::accepted(value.expose(), installations),
                        None => None,
                    };
                    let installation = self.installation.expect("GitHub picker names its installation");
                    let found = match accepted {
                        Some(ids) => ids.contains(&installation),
                        None => false,
                    };
                    if !found {
                        return self.github_installation_stop(Why::Missing);
                    }
                    self.state = State::GithubId;
                    Need::Read(Item::AppId)
                }
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.github_installation_stop(Why::Unexpected)
                }
            },
            State::GithubId => match event {
                Event::Read(read) => match app::id_of(read) {
                    Some(id) => {
                        self.state = State::GithubKey(id);
                        Need::Read(Item::AppKey)
                    }
                    None => self.github_stop(Stage::App, b"id", Why::Missing),
                },
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.github_stop(Stage::App, b"id", Why::Unexpected)
                }
            },
            State::GithubKey(id) => match event {
                Event::Read(Some(key)) => {
                    let app = app::Kept::new(id, key);
                    let auth = app.auth();
                    self.state = State::GithubMint(app);
                    Need::Send {
                        request: github_request::Request::Mint {
                            installation: self.installation.expect("GitHub picker names its installation"),
                            scope: github_request::Scope::Full,
                        }
                        .into(),
                        auth,
                    }
                }
                Event::Read(None) => self.github_stop(Stage::App, b"key", Why::Missing),
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                    self.github_stop(Stage::App, b"key", Why::Unexpected)
                }
            },
            State::GithubMint(app) => {
                drop(app);
                self.github_mint(event)
            }
            State::GithubPage => self.github_page(event),
            State::Begin
            | State::Read
            | State::ApiRead
            | State::Admin
            | State::Settings
            | State::Search
            | State::Ended => {
                unreachable!("GitHub phases only")
            }
        }
    }

    fn github_mint(&mut self, event: Event) -> Need {
        match reply(event) {
            Ok(Reply::GithubDocument(github_document::Document::Minted(minted))) => {
                self.state = State::GithubPage;
                Need::Send {
                    request: github_request::Request::Reachable { page: self.page, per_page: self.limit }.into(),
                    auth: Auth::Bearer(minted.token),
                }
            }
            Ok(Reply::Unsigned) => self.github_stop(Stage::App, b"key", Why::KeyForm),
            Ok(Reply::Status(404)) => self.github_installation_stop(Why::Missing),
            Ok(
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)),
            ) => self.github_installation_stop(unexpected(other)),
            Err(why) => self.github_installation_stop(why),
        }
    }

    fn github_page(&mut self, event: Event) -> Need {
        match reply(event) {
            Ok(Reply::GithubRows(rows)) => match self.github_rows(rows) {
                Ok(need) => need,
                Err(why) => self.github_installation_stop(why),
            },
            Ok(Reply::Unsigned) => self.github_installation_stop(Why::Unexpected),
            Ok(
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)),
            ) => self.github_installation_stop(unexpected(other)),
            Err(why) => self.github_installation_stop(why),
        }
    }

    fn github_stop(&mut self, stage: Stage, subject: &[u8], why: Why) -> Need {
        self.state = State::Ended;
        self.token = None;
        app::stop(self.forge, stage, subject, why)
    }

    fn github_installation_stop(&mut self, why: Why) -> Need {
        let subject = app::decimal(self.installation.expect("GitHub picker names its installation"));
        self.github_stop(Stage::Installation, &subject, why)
    }

    fn github_rows(&mut self, rows: Box<[github_document::Row]>) -> Result<Need, Why> {
        let limit = usize::try_from(self.limit).expect("u32 fits");
        if rows.len() > limit {
            return Err(Why::Unexpected);
        }
        let more = rows.len() == limit;
        let mut reached = List::with_capacity(u32::try_from(rows.len()).expect("bounded page"));
        for row in rows {
            let row = match row {
                github_document::Row::Reachable(row) => row,
                github_document::Row::Log(_)
                | github_document::Row::Installation(_)
                | github_document::Row::Branch(_)
                | github_document::Row::SkippedBranch
                | github_document::Row::Issue(_)
                | github_document::Row::Comment(_)
                | github_document::Row::Pull(_)
                | github_document::Row::Context(_)
                | github_document::Row::Run(_)
                | github_document::Row::Collaborator(_)
                | github_document::Row::Remark(_)
                | github_document::Row::Review(_)
                | github_document::Row::Commit(_)
                | github_document::Row::Rule(_)
                | github_document::Row::File(_) => return Err(Why::Unexpected),
            };
            reached
                .push(Reached {
                    id: row.id,
                    owner_id: row.owner,
                    owner: row.login,
                    name: row.name,
                    private: row.private,
                })
                .expect("bounded page");
        }
        self.state = State::Ended;
        Ok(Need::Done(Outcome::Reachable { rows: reached.into_boxed(), more }))
    }

    fn repositories(&mut self, state: State, event: Event) -> Result<Need, Why> {
        match state {
            State::Settings => match reply(event)? {
                Reply::Document(Document::ApiSettings(settings)) if settings.max_response_items > 0 => {
                    self.limit = core::cmp::min(self.limit, settings.max_response_items);
                    self.state = State::Search;
                    let page = Page { page: Some(self.page), limit: Some(self.limit) };
                    Ok(self.send(match self.listing {
                        Token::Admin => Request::RepoSearch { page },
                        Token::Api => Request::UserRepos { page },
                    }))
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
            State::Search => self.rows(reply(event)?),
            State::Begin
            | State::Read
            | State::ApiRead
            | State::Admin
            | State::GithubBegin { .. }
            | State::GithubAccepted { .. }
            | State::GithubId
            | State::GithubKey(_)
            | State::GithubMint(_)
            | State::GithubPage
            | State::Ended => {
                unreachable!("repository phases only")
            }
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
        let limit = usize::try_from(self.limit).expect("u32 fits");
        if rows.len() > limit {
            return Err(Why::Unexpected);
        }
        let more = rows.len() == limit;
        let mut reached = List::with_capacity(u32::try_from(rows.len()).expect("bounded page"));
        for row in rows {
            let row = match (row, &self.listing) {
                (Row::Found(row), Token::Admin) | (Row::Reachable(row), Token::Api) => row,
                (Row::Found(_), Token::Api)
                | (Row::Reachable(_), Token::Admin)
                | (
                    Row::Hook(_)
                    | Row::Token(_)
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
                    | Row::Step(_),
                    Token::Admin | Token::Api,
                ) => return Err(Why::Unexpected),
            };
            if match self.listing {
                Token::Admin => true,
                Token::Api => row.push,
            } {
                reached
                    .push(Reached {
                        id: row.repository,
                        owner_id: row.owner,
                        owner: row.login,
                        name: row.name,
                        private: row.private,
                    })
                    .expect("bounded page");
            }
        }
        self.state = State::Ended;
        self.token = None;
        Ok(Need::Done(Outcome::Reachable { rows: reached.into_boxed(), more }))
    }
}

/// Consumes one event and emits at most one need; terminal machines ignore later events.
pub fn step(machine: &mut Reachable, event: Event, out: &mut Queue<Need>) {
    let event = reconcile::normal(event);
    let state = core::mem::replace(&mut machine.state, State::Ended);
    match state {
        State::Ended => (),
        State::Begin
        | State::Read
        | State::ApiRead
        | State::Admin
        | State::Settings
        | State::Search
        | State::GithubBegin { .. }
        | State::GithubAccepted { .. }
        | State::GithubId
        | State::GithubKey(_)
        | State::GithubMint(_)
        | State::GithubPage => {
            let need = machine.advance(state, event);
            out.push(need);
        }
    }
}
