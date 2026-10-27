//! Contents plans: list changed paths, then read each side within the answer bound.
use crate::api;
use crate::forgejo::plans::{
    Body, Heard,
    plan::{Bounds, Exchange, Step},
    reading,
    tables::{Repository, Tables},
};
use crate::forgejo::{document as d, id::ObjectId, request as r};
use alloc::boxed::Box;
use skein_lib::{List, bytes};
#[derive(Debug)]
enum Mode {
    Pull { number: u64, head: api::Commit, page: u32 },
    Compare { before: api::Commit, after: api::Commit },
    File { head: api::Commit, path: Box<[u8]>, max: u32 },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Pull,
    Files,
    Compare,
    Reverse,
    Before,
    After,
    File,
}
#[derive(Debug)]
pub struct Plan {
    mode: Mode,
    stage: Stage,
    row: Repository,
    changed: List<d::ChangedFile>,
    files: List<api::File>,
    commits: List<api::Commit>,
    position: u32,
    before: api::Commit,
    after: api::Commit,
    size: u64,
    limit: u32,
    missing: bool,
    more: Option<bool>,
    contains: bool,
}
impl Plan {
    pub fn new(read: api::Read, row: Repository, bounds: Bounds) -> Result<Self, api::Error> {
        let (mode, stage, limit) = match read {
            api::Read::PullFiles { number, head, page } => {
                (Mode::Pull { number, head, page }, Stage::Pull, bounds.page_size)
            }
            api::Read::Compare { before, after } => (Mode::Compare { before, after }, Stage::Compare, bounds.rows),
            api::Read::File { head, path, max_bytes } => (Mode::File { head, path, max: max_bytes }, Stage::File, 0),
            api::Read::Locate { .. }
            | api::Read::Items { .. }
            | api::Read::Item { .. }
            | api::Read::Pull { .. }
            | api::Read::PullFor { .. }
            | api::Read::Reviews { .. }
            | api::Read::Statuses { .. }
            | api::Read::Remarks { .. }
            | api::Read::Branch { .. }
            | api::Read::Branches { .. }
            | api::Read::Protection { .. }
            | api::Read::Settings
            | api::Read::Collaborators { .. }
            | api::Read::Permission { .. }
            | api::Read::Checks { .. }
            | api::Read::Runs { .. }
            | api::Read::Run { .. }
            | api::Read::Tree { .. }
            | api::Read::Artifact { .. }
            | api::Read::Commits { .. }
            | api::Read::Job { .. } => return Err(api::Error::InvalidAnswer),
        };
        Ok(Self {
            mode,
            stage,
            row,
            changed: List::with_capacity(limit),
            files: List::with_capacity(limit),
            commits: List::with_capacity(if stage == Stage::Compare { bounds.rows } else { 0 }),
            position: 0,
            before: [0; 32],
            after: [0; 32],
            size: 0,
            limit,
            missing: false,
            more: None,
            contains: false,
        })
    }
    pub fn begin(&mut self, bounds: Bounds) -> Result<Step, api::Error> {
        let at = self.at();
        match &self.mode {
            Mode::Pull { number, .. } => Ok(Step::send(r::Request::Pull { at, number: *number })),
            Mode::Compare { before, after } => {
                self.before = *before;
                self.after = *after;
                let before = self.id(*before);
                let after = self.id(*after);
                Ok(Step::send(r::Request::Compare { at, before, after, files: true }))
            }
            Mode::File { head, path, max } => {
                self.size = u64::try_from(path.len()).expect("bounded path");
                self.check_size(bounds)?;
                Ok(Step::Send(Exchange {
                    request: r::Request::Raw { at, path: path.clone(), reference: self.id(*head) },
                    range: Some(crate::forgejo::plans::plan::Range::Head(*max)),
                }))
            }
        }
    }
    fn at(&self) -> r::Repo {
        r::Repo { owner: self.row.owner.clone(), name: self.row.name.clone() }
    }
    fn id(&self, bytes: api::Commit) -> ObjectId {
        ObjectId { bytes, format: self.row.object_format }
    }
    #[must_use]
    pub fn raw_room(&self, answer_bytes: u32) -> u32 {
        answer_bytes.saturating_sub(u32::try_from(self.size).expect("checked answer size"))
    }
    fn check_size(&self, bounds: Bounds) -> Result<(), api::Error> {
        if self.size > u64::from(bounds.answer_bytes) { Err(api::Error::TooLarge) } else { Ok(()) }
    }
    fn add_size(&mut self, size: u64, bounds: Bounds) -> Result<(), api::Error> {
        self.size = self.size.checked_add(size).ok_or(api::Error::TooLarge)?;
        self.check_size(bounds)
    }
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        repository: api::Repository,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, fields } => {
                self.missing = status == 404 && (self.stage == Stage::Before || self.stage == Stage::After);
                if self.missing {
                    return Ok(None);
                }
                let raw = self.stage == Stage::File || self.stage == Stage::Before || self.stage == Stage::After;
                if status != 200 && !(raw && status == 206) {
                    let asked = if self.stage == Stage::File {
                        super::failure::Asked::Leaf
                    } else {
                        super::failure::Asked::Read
                    };
                    let classified = super::failure::classify(asked, status, None, fields.retry_after);
                    match classified.drop {
                        super::failure::Drop::Nothing => {}
                        super::failure::Drop::Path | super::failure::Drop::Row => tables.drop_repository(repository),
                    }
                    return Err(classified.error);
                }
                if self.stage == Stage::Files {
                    self.more = fields.has_more;
                }
                Ok(None)
            }
            Heard::Row(row) => {
                self.row(row, bounds)?;
                Ok(None)
            }
            Heard::Done(body) => {
                if self.missing {
                    return self.side(None, bounds);
                }
                match body {
                    Body::Document(document) => {
                        crate::forgejo::plans::reads::keep::keep_document(&document, repository.forge, tables);
                        self.done(document, bounds)
                    }
                    Body::Raw { bytes, more } => self.raw(bytes, more, bounds),
                    Body::Malformed(crate::forgejo::decode::Malformed::Bound { field }) => {
                        Err(if field == b"log" { api::Error::InvalidAnswer } else { api::Error::TooLarge })
                    }
                    Body::Empty | Body::Malformed(_) => Err(api::Error::InvalidAnswer),
                }
            }
            Heard::Failed(failure) => Err(crate::forgejo::plans::failure::transport(failure)),
        }
    }
    fn row(&mut self, row: d::Row, bounds: Bounds) -> Result<(), api::Error> {
        match row {
            d::Row::File(file) => {
                if self.stage == Stage::Reverse {
                    return Ok(());
                }
                if self.stage != Stage::Files && self.stage != Stage::Compare {
                    return Err(api::Error::InvalidAnswer);
                }
                self.add_size(
                    u64::try_from(crate::measure::FILE_BYTES.saturating_add(file.filename.len())).expect("bounded row"),
                    bounds,
                )?;
                let value = api::File { path: file.filename.clone(), before: None, after: None };
                match self.files.push(value) {
                    Ok(()) => {}
                    Err(_) => return Err(api::Error::TooLarge),
                }
                match self.changed.push(file) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(api::Error::TooLarge),
                }
            }
            d::Row::Commit(commit) => {
                if self.stage == Stage::Reverse {
                    return Ok(());
                }
                if self.stage != Stage::Compare {
                    return Err(api::Error::InvalidAnswer);
                }
                let commit = reading::commit(commit, self.row.object_format).ok_or(api::Error::InvalidAnswer)?;
                self.add_size(u64::try_from(crate::measure::COMMIT_BYTES).expect("commit width"), bounds)?;
                match self.commits.push(commit) {
                    Ok(()) => Ok(()),
                    Err(_) => Err(api::Error::TooLarge),
                }
            }
            d::Row::CommitMessage(_)
            | d::Row::Found(_)
            | d::Row::Reachable(_)
            | d::Row::Token(_)
            | d::Row::User(_)
            | d::Row::Issue(_)
            | d::Row::Comment(_)
            | d::Row::Review(_)
            | d::Row::Status(_)
            | d::Row::ReviewComment(_)
            | d::Row::Pull(_)
            | d::Row::Branch(_)
            | d::Row::Protection(_)
            | d::Row::Entry(_)
            | d::Row::Run(_)
            | d::Row::Hook(_)
            | d::Row::Team(_)
            | d::Row::Step(_)
            | d::Row::Job(_) => Err(api::Error::InvalidAnswer),
        }
    }
    fn done(&mut self, document: d::Document, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        match document {
            d::Document::Pull(pull) => {
                if self.stage != Stage::Pull {
                    return Err(api::Error::InvalidAnswer);
                }
                let (number, head, page) = match self.mode {
                    Mode::Pull { number, head, page } => (number, head, page),
                    Mode::Compare { .. } | Mode::File { .. } => return Err(api::Error::InvalidAnswer),
                };
                if reading::commit(pull.head.sha.ok_or(api::Error::InvalidAnswer)?, self.row.object_format)
                    .ok_or(api::Error::InvalidAnswer)?
                    != head
                {
                    return Err(api::Error::Stale);
                }
                self.before =
                    reading::commit(pull.merge_base.ok_or(api::Error::InvalidAnswer)?, self.row.object_format)
                        .ok_or(api::Error::InvalidAnswer)?;
                self.after = head;
                self.stage = Stage::Files;
                Ok(Some(Step::send(r::Request::PullFiles {
                    at: self.at(),
                    number,
                    page: r::Page { page: Some(page), limit: Some(self.limit) },
                })))
            }
            d::Document::Listed { .. } => {
                if self.stage != Stage::Files {
                    return Err(api::Error::InvalidAnswer);
                }
                self.next_side(bounds)
            }
            d::Document::Compare(comparison) => {
                if self.stage == Stage::Compare {
                    self.stage = Stage::Reverse;
                    Ok(Some(Step::send(r::Request::Compare {
                        at: self.at(),
                        before: self.id(self.after),
                        after: self.id(self.before),
                        files: false,
                    })))
                } else if self.stage == Stage::Reverse {
                    self.contains = comparison.total_commits == 0;
                    self.next_side(bounds)
                } else {
                    Err(api::Error::InvalidAnswer)
                }
            }
            d::Document::Token(_)
            | d::Document::User(_)
            | d::Document::Org(_)
            | d::Document::CreatedFile { .. }
            | d::Document::CreatedRepository(_)
            | d::Document::Hook(_)
            | d::Document::Team(_)
            | d::Document::ApiSettings(_)
            | d::Document::Repository(_)
            | d::Document::RepoSearch { .. }
            | d::Document::Search { .. }
            | d::Document::Issue(_)
            | d::Document::Comment(_)
            | d::Document::Status(_)
            | d::Document::Review(_)
            | d::Document::CombinedStatus(_)
            | d::Document::Branch(_)
            | d::Document::Protection(_)
            | d::Document::Permission(_)
            | d::Document::Tree { .. }
            | d::Document::Runs { .. }
            | d::Document::Job(_)
            | d::Document::Run(_)
            | d::Document::Error(_) => Err(api::Error::InvalidAnswer),
        }
    }
    fn next_side(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        for _ in 0..self.changed.len().saturating_mul(3).saturating_add(1) {
            if self.position == self.changed.len() {
                return self.terminal();
            }
            let file = self.changed.get(self.position).expect("listed file");
            let compare = match self.mode {
                Mode::Compare { .. } => true,
                Mode::Pull { .. } | Mode::File { .. } => false,
            };
            if self.stage == Stage::After {
                self.position = self.position.saturating_add(1);
                self.stage = Stage::Files;
                continue;
            }
            if self.stage == Stage::Before {
                if !compare && file.status == d::FileStatus::Deleted {
                    self.stage = Stage::After;
                    continue;
                }
                self.stage = Stage::After;
                return Ok(Some(Step::send(r::Request::Raw {
                    at: self.at(),
                    path: file.filename.clone(),
                    reference: self.id(self.after),
                })));
            }
            self.stage = Stage::Before;
            if !compare && file.status == d::FileStatus::Added {
                continue;
            }
            let path = if !compare && file.status == d::FileStatus::Renamed {
                file.previous_filename.as_ref().ok_or(api::Error::InvalidAnswer)?.clone()
            } else {
                file.filename.clone()
            };
            self.check_size(bounds)?;
            return Ok(Some(Step::send(r::Request::Raw { at: self.at(), path, reference: self.id(self.before) })));
        }
        Err(api::Error::InvalidAnswer)
    }
    fn raw(&mut self, value: Box<[u8]>, more: bool, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if self.stage == Stage::File {
            let (head, path, max) = match &self.mode {
                Mode::File { head, path, max } => (*head, path.clone(), *max),
                Mode::Pull { .. } | Mode::Compare { .. } => return Err(api::Error::InvalidAnswer),
            };
            let room = max.min(self.raw_room(bounds.answer_bytes));
            let length = value.len().min(usize::try_from(room).expect("bounded raw room"));
            self.add_size(u64::try_from(length).expect("bounded bytes"), bounds)?;
            let truncated = more || value.len() > length;
            let bytes = if value.len() == length {
                value
            } else {
                bytes::copy_of(value.get(..length).expect("bounded length"))
            };
            return Ok(Some(Step::Done(Ok(api::Answer::File { head, path, bytes, truncated }))));
        }
        if more {
            return Err(api::Error::TooLarge);
        }
        self.side(Some(value), bounds)
    }
    fn side(&mut self, value: Option<Box<[u8]>>, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if let Some(value) = &value {
            self.add_size(u64::try_from(value.len()).expect("bounded bytes"), bounds)?;
        }
        let file = self.files.get_mut(self.position).ok_or(api::Error::InvalidAnswer)?;
        match self.stage {
            Stage::Before => file.before = value,
            Stage::After => file.after = value,
            Stage::Pull | Stage::Files | Stage::Compare | Stage::Reverse | Stage::File => {
                return Err(api::Error::InvalidAnswer);
            }
        }
        self.next_side(bounds)
    }
    fn terminal(&mut self) -> Result<Option<Step>, api::Error> {
        let files = core::mem::replace(&mut self.files, List::with_capacity(0)).into_boxed();
        let answer = match self.mode {
            Mode::Pull { head, .. } => api::Answer::PullFiles {
                head,
                more: self.more.unwrap_or(files.len() == usize::try_from(self.limit).expect("page limit")),
                files,
            },
            Mode::Compare { before, after } => api::Answer::Compare {
                before,
                after,
                contains_before: self.contains,
                files,
                commits: core::mem::replace(&mut self.commits, List::with_capacity(0)).into_boxed(),
            },
            Mode::File { .. } => return Err(api::Error::InvalidAnswer),
        };
        Ok(Some(Step::Done(Ok(answer))))
    }
}
