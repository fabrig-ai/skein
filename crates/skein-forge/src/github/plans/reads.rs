//! The reads whose GitHub requests and documents are complete (forge-github.md, 4.1).
use super::{
    Body, Heard, failure,
    plan::{Bounds, Exchange, Step},
    tables::{self, Repository, Tables},
};
use crate::{
    api,
    github::{document, header::Head, request::Request},
};
use alloc::boxed::Box;
use skein_lib::List;

mod checks;
mod done;
mod files;
mod issues;
mod job;
mod people;
mod protection;
mod pulls;
mod reviews;
mod size;

/// The reads planned by the GitHub dialect.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Read {
    Protection { branch: Box<[u8]> },
    Locate { owner: Box<[u8]>, name: Box<[u8]> },
    Settings,
    Branch { branch: Box<[u8]> },
    Branches { prefix: Box<[u8]> },
    Items { since: skein_lib::Time, page: u32, kind: Option<api::Kind> },
    Item { number: u64, after: u64 },
    Pull { number: u64 },
    PullFor { head: Box<[u8]>, base: Box<[u8]> },
    Statuses { commit: api::Commit, page: u32 },
    Checks { commit: api::Commit },
    Permission { user: u64 },
    Collaborators { page: u32 },
    Remarks { number: u64, review: u64, page: u32 },
    Reviews { number: u64, page: u32 },
    File { head: api::Commit, path: Box<[u8]>, max_bytes: u32 },
    Compare { before: api::Commit, after: api::Commit },
    PullFiles { number: u64, head: api::Commit, page: u32 },
    Job { attempt: api::JobAttempt, max_bytes: u32 },
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Stage {
    Rules,
    Ruleset,
    Classic,
    New,
    Resolve,
    Read,
    Comments,
    Listing,
    Combined,
    Runs,
    PullGraphql,
    Jobs,
    Job,
    JobLog,
    Log,
    ResolveUser,
    Permission,
    Raw,
    Compare,
    FilePull,
    MergeBase,
    FileListing,
    Before,
    After,
    Done,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Response {
    None,
    Head,
    Body { next: bool },
    Refusal { status: u16, head: Head },
}
#[derive(Debug)]
pub struct Plan {
    repository: api::Repository,
    read: Read,
    stage: Stage,
    response: Response,
    row: Option<Repository>,
    request: Option<Exchange>,
    rows: List<Box<[u8]>>,
    size: u64,
    count: u32,
    page: u32,
    limit: u32,
    issues: issues::State,
    pulls: pulls::State,
    checks: checks::State,
    login: Option<Box<[u8]>>,
    collaborators: List<api::Collaborator>,
    remarks: List<api::Remark>,
    reviews: List<api::Review>,
    last_review: u64,
    raw_cap: u32,
    job_step: Option<api::Step>,
    files: files::State,
    protection: protection::State,
}
impl Plan {
    #[must_use]
    pub fn new(repository: api::Repository, read: api::Read) -> Self {
        let read = match read {
            api::Read::Runs { .. }
            | api::Read::Run { .. }
            | api::Read::Tree { .. }
            | api::Read::Artifact { .. }
            | api::Read::Commits { .. } => {
                unreachable!("turn calls refused before read plan")
            }
            api::Read::Locate { owner, name } => Read::Locate { owner, name },
            api::Read::Settings => Read::Settings,
            api::Read::Branch { branch } => Read::Branch { branch },
            api::Read::Branches { prefix } => Read::Branches { prefix },
            api::Read::Items { since, page, kind } => Read::Items { since, page, kind },
            api::Read::Item { number, after } => Read::Item { number, after },
            api::Read::Pull { number } => Read::Pull { number },
            api::Read::PullFor { head, base } => Read::PullFor { head, base },
            api::Read::Statuses { commit, page } => Read::Statuses { commit, page },
            api::Read::Compare { before, after } => Read::Compare { before, after },
            api::Read::File { head, path, max_bytes } => Read::File { head, path, max_bytes },
            api::Read::Reviews { number, page } => Read::Reviews { number, page },
            api::Read::Remarks { number, review, page } => Read::Remarks { number, review, page },
            api::Read::Collaborators { page } => Read::Collaborators { page },
            api::Read::Permission { user } => Read::Permission { user },
            api::Read::PullFiles { number, head, page } => Read::PullFiles { number, head, page },
            api::Read::Checks { commit } => Read::Checks { commit },
            api::Read::Protection { branch } => Read::Protection { branch },
            api::Read::Job { attempt, max_bytes } => Read::Job { attempt, max_bytes },
        };
        Self {
            repository,
            read,
            stage: Stage::New,
            response: Response::None,
            row: None,
            request: None,
            rows: List::with_capacity(0),
            size: 0,
            count: 0,
            page: 1,
            limit: 0,
            issues: issues::State::new(),
            pulls: pulls::State::new(),
            checks: checks::State::new(),
            login: None,
            collaborators: List::with_capacity(0),
            remarks: List::with_capacity(0),
            reviews: List::with_capacity(0),
            last_review: 0,
            raw_cap: 0,
            job_step: None,
            files: files::State::new(),
            protection: protection::State::new(),
        }
    }
    pub fn begin(&mut self, bounds: Bounds, _word: &[u8], tables: &mut Tables) -> Result<Step, api::Error> {
        if self.stage != Stage::New {
            return Err(api::Error::InvalidAnswer);
        }
        match &self.read {
            Read::Locate { owner, name } => {
                self.stage = Stage::Read;
                return Ok(self.send(Request::Locate { owner: owner.clone(), name: name.clone() }));
            }
            Read::Settings => {
                self.stage = Stage::Read;
                return Ok(self.send(Request::Repository { id: u64::from(self.repository.repository) }));
            }
            Read::Branches { prefix } => {
                if prefix.is_empty() {
                    return Err(api::Error::Empty);
                }
                let capacity = size::capacity(bounds, crate::measure::STRING_BYTES)?;
                self.limit = size::page_size(bounds, capacity)?;
                self.rows = List::with_capacity(capacity);
            }
            Read::Items { .. } | Read::Item { .. } => self.issue_admission(bounds)?,
            Read::Pull { .. } | Read::PullFor { .. } | Read::Statuses { .. } => self.pull_admission(bounds)?,
            Read::Checks { .. } => self.checks_admission(bounds)?,
            Read::Compare { .. } | Read::PullFiles { .. } => self.files_admission(bounds)?,
            Read::File { path, max_bytes, .. } => {
                let path = u32::try_from(path.len()).ok().ok_or(api::Error::TooLarge)?;
                let room = bounds.answer_bytes.checked_sub(path).ok_or(api::Error::TooLarge)?;
                self.raw_cap = room.min(*max_bytes);
            }
            Read::Reviews { .. } => {
                let capacity = size::capacity(bounds, crate::measure::REVIEW_BYTES)?;
                self.limit = size::page_size(bounds, capacity)?;
                self.reviews = List::with_capacity(capacity);
            }
            Read::Remarks { .. } => {
                let capacity = size::capacity(bounds, crate::measure::REMARK_BYTES)?;
                self.limit = size::page_size(bounds, capacity)?;
                self.remarks = List::with_capacity(capacity);
            }
            Read::Collaborators { .. } => {
                let capacity = size::capacity(bounds, crate::measure::COLLABORATOR_BYTES)?;
                self.limit = size::page_size(bounds, capacity)?;
                self.collaborators = List::with_capacity(capacity);
            }
            Read::Permission { user } => {
                if let Some(login) = tables.user(self.repository.forge, *user) {
                    self.login = Some(Box::from(login));
                }
            }
            Read::Protection { .. } => self.protection_admission(bounds)?,
            Read::Branch { .. } | Read::Job { .. } => {}
        }
        self.row = tables.repository(self.repository).cloned();
        if self.row.is_none() {
            self.stage = Stage::Resolve;
            return Ok(self.send(Request::Repository { id: u64::from(self.repository.repository) }));
        }
        self.initial()
    }
    pub fn pending(&mut self) -> Option<Exchange> {
        self.request.take()
    }
    pub fn set_pending(&mut self, exchange: Exchange) {
        self.request = Some(exchange);
    }
    fn send(&mut self, request: Request) -> Step {
        self.response = Response::Head;
        self.count = 0;
        Step::send(request)
    }
    fn initial(&mut self) -> Result<Step, api::Error> {
        match self.read {
            Read::Protection { .. } => return self.protection_initial(),
            Read::Checks { .. } => return self.checks_initial(),
            Read::Job { .. } => return self.job_initial(),
            Read::Permission { .. } => return self.permission_initial(),
            Read::Compare { .. } => return self.compare_initial(),
            Read::PullFiles { .. } => return self.pull_files_initial(),
            Read::File { .. } => return self.file_initial(),
            Read::Reviews { .. } => return self.reviews_initial(),
            Read::Remarks { .. } => return self.remarks_initial(),
            Read::Collaborators { .. } => return self.collaborators_initial(),
            Read::Pull { .. } | Read::PullFor { .. } | Read::Statuses { .. } => return self.pull_initial(),
            Read::Locate { .. }
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Items { .. }
            | Read::Item { .. } => {}
        }
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let owner = row.owner.clone();
        let name = row.name.clone();
        let request = match &self.read {
            Read::Branch { branch } => Request::Branch { owner, name, branch: branch.clone() },
            Read::Branches { .. } => Request::Branches { owner, name, page: self.page, per_page: self.limit },
            Read::Items { since, page, .. } => Request::Issues {
                owner,
                name,
                since: skein_lib::Wall::from_nanos(since.as_nanos()),
                page: *page,
                per_page: self.limit,
            },
            Read::Item { number, .. } => Request::Issue { owner, name, number: *number },
            Read::Pull { .. }
            | Read::PullFor { .. }
            | Read::Checks { .. }
            | Read::Statuses { .. }
            | Read::Locate { .. }
            | Read::Settings
            | Read::Permission { .. }
            | Read::Collaborators { .. }
            | Read::Remarks { .. }
            | Read::Reviews { .. }
            | Read::File { .. }
            | Read::Compare { .. }
            | Read::PullFiles { .. }
            | Read::Protection { .. }
            | Read::Job { .. } => return Err(api::Error::InvalidAnswer),
        };
        self.stage = Stage::Read;
        Ok(self.send(request))
    }
    /// Admission and call-order errors use `Err`; exchange verdicts finish the plan.
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        if self.stage == Stage::New || self.stage == Stage::Done {
            return Err(api::Error::InvalidAnswer);
        }
        // A second head or a body before the head is a caller ordering error.
        match (&heard, &self.response) {
            (Heard::Head { .. }, Response::Head)
            | (Heard::Row(_) | Heard::Done(_), Response::Body { .. } | Response::Refusal { .. })
            | (Heard::Failed(_), Response::Head | Response::Body { .. } | Response::Refusal { .. }) => {}
            (Heard::Head { .. }, Response::None | Response::Body { .. } | Response::Refusal { .. })
            | (Heard::Row(_) | Heard::Done(_), Response::None | Response::Head)
            | (Heard::Failed(_), Response::None) => return Err(api::Error::InvalidAnswer),
        }
        match self.advance(heard, bounds, word, tables) {
            Ok(step) => Ok(step),
            Err(error) => {
                self.stage = Stage::Done;
                self.response = Response::None;
                Ok(Some(Step::Done(Err(error))))
            }
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "the read dispatcher exhaustively handles heads, rows and terminal bodies, including write results"
    )]
    fn advance(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, head } => {
                if self.stage == Stage::JobLog && status == 302 {
                    return self.job_redirect(&head);
                }
                if self.stage == Stage::Log && (300..400).contains(&status) {
                    return Err(api::Error::Unavailable);
                }
                if self.stage == Stage::JobLog && status == 200 {
                    return Err(api::Error::InvalidAnswer);
                }
                if status == 200 {
                    if head.json {
                        if self.stage == Stage::Raw {
                            return Err(api::Error::Missing);
                        }
                        if self.stage == Stage::Before || self.stage == Stage::After {
                            return Err(api::Error::InvalidAnswer);
                        }
                    }
                    match self.read {
                        Read::Items { .. } if self.stage == Stage::Read => {
                            let date = head.date.ok_or(api::Error::InvalidAnswer)?;
                            self.issues.set_now(skein_lib::Time::from_nanos(date.as_nanos()));
                        }
                        Read::Items { .. }
                        | Read::Item { .. }
                        | Read::Locate { .. }
                        | Read::Settings
                        | Read::Branch { .. }
                        | Read::Branches { .. }
                        | Read::Pull { .. }
                        | Read::PullFor { .. }
                        | Read::Checks { .. }
                        | Read::Statuses { .. }
                        | Read::Permission { .. }
                        | Read::Collaborators { .. }
                        | Read::Remarks { .. }
                        | Read::Reviews { .. }
                        | Read::File { .. }
                        | Read::Compare { .. }
                        | Read::PullFiles { .. }
                        | Read::Protection { .. }
                        | Read::Job { .. } => {}
                    }
                    self.response = Response::Body { next: head.next };
                    return Ok(None);
                }
                if status == 404 && (self.stage == Stage::Before || self.stage == Stage::After) {
                    return self.side_done(None, bounds);
                }
                // Both limit statuses wait for Done so secondary-limit text can be
                // classified. If transport fails meanwhile, the heard status wins.
                if status == 403 || status == 429 || (status == 404 && self.stage == Stage::Classic) {
                    self.response = Response::Refusal { status, head };
                    return Ok(None);
                }
                Err(self.read_failure(status, &head, None, tables))
            }
            Heard::Row(row) => {
                match self.response {
                    Response::Refusal { .. } => return Ok(None),
                    Response::Body { .. } => {}
                    Response::None | Response::Head => return Err(api::Error::InvalidAnswer),
                }
                self.keep_row(row, bounds, word, tables)?;
                Ok(None)
            }
            Heard::Done(body) => {
                match &self.response {
                    Response::Refusal { status, head } => {
                        let refusal = match &body {
                            Body::Document(document::Document::Error(refusal)) => Some(refusal),
                            Body::Document(
                                document::Document::Repository(_)
                                | document::Document::Branch(_)
                                | document::Document::Issue(_)
                                | document::Document::Pull(_)
                                | document::Document::Job(_)
                                | document::Document::Combined(_)
                                | document::Document::Rows
                                | document::Document::Delivery(_)
                                | document::Document::Installation(_)
                                | document::Document::Minted(_)
                                | document::Document::Viewer(_)
                                | document::Document::Merge(_)
                                | document::Document::Enqueued(_)
                                | document::Document::PullGraphql(_)
                                | document::Document::Graphql(_)
                                | document::Document::App(_)
                                | document::Document::HookConfig(_)
                                | document::Document::Conversion(_)
                                | document::Document::User(_)
                                | document::Document::Compare(_)
                                | document::Document::Permission(_)
                                | document::Document::Id(_)
                                | document::Document::Number(_)
                                | document::Document::Done
                                | document::Document::Ruleset(_)
                                | document::Document::ClassicProtection(_)
                                | document::Document::BotUser(_),
                            )
                            | Body::Raw { .. }
                            | Body::Empty
                            | Body::Malformed(_) => None,
                        };
                        if *status == 404
                            && self.stage == Stage::Classic
                            && let Some(refusal) = refusal
                            && refusal.message.as_ref() == b"Branch not protected"
                        {
                            return self.classic_done(None, bounds);
                        }
                        return Err(self.read_failure(*status, head, refusal, tables));
                    }
                    Response::Body { .. } => {}
                    Response::None | Response::Head => return Err(api::Error::InvalidAnswer),
                }
                match body {
                    Body::Document(document) => self.done(document, bounds, word, tables),
                    Body::Malformed(malformed) => Err(failure::malformed(malformed)),
                    Body::Raw { bytes, more } => self.raw_done(bytes, more, bounds),
                    Body::Empty => Err(api::Error::InvalidAnswer),
                }
            }
            Heard::Failed(failed) => match &self.response {
                Response::Refusal { status, head } => Err(self.read_failure(*status, head, None, tables)),
                Response::None | Response::Head | Response::Body { .. } => Err(failure::transport(failed)),
            },
        }
    }
    fn read_failure(
        &self,
        status: u16,
        head: &Head,
        refusal: Option<&document::Refusal>,
        tables: &mut Tables,
    ) -> api::Error {
        let asked =
            if let (Read::Branch { .. }, Stage::Read) | (Read::File { .. }, Stage::Raw) = (&self.read, self.stage) {
                crate::github::failure::Asked::Leaf
            } else {
                crate::github::failure::Asked::Read
            };
        let error = failure::read(asked, status, head, refusal, self.repository, tables);
        if let Read::Job { .. } = self.read
            && error == api::Error::Missing
        {
            return api::Error::MissingJob;
        }
        error
    }
    fn keep_row(
        &mut self,
        row: document::Row,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<(), api::Error> {
        match self.read {
            Read::Protection { .. } => return self.protection_row(row, bounds),
            Read::Checks { .. } => return self.checks_row(row, bounds),
            Read::Compare { .. } | Read::PullFiles { .. } => return self.file_row(row, bounds),
            Read::Reviews { .. } => return self.review_row(row, bounds, word),
            Read::Remarks { .. } => return self.remark_row(row, bounds),
            Read::Collaborators { .. } => return self.collaborator_row(row, bounds, tables),
            Read::Items { .. } | Read::Item { .. } => return self.issue_row(row, bounds, word),
            Read::Pull { .. } | Read::PullFor { .. } | Read::Statuses { .. } => return self.pull_row(row, bounds),
            Read::Locate { .. }
            | Read::Settings
            | Read::Branch { .. }
            | Read::Branches { .. }
            | Read::Permission { .. }
            | Read::File { .. }
            | Read::Job { .. } => {}
        }
        if self.stage != Stage::Read {
            return Err(api::Error::InvalidAnswer);
        }
        let Read::Branches { prefix } = &self.read else {
            return Err(api::Error::InvalidAnswer);
        };
        self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
        if self.count > self.limit {
            return Err(api::Error::TooLarge);
        }
        match row {
            document::Row::SkippedBranch => Ok(()),
            document::Row::Branch(name) => {
                if !name.starts_with(prefix) {
                    return Ok(());
                }
                if self.rows.len() >= bounds.rows {
                    return Err(api::Error::TooLarge);
                }
                self.size = size::add(self.size, &name, bounds)?;
                match self.rows.push(name) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(api::Error::TooLarge),
                }
            }
            document::Row::Issue(_)
            | document::Row::Comment(_)
            | document::Row::Log(_)
            | document::Row::Installation(_)
            | document::Row::Pull(_)
            | document::Row::Context(_)
            | document::Row::Run(_)
            | document::Row::Commit(_)
            | document::Row::File(_)
            | document::Row::Review(_)
            | document::Row::Remark(_)
            | document::Row::Collaborator(_)
            | document::Row::Rule(_)
            | document::Row::Reachable(_) => Err(api::Error::InvalidAnswer),
        }
    }
}
