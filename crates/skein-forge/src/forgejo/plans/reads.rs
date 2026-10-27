//! JSON read plans, streamed rows and their running answer bound (forge.md, 3.2).
use crate::forgejo::plans::{
    Body, Heard,
    plan::{Bounds, Exchange, Step},
    reading,
    tables::{Repository, Tables},
};
mod done;
pub mod keep;
use keep::{keep_document, keep_row, page_bound};
pub mod size;
use crate::api;
use crate::forgejo::{
    document as d,
    id::ObjectId,
    request::{self as r, Request},
};
use alloc::boxed::Box;
use skein_lib::{List, Time, Wall, bytes};
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Stage {
    Resolve,
    User,
    Read,
    Comments,
    Pulls,
    Status,
    Branches,
    Protection,
    Collaborators,
    Permissions,
}
#[derive(Debug)]
enum Rows {
    None,
    Items(List<api::Summary>),
    Comments(List<api::Comment>),
    Reviews(List<api::Review>),
    Statuses(List<api::Status>),
    Remarks(List<api::Remark>),
    Branches(List<Box<[u8]>>),
    Collaborators(List<api::Collaborator>),
}
#[derive(Debug)]
pub struct Plan {
    repository: api::Repository,
    read: api::Read,
    stage: Stage,
    row: Option<Repository>,
    request: Option<Exchange>,
    turn: Option<super::turn::Plan>,
    contents: Option<crate::forgejo::plans::contents::Plan>,
    actions: Option<crate::forgejo::plans::actions::Plan>,
    rows: Rows,
    item: Option<api::Summary>,
    pull: Option<d::PullRequest>,
    size: u64,
    count: u32,
    page: u32,
    limit: u32,
    position: u32,
    more: bool,
    missing: bool,
    now: Time,
    cut: bool,
    bound: Option<u32>,
    after: u64,
}
impl Plan {
    #[must_use]
    pub fn new(repository: api::Repository, read: api::Read) -> Self {
        Self {
            repository,
            read,
            stage: Stage::Read,
            row: None,
            request: None,
            turn: None,
            contents: None,
            actions: None,
            rows: Rows::None,
            item: None,
            pull: None,
            size: 0,
            count: 0,
            page: 1,
            limit: 1,
            position: 0,
            more: false,
            missing: false,
            now: Time::ZERO,
            cut: false,
            bound: None,
            after: 0,
        }
    }
    pub fn begin(&mut self, bounds: Bounds, _word: &[u8], tables: &mut Tables) -> Result<Step, api::Error> {
        if let api::Read::Branches { prefix } = &self.read
            && prefix.is_empty()
        {
            return Err(api::Error::Empty);
        }
        self.row = tables.repository(self.repository).cloned();
        match &self.read {
            api::Read::Locate { owner, name } => {
                return Ok(self.send(Request::Repository { at: r::Repo { owner: owner.clone(), name: name.clone() } }));
            }
            api::Read::Settings => {
                return Ok(self.send(Request::RepositoryById { id: u64::from(self.repository.repository) }));
            }
            api::Read::Items { .. }
            | api::Read::Item { .. }
            | api::Read::Pull { .. }
            | api::Read::PullFor { .. }
            | api::Read::Reviews { .. }
            | api::Read::Statuses { .. }
            | api::Read::Remarks { .. }
            | api::Read::Branch { .. }
            | api::Read::Branches { .. }
            | api::Read::Protection { .. }
            | api::Read::Collaborators { .. }
            | api::Read::Permission { .. }
            | api::Read::PullFiles { .. }
            | api::Read::Compare { .. }
            | api::Read::Checks { .. }
            | api::Read::Runs { .. }
            | api::Read::Run { .. }
            | api::Read::Tree { .. }
            | api::Read::Job { .. }
            | api::Read::File { .. } => {}
        }
        if self.row.is_none() {
            self.stage = Stage::Resolve;
            return Ok(self.send(Request::RepositoryById { id: u64::from(self.repository.repository) }));
        }
        self.initial(bounds, tables)
    }
    pub fn pending(&mut self) -> Option<Exchange> {
        self.request.take()
    }
    pub fn set_pending(&mut self, request: Exchange) {
        self.request = Some(request);
    }
    fn send(&mut self, request: Request) -> Step {
        self.bound = page_bound(&request);
        self.count = 0;
        Step::send(request)
    }
    fn at(&self) -> r::Repo {
        let row = self.row.as_ref().expect("resolved before path request");
        r::Repo { owner: row.owner.clone(), name: row.name.clone() }
    }
    fn paged(&mut self, page: u32, bounds: Bounds) -> r::Page {
        self.page = page;
        self.limit = bounds.page_size;
        r::Page { page: Some(page), limit: Some(self.limit) }
    }
    fn initial(&mut self, bounds: Bounds, tables: &mut Tables) -> Result<Step, api::Error> {
        self.stage = Stage::Read;
        let at = self.at();
        let capacity = bounds.rows;
        let request = match self.read.clone() {
            api::Read::Runs { workflow, commit, page } => {
                return Ok(self.turn(super::turn::Mode::Runs { workflow, commit, page }, bounds));
            }
            api::Read::Run { run } => return Ok(self.turn(super::turn::Mode::Run(run), bounds)),
            api::Read::Tree { object, page } => return Ok(self.turn(super::turn::Mode::Tree { object, page }, bounds)),
            api::Read::Items { since, page, kind } => {
                self.rows = Rows::Items(List::with_capacity(bounds.page_size));
                let kind = match kind {
                    Some(api::Kind::Issue) => Some(r::IssueKind::Issues),
                    Some(api::Kind::Pull) => Some(r::IssueKind::Pulls),
                    None => None,
                };
                Request::Issues {
                    at,
                    state: Some(r::StateFilter::All),
                    kind,
                    since: Some(Wall::from_nanos(since.as_nanos())),
                    sort: Some(r::Sort::LeastUpdate),
                    page: self.paged(page, bounds),
                }
            }
            api::Read::Item { number, after } => {
                self.after = after;
                self.rows = Rows::Comments(List::with_capacity(0));
                Request::Issue { at, number }
            }
            api::Read::Pull { number } => Request::Pull { at, number },
            api::Read::PullFor { head, base } => Request::PullFor { at, head, base },
            api::Read::Reviews { number, page } => {
                self.rows = Rows::Reviews(List::with_capacity(bounds.page_size));
                Request::Reviews { at, number, page: self.paged(page, bounds) }
            }
            api::Read::Statuses { commit, page } => {
                self.rows = Rows::Statuses(List::with_capacity(bounds.page_size));
                Request::CombinedStatus { at, commit: self.id(commit), page: self.paged(page, bounds) }
            }
            api::Read::Remarks { number, review, page } => {
                self.rows = Rows::Remarks(List::with_capacity(bounds.page_size));
                self.paged(page, bounds);
                Request::ReviewComments { at, number, review }
            }
            api::Read::Branch { branch: name } | api::Read::Protection { branch: name } => Request::Branch { at, name },
            api::Read::Branches { prefix } => {
                let room = bounds
                    .answer_bytes
                    .checked_div(u32::try_from(crate::measure::STRING_BYTES).expect("box size"))
                    .expect("nonzero width");
                self.rows = Rows::Branches(List::with_capacity(capacity.min(room)));
                self.stage = Stage::Branches;
                Request::Branches { at, prefix: Some(prefix), page: r::Page { page: None, limit: None } }
            }
            api::Read::Collaborators { page } => {
                self.rows = Rows::Collaborators(List::with_capacity(bounds.page_size));
                self.stage = Stage::Collaborators;
                Request::Collaborators { at, page: self.paged(page, bounds) }
            }
            api::Read::Permission { user } => return Ok(self.permission(user, tables)),
            read @ (api::Read::PullFiles { .. } | api::Read::Compare { .. } | api::Read::File { .. }) => {
                let mut plan = crate::forgejo::plans::contents::Plan::new(
                    read,
                    self.row.as_ref().expect("resolved").clone(),
                    bounds,
                )?;
                let step = plan.begin(bounds)?;
                self.contents = Some(plan);
                return Ok(step);
            }
            read @ (api::Read::Checks { .. } | api::Read::Job { .. }) => {
                let mut plan = crate::forgejo::plans::actions::Plan::new(
                    read,
                    self.row.as_ref().expect("resolved").clone(),
                    bounds,
                )?;
                let step = plan.begin();
                self.actions = Some(plan);
                return Ok(step);
            }
            api::Read::Locate { .. } | api::Read::Settings => {
                return Err(api::Error::Unavailable);
            }
        };
        Ok(self.send(request))
    }
    fn turn(&mut self, mode: super::turn::Mode, bounds: Bounds) -> Step {
        let plan = super::turn::Plan::new(mode, self.row.as_ref().expect("resolved").clone(), bounds);
        let step = plan.begin(bounds);
        self.turn = Some(plan);
        step
    }
    fn id(&self, bytes: api::Commit) -> ObjectId {
        ObjectId { bytes, format: self.row.as_ref().expect("resolved format").object_format }
    }
    fn permission(&mut self, user: u64, tables: &mut Tables) -> Step {
        let request = match tables.user(self.repository.forge, user) {
            Some(login) => Request::Permission { at: self.at(), login: bytes::copy_of(login) },
            None => {
                self.stage = Stage::User;
                Request::UserSearch { uid: user, limit: Some(1) }
            }
        };
        self.send(request)
    }
    #[must_use]
    pub fn after(&self) -> u64 {
        self.after
    }
    #[must_use]
    pub fn raw_room(&self, answer_bytes: u32) -> u32 {
        match &self.contents {
            Some(plan) => plan.raw_room(answer_bytes),
            None => match &self.actions {
                Some(plan) => plan.raw_room(answer_bytes),
                None => answer_bytes,
            },
        }
    }
    pub fn take_cut(&mut self) -> bool {
        core::mem::replace(&mut self.cut, false)
    }
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        if let Some(plan) = &mut self.turn {
            return plan.heard(heard, bounds, self.repository, tables);
        }
        if let Some(plan) = &mut self.contents {
            return plan.heard(heard, bounds, self.repository, tables);
        }
        if let Some(plan) = &mut self.actions {
            return plan.heard(heard, bounds, self.repository, tables);
        }
        match heard {
            Heard::Head { status, fields } => {
                if self.stage == Stage::Branches && status == 404 {
                    self.missing = true;
                    return Ok(None);
                }
                if status != 200 {
                    let asked = if self.stage == Stage::Read
                        && let api::Read::Branch { .. } = self.read
                    {
                        super::failure::Asked::Leaf
                    } else {
                        super::failure::Asked::Read
                    };
                    let classified = super::failure::classify(asked, status, None, fields.retry_after);
                    match classified.drop {
                        super::failure::Drop::Nothing => {}
                        super::failure::Drop::Path | super::failure::Drop::Row => {
                            tables.drop_repository(self.repository);
                        }
                    }
                    return Err(classified.error);
                }
                if self.stage == Stage::Read
                    && let api::Read::Items { .. } = &self.read
                {
                    self.now = reading::time(fields.date.ok_or(api::Error::InvalidAnswer)?);
                }
                Ok(None)
            }
            Heard::Row(row) => {
                self.row(row, bounds, word, tables)?;
                Ok(None)
            }
            Heard::Done(body) => {
                if self.missing {
                    return self.terminal(api::Answer::Branches(Box::new([])), bounds);
                }
                match body {
                    Body::Document(document) => self.done(document, bounds, word, tables),
                    Body::Malformed(crate::forgejo::decode::Malformed::Bound { field }) => {
                        Err(if field == b"log" { api::Error::InvalidAnswer } else { api::Error::TooLarge })
                    }
                    Body::Malformed(_) | Body::Empty | Body::Raw { .. } => Err(api::Error::InvalidAnswer),
                }
            }
            Heard::Failed(failure) => Err(crate::forgejo::plans::failure::transport(failure)),
        }
    }
    fn add_size(&mut self, size: u64, bounds: Bounds) -> bool {
        let Some(total) = self.size.checked_add(size) else {
            return false;
        };
        if total > u64::from(bounds.answer_bytes) {
            return false;
        }
        self.size = total;
        true
    }
    fn row(&mut self, row: d::Row, bounds: Bounds, word: &[u8], tables: &mut Tables) -> Result<(), api::Error> {
        self.count = self.count.checked_add(1).ok_or(api::Error::TooLarge)?;
        if let Some(bound) = self.bound
            && self.count > bound
        {
            return Err(api::Error::TooLarge);
        }
        keep_row(&row, self.repository.forge, tables);
        match row {
            d::Row::Pull(pull) => self.lookup_row(pull),
            d::Row::User(user) => {
                if self.stage == Stage::User {
                    return Ok(());
                }
                let size = u64::try_from(crate::measure::COLLABORATOR_BYTES).expect("collaborator width");
                if !self.add_size(size, bounds) {
                    return Err(api::Error::TooLarge);
                }
                match &mut self.rows {
                    Rows::Collaborators(rows) => {
                        match rows.push(api::Collaborator { user: user.id, permission: api::Permission::None }) {
                            Ok(()) => Ok(()),
                            Err(_) => Err(api::Error::TooLarge),
                        }
                    }
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Reviews(_)
                    | Rows::Statuses(_)
                    | Rows::Remarks(_)
                    | Rows::Branches(_) => Err(api::Error::InvalidAnswer),
                }
            }
            d::Row::Protection(_) => Ok(()),
            d::Row::Issue(issue) => self.append(
                api::Answer::Items { items: Box::new([reading::summary(word, issue)]), more: false, now: self.now },
                bounds,
            ),
            d::Row::Comment(comment) => {
                let item = self.item.as_ref().ok_or(api::Error::InvalidAnswer)?.clone();
                self.append(
                    api::Answer::Item { item, comments: Box::new([reading::comment(word, comment)]), more: false },
                    bounds,
                )
            }
            d::Row::Review(review) => {
                match reading::review(word, review, self.row.as_ref().expect("resolved format").object_format)? {
                    Some(review) => {
                        self.append(api::Answer::Reviews { reviews: Box::new([review]), more: false }, bounds)
                    }
                    None => Ok(()),
                }
            }
            d::Row::Status(_) if self.stage == Stage::Status => Ok(()),
            d::Row::Status(status) => self.append(
                api::Answer::Statuses {
                    ci: api::Ci::None,
                    statuses: Box::new([reading::status(status, &self.row.as_ref().expect("resolved URL").html_url)?]),
                    more: false,
                },
                bounds,
            ),
            d::Row::ReviewComment(remark) => {
                let skip = self.page.saturating_sub(1).checked_mul(self.limit).ok_or(api::Error::TooLarge)?;
                if self.count <= skip {
                    return Ok(());
                }
                if self.count > skip.saturating_add(self.limit) {
                    self.more = true;
                    self.cut = true;
                    return Ok(());
                }
                self.append(
                    api::Answer::Remarks { remarks: Box::new([reading::remark(word, remark)]), more: false },
                    bounds,
                )
            }
            d::Row::Branch(branch) => {
                if let api::Read::Branches { prefix } = &self.read
                    && !branch.name.starts_with(prefix)
                {
                    return Err(api::Error::InvalidAnswer);
                }
                self.append(api::Answer::Branches(Box::new([branch.name])), bounds)
            }
            d::Row::Found(_)
            | d::Row::Reachable(_)
            | d::Row::Token(_)
            | d::Row::Hook(_)
            | d::Row::Team(_)
            | d::Row::File(_)
            | d::Row::Commit(_)
            | d::Row::Entry(_)
            | d::Row::Run(_)
            | d::Row::Step(_)
            | d::Row::Job(_) => Err(api::Error::InvalidAnswer),
        }
    }
    fn lookup_row(&mut self, pull: d::PullRequest) -> Result<(), api::Error> {
        if self.stage == Stage::Pulls {
            if self.more {
                return Ok(());
            }
            let found = match &self.read {
                api::Read::PullFor { head, base } => {
                    pull.head.name == *head
                        && pull.head.repo_id == u64::from(self.repository.repository)
                        && pull.base.name == *base
                }
                api::Read::Locate { .. }
                | api::Read::Items { .. }
                | api::Read::Item { .. }
                | api::Read::Pull { .. }
                | api::Read::Reviews { .. }
                | api::Read::Statuses { .. }
                | api::Read::Remarks { .. }
                | api::Read::Branch { .. }
                | api::Read::Branches { .. }
                | api::Read::Protection { .. }
                | api::Read::Settings
                | api::Read::Collaborators { .. }
                | api::Read::Permission { .. }
                | api::Read::PullFiles { .. }
                | api::Read::Compare { .. }
                | api::Read::Checks { .. }
                | api::Read::Runs { .. }
                | api::Read::Run { .. }
                | api::Read::Tree { .. }
                | api::Read::Job { .. }
                | api::Read::File { .. } => false,
            };
            if found {
                self.pull = Some(pull);
                self.more = true;
            }
            return Ok(());
        }
        Err(api::Error::InvalidAnswer)
    }
    fn append(&mut self, answer: api::Answer, bounds: Bounds) -> Result<(), api::Error> {
        let mut size = size::answer(&answer, bounds.rows)?;
        match &answer {
            api::Answer::Item { item, .. } => {
                size = size.saturating_sub(size::summary(item, bounds.rows)?);
            }
            api::Answer::Items { .. }
            | api::Answer::Pull(_)
            | api::Answer::Reviews { .. }
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Located { .. }
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::File { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Queued
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
        let room = match &self.rows {
            Rows::None => false,
            Rows::Items(rows) => rows.len() < rows.capacity(),
            Rows::Comments(rows) => rows.len() < rows.capacity(),
            Rows::Reviews(rows) => rows.len() < rows.capacity(),
            Rows::Statuses(rows) => rows.len() < rows.capacity(),
            Rows::Remarks(rows) => rows.len() < rows.capacity(),
            Rows::Branches(rows) => rows.len() < rows.capacity(),
            Rows::Collaborators(rows) => rows.len() < rows.capacity(),
        };
        if !room || !self.add_size(size, bounds) {
            match &self.rows {
                Rows::Comments(_) | Rows::Remarks(_) => {
                    self.more = true;
                    self.cut = true;
                    return Ok(());
                }
                Rows::None
                | Rows::Items(_)
                | Rows::Reviews(_)
                | Rows::Statuses(_)
                | Rows::Branches(_)
                | Rows::Collaborators(_) => return Err(api::Error::TooLarge),
            }
        }
        self.push(answer)
    }
    fn push_items(&mut self, items: Box<[api::Summary]>) -> Result<(), api::Error> {
        let rows = match &mut self.rows {
            Rows::Items(rows) => rows,
            Rows::None
            | Rows::Comments(_)
            | Rows::Reviews(_)
            | Rows::Statuses(_)
            | Rows::Remarks(_)
            | Rows::Branches(_)
            | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
        };
        for row in items {
            rows.push(row).expect("room checked");
        }
        Ok(())
    }
    #[expect(clippy::too_many_lines, reason = "exhaustive answer selection names every vocabulary variant")]
    fn push(&mut self, answer: api::Answer) -> Result<(), api::Error> {
        match answer {
            api::Answer::Items { items, .. } => self.push_items(items)?,
            api::Answer::Item { comments, .. } => {
                let rows = match &mut self.rows {
                    Rows::Comments(rows) => rows,
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Reviews(_)
                    | Rows::Statuses(_)
                    | Rows::Remarks(_)
                    | Rows::Branches(_)
                    | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
                };
                for row in comments {
                    rows.push(row).expect("room checked");
                }
            }
            api::Answer::Reviews { reviews, .. } => {
                let rows = match &mut self.rows {
                    Rows::Reviews(rows) => rows,
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Statuses(_)
                    | Rows::Remarks(_)
                    | Rows::Branches(_)
                    | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
                };
                for row in reviews {
                    rows.push(row).expect("room checked");
                }
            }
            api::Answer::Statuses { statuses, .. } => {
                let rows = match &mut self.rows {
                    Rows::Statuses(rows) => rows,
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Reviews(_)
                    | Rows::Remarks(_)
                    | Rows::Branches(_)
                    | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
                };
                for row in statuses {
                    rows.push(row).expect("room checked");
                }
            }
            api::Answer::Remarks { remarks, .. } => {
                let rows = match &mut self.rows {
                    Rows::Remarks(rows) => rows,
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Reviews(_)
                    | Rows::Statuses(_)
                    | Rows::Branches(_)
                    | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
                };
                for row in remarks {
                    rows.push(row).expect("room checked");
                }
            }
            api::Answer::Branches(branches) => {
                let rows = match &mut self.rows {
                    Rows::Branches(rows) => rows,
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Reviews(_)
                    | Rows::Statuses(_)
                    | Rows::Remarks(_)
                    | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
                };
                for row in branches {
                    rows.push(row).expect("room checked");
                }
            }
            api::Answer::Pull(_)
            | api::Answer::Commit(_)
            | api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Located { .. }
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::Job { .. }
            | api::Answer::File { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Queued
            | api::Answer::Branch(_)
            | api::Answer::Done => return Err(api::Error::InvalidAnswer),
        }
        Ok(())
    }
}
