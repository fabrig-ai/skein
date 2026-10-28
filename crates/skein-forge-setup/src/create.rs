//! Makes a first repository by filling only what is absent (forge-setup.md, 4.9).
use crate::{
    access::{Access, Fail, Move, Shared},
    call::Auth,
    given::{self, Given, Refused},
    host::File,
    item::Item,
    reconcile::{self, Event, Failure, Need, Outcome, Reply, Stage, Why, reply, unexpected},
};
use alloc::boxed::Box;
use core::num::NonZeroU64;
use skein_forge::forgejo::{
    document::Document,
    request::{Author, Repo, Request},
};
use skein_lib::{Queue, Secret, Writer};

#[derive(Debug)]
enum State {
    Begin,
    AdminFile,
    Admin,
    Org,
    MakeOrg,
    Repository,
    MakeRepository,
    Access,
    Branch,
    Raw,
    File,
    Ended,
}
/// One creation, holding its administrator only until the run ends.
#[derive(Debug)]
pub struct Create {
    forge: u16,
    admin_name: Box<[u8]>,
    admin: Option<Secret>,
    admin_login: Box<[u8]>,
    owner: Option<Box<[u8]>>,
    at: Repo,
    subject: Box<[u8]>,
    author: Author,
    files: Box<[File]>,
    next: usize,
    id: Option<NonZeroU64>,
    branch: Box<[u8]>,
    access: Access,
    shared: Shared,
    state: State,
}
impl Create {
    /// The host has checked the plan against its admitted bounds before constructing it.
    pub fn new(given: &Given, org: Box<[u8]>, repository: Box<[u8]>, files: Box<[File]>) -> Result<Self, Refused> {
        given::check(given)?;
        given::create_fields(&given.limits, u32::MAX, &org, &repository, &files)?;
        let mut subject = Writer::new(
            org.len().checked_add(repository.len()).expect("checked names").checked_add(1).expect("subject slash"),
        );
        subject.put(&org).expect("org capacity");
        subject.put(b"/").expect("slash capacity");
        subject.put(&repository).expect("name capacity");
        Ok(Self {
            forge: given.forge,
            admin_name: given.admin_name.clone(),
            admin: None,
            admin_login: Box::new([]),
            owner: given.org_owner.clone(),
            at: Repo { owner: org, name: repository },
            subject: subject.finish(),
            author: Author { name: given.bot_login.clone(), email: given.bot_email.clone() },
            files,
            next: 0,
            id: None,
            branch: Box::new([]),
            access: Access::new(
                0,
                &given.bot_login,
                match given.listener {
                    Some(address) => reconcile::hook_url(address, given.forge),
                    None => None,
                },
            ),
            shared: Shared { cap: None, secret: None },
            state: State::Begin,
        })
    }
    fn stage(&self) -> Stage {
        match self.state {
            State::Begin | State::AdminFile | State::Admin => Stage::Admin,
            State::Org | State::MakeOrg => Stage::Org,
            State::Repository | State::MakeRepository | State::Access => Stage::Repository,
            State::Branch | State::Raw | State::File => Stage::File,
            State::Ended => unreachable!("ended creation has no outstanding need"),
        }
    }
    fn stop(&mut self, why: Why) -> Need {
        let step = self.stage();
        let subject = match step {
            Stage::Admin => self.admin_name.clone(),
            Stage::Org => self.at.owner.clone(),
            Stage::Repository => self.subject.clone(),
            Stage::File => self.file().path.clone(),
            Stage::Bot | Stage::Token | Stage::App | Stage::Installation | Stage::State => {
                unreachable!("creation stages")
            }
        };
        self.state = State::Ended;
        self.admin = None;
        self.shared.secret = None;
        Need::Stop(Failure { forge: self.forge, step, subject, why })
    }
    fn send(&self, request: Request) -> Need {
        Need::Send { request: request.into(), auth: Auth::Token(self.admin.clone().expect("admin before writes")) }
    }
    fn repository(&mut self) -> Need {
        self.state = State::Repository;
        self.send(Request::Repository { at: self.at.clone() })
    }
    fn access(&mut self, id: u64, branch: Box<[u8]>) -> Result<Need, Why> {
        self.id = Some(NonZeroU64::new(id).ok_or(Why::Unexpected)?);
        if branch.is_empty() {
            return Err(Why::Unexpected);
        }
        self.branch = branch;
        self.state = State::Access;
        let request = self.access.known(self.at.clone());
        Ok(self.send(request))
    }
    fn file(&self) -> &File {
        self.files.get(self.next).expect("current file")
    }
    fn next_file(&mut self) -> Need {
        if self.next == self.files.len() {
            self.state = State::Ended;
            self.admin = None;
            self.shared.secret = None;
            Need::Done(Outcome::Created { id: self.id.expect("repository read or made") })
        } else {
            self.state = State::Branch;
            self.send(Request::Branch { at: self.at.clone(), name: self.branch.clone() })
        }
    }
    fn file_done(&mut self) -> Need {
        self.next = self.next.checked_add(1).expect("bounded files");
        self.next_file()
    }
    fn put_file(&mut self) -> Need {
        let file = self.file();
        let mut message = Writer::new(file.path.len().checked_add(4).expect("bounded path"));
        message.put(b"Add ").expect("message prefix");
        message.put(&file.path).expect("message path");
        let request = Request::CreateFile {
            at: self.at.clone(),
            path: file.path.clone(),
            branch: self.branch.clone(),
            content: file.content.clone(),
            message: message.finish(),
            author: self.author.clone(),
        };
        self.state = State::File;
        self.send(request)
    }
    fn advance(&mut self, event: Event) -> Result<Need, Why> {
        match self.state {
            State::Begin => match event {
                Event::Begin => {
                    self.state = State::AdminFile;
                    Ok(Need::Read(Item::AdminToken))
                }
                Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => Err(Why::Unexpected),
            },
            State::AdminFile => match event {
                Event::Read(Some(token)) => {
                    self.admin = Some(token);
                    self.state = State::Admin;
                    Ok(self.send(Request::User))
                }
                Event::Read(None) => Err(Why::Missing),
                Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => Err(Why::Unexpected),
            },
            State::Admin => {
                let admin = reconcile::account(event)?;
                if !admin.is_admin {
                    return Err(Why::NotAdmin);
                }
                self.admin_login = admin.login;
                self.state = State::Org;
                Ok(self.send(Request::Org { name: self.at.owner.clone() }))
            }
            State::Access => match self.access.event(&mut self.shared, event) {
                Ok(Move::Send(request)) => Ok(self.send(request)),
                Ok(Move::Read(item)) => Ok(Need::Read(item)),
                Ok(Move::Write { item, bytes }) => Ok(Need::Write { item, bytes }),
                Ok(Move::Draw(count)) => Ok(Need::Draw(count)),
                Ok(Move::Done) => Ok(self.next_file()),
                Err(Fail::Gone) => Err(Why::Missing),
                Err(Fail::Why(why)) => Err(why),
            },
            State::Org
            | State::MakeOrg
            | State::Repository
            | State::MakeRepository
            | State::Branch
            | State::Raw
            | State::File => self.answer(reply(event)?),
            State::Ended => unreachable!("terminal creation ignores events"),
        }
    }
    fn answer(&mut self, answer: Reply) -> Result<Need, Why> {
        match self.state {
            State::Org => match answer {
                Reply::Document(Document::Org(_)) => Ok(self.repository()),
                Reply::Status(404) => {
                    let owner = match &self.owner {
                        Some(owner) => owner.clone(),
                        None => self.admin_login.clone(),
                    };
                    self.state = State::MakeOrg;
                    Ok(self.send(Request::CreateOrg { person: owner, name: self.at.owner.clone() }))
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
            State::MakeOrg => match answer {
                Reply::Document(Document::Org(_)) => Ok(self.repository()),
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            State::Repository => match answer {
                Reply::Document(Document::Repository(repository)) => {
                    self.access(repository.id, repository.default_branch)
                }
                Reply::Status(404) => {
                    self.state = State::MakeRepository;
                    Ok(self.send(Request::CreateRepository {
                        org: self.at.owner.clone(),
                        name: self.at.name.clone(),
                        private: true,
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
            State::MakeRepository => match answer {
                Reply::Document(Document::CreatedRepository(repository)) => {
                    self.access(repository.id, repository.default_branch)
                }
                Reply::Status(409) => Ok(self.repository()),
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            State::Branch | State::Raw | State::File => self.file_answer(answer),
            State::Begin | State::AdminFile | State::Admin | State::Access | State::Ended => {
                unreachable!("creation reply phases")
            }
        }
    }
    fn file_answer(&mut self, answer: Reply) -> Result<Need, Why> {
        match self.state {
            State::Branch => match answer {
                Reply::Document(Document::Branch(branch)) => {
                    self.state = State::Raw;
                    Ok(self.send(Request::Raw {
                        at: self.at.clone(),
                        path: self.file().path.clone(),
                        reference: branch.commit,
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
            State::Raw => match answer {
                Reply::Empty => Ok(self.file_done()),
                Reply::Status(404) => Ok(self.put_file()),
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            State::File => match answer {
                Reply::Document(Document::CreatedFile { .. }) => Ok(self.file_done()),
                other @ (Reply::Document(_)
                | Reply::Rows(_)
                | Reply::Empty
                | Reply::Status(_)
                | Reply::Down
                | Reply::GithubDocument(_)
                | Reply::GithubRows(_)
                | Reply::Unsigned) => Err(unexpected(other)),
            },
            State::Begin
            | State::AdminFile
            | State::Admin
            | State::Org
            | State::MakeOrg
            | State::Repository
            | State::MakeRepository
            | State::Access
            | State::Ended => unreachable!("file reply phases"),
        }
    }
}
/// One event in, at most one need out; an ended run ignores later events.
pub fn step(machine: &mut Create, event: Event, out: &mut Queue<Need>) {
    if let State::Ended = machine.state {
        return;
    }
    let need = match machine.advance(reconcile::normal(event)) {
        Ok(need) => need,
        Err(why) => machine.stop(why),
    };
    out.push(need);
}
/// Stops at the outstanding stage, releasing every held credential.
pub fn halt(machine: &mut Create, why: Why, out: &mut Queue<Need>) {
    if let State::Ended = machine.state {
        return;
    }
    out.push(machine.stop(why));
}
