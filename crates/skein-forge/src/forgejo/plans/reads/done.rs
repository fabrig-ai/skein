//! Terminal documents and the next exchange of a JSON read.
use super::{Bounds, Plan, Request, Rows, Stage, Step, Tables, api, bytes, d, keep_document, reading};
impl Plan {
    pub(super) fn done(
        &mut self,
        document: d::Document,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        self.check_repository_document(&document)?;
        keep_document(&document, self.repository.forge, tables);
        match document {
            d::Document::Repository(repository) => self.repository_done(repository, bounds, tables),
            d::Document::Issue(issue) => {
                self.item = Some(reading::summary(word, issue));
                self.size = super::size::summary(self.item.as_ref().expect("summary"), bounds.rows)?;
                if self.size > u64::from(bounds.answer_bytes) {
                    return Err(api::Error::TooLarge);
                }
                let room = u64::from(bounds.answer_bytes)
                    .saturating_sub(self.size)
                    .checked_div(u64::try_from(crate::measure::COMMENT_BYTES).expect("comment width"))
                    .expect("nonzero width");
                self.rows = Rows::Comments(skein_lib::List::with_capacity(
                    bounds.rows.min(u32::try_from(room).expect("bounded room")),
                ));
                self.stage = Stage::Comments;
                let number = self.item.as_ref().expect("summary").number;
                Ok(Some(self.send(Request::Comments { at: self.at(), number })))
            }
            d::Document::Pull(pull) => self.pull_done(pull, bounds),
            d::Document::CombinedStatus(combined) => {
                let ci = reading::ci(combined.state, combined.total_count);
                let read = u64::from(self.page.saturating_sub(1))
                    .saturating_mul(u64::from(self.limit))
                    .saturating_add(u64::from(self.count));
                let statuses = match core::mem::replace(&mut self.rows, Rows::None) {
                    Rows::Statuses(rows) => rows.into_boxed(),
                    Rows::None
                    | Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Reviews(_)
                    | Rows::Remarks(_)
                    | Rows::Branches(_)
                    | Rows::Collaborators(_) => return Err(api::Error::InvalidAnswer),
                };
                let answer = api::Answer::Statuses { ci, statuses, more: read < combined.total_count };
                self.terminal(answer, bounds)
            }
            d::Document::Branch(branch) => self.branch_done(branch, bounds),
            d::Document::Protection(protection) => {
                self.terminal(api::Answer::Protection(Some(reading::protection(protection))), bounds)
            }
            d::Document::Permission(permission) => {
                let permission = reading::permission(permission.permission);
                let answer = match &mut self.rows {
                    Rows::Collaborators(rows) => {
                        rows.get_mut(self.position).ok_or(api::Error::InvalidAnswer)?.permission = permission;
                        self.position = self.position.saturating_add(1);
                        return self.next_permission(tables);
                    }
                    Rows::None => api::Answer::Permission(permission),
                    Rows::Items(_)
                    | Rows::Comments(_)
                    | Rows::Reviews(_)
                    | Rows::Statuses(_)
                    | Rows::Remarks(_)
                    | Rows::Branches(_) => return Err(api::Error::InvalidAnswer),
                };
                self.terminal(answer, bounds)
            }
            d::Document::Search { ok } => self.search_done(ok, tables),
            d::Document::Listed { skipped } => self.listed_done(skipped, bounds, tables),
            d::Document::RepoSearch { .. }
            | d::Document::Token(_)
            | d::Document::User(_)
            | d::Document::Org(_)
            | d::Document::CreatedFile { .. }
            | d::Document::CreatedRepository(_)
            | d::Document::Hook(_)
            | d::Document::Team(_)
            | d::Document::ApiSettings(_)
            | d::Document::Comment(_)
            | d::Document::Status(_)
            | d::Document::Review(_)
            | d::Document::Compare(_)
            | d::Document::Tree { .. }
            | d::Document::Runs { .. }
            | d::Document::Job(_)
            | d::Document::Run(_)
            | d::Document::Error(_) => Err(api::Error::InvalidAnswer),
        }
    }
    pub(super) fn terminal(&mut self, answer: api::Answer, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let size = super::size::answer(&answer, bounds.rows)?;
        if size > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        self.size = size;
        Ok(Some(Step::Done(Ok(answer))))
    }
    fn check_repository_document(&self, document: &d::Document) -> Result<(), api::Error> {
        match document {
            d::Document::Repository(repository) => self.check_repository(repository),
            d::Document::Token(_)
            | d::Document::User(_)
            | d::Document::Org(_)
            | d::Document::CreatedFile { .. }
            | d::Document::CreatedRepository(_)
            | d::Document::Hook(_)
            | d::Document::Team(_)
            | d::Document::ApiSettings(_)
            | d::Document::RepoSearch { .. }
            | d::Document::Search { .. }
            | d::Document::Listed { .. }
            | d::Document::Issue(_)
            | d::Document::Comment(_)
            | d::Document::Pull(_)
            | d::Document::Status(_)
            | d::Document::Review(_)
            | d::Document::CombinedStatus(_)
            | d::Document::Branch(_)
            | d::Document::Protection(_)
            | d::Document::Permission(_)
            | d::Document::Compare(_)
            | d::Document::Tree { .. }
            | d::Document::Runs { .. }
            | d::Document::Job(_)
            | d::Document::Run(_)
            | d::Document::Error(_) => Ok(()),
        }
    }
    fn check_repository(&self, repository: &d::Repository) -> Result<(), api::Error> {
        let Ok(id) = u32::try_from(repository.id) else {
            return Err(api::Error::TooLarge);
        };
        let by_id = self.stage == Stage::Resolve || self.read == api::Read::Settings;
        if id == 0 || (by_id && id != self.repository.repository) {
            return Err(api::Error::InvalidAnswer);
        }
        Ok(())
    }
    fn repository_done(
        &mut self,
        repository: d::Repository,
        bounds: Bounds,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        let Ok(id) = u32::try_from(repository.id) else {
            return Err(api::Error::TooLarge);
        };
        if id == 0 {
            return Err(api::Error::InvalidAnswer);
        }
        let key = api::Repository { forge: self.repository.forge, repository: id };
        tables.keep_repository(key, reading::repository(repository.clone()));
        self.row = Some(reading::repository(repository.clone()));
        if self.stage == Stage::Resolve {
            return Ok(Some(self.initial(bounds, tables)?));
        }
        let answer = match &self.read {
            api::Read::Locate { .. } => api::Answer::Located { repository: id },
            api::Read::Settings => api::Answer::Settings(reading::settings(repository)),
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
            | api::Read::File { .. } => return Err(api::Error::InvalidAnswer),
        };
        self.terminal(answer, bounds)
    }
    fn pull_done(&mut self, pull: d::PullRequest, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let closed = pull.state != d::IssueState::Open;
        self.pull = Some(pull);
        let lookup = match &self.read {
            api::Read::PullFor { .. } => true,
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
        if lookup && closed {
            self.stage = Stage::Pulls;
            let page = self.paged(1, bounds);
            return Ok(Some(self.send(Request::Pulls { at: self.at(), page })));
        }
        Ok(Some(self.pull_status(bounds)?))
    }
    fn branch_done(&mut self, branch: d::Branch, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        match &self.read {
            api::Read::Protection { .. } => {
                self.stage = Stage::Protection;
                let request = match branch.effective_branch_protection_name {
                    Some(name) => Request::Protection { at: self.at(), name },
                    None => Request::Protections { at: self.at() },
                };
                Ok(Some(self.send(request)))
            }
            api::Read::Branch { .. } => self.terminal(
                api::Answer::Commit(
                    reading::commit(branch.commit, self.row.as_ref().expect("format").object_format)
                        .ok_or(api::Error::InvalidAnswer)?,
                ),
                bounds,
            ),
            api::Read::Locate { .. }
            | api::Read::Items { .. }
            | api::Read::Item { .. }
            | api::Read::Pull { .. }
            | api::Read::PullFor { .. }
            | api::Read::Reviews { .. }
            | api::Read::Statuses { .. }
            | api::Read::Remarks { .. }
            | api::Read::Branches { .. }
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
            | api::Read::File { .. } => Err(api::Error::InvalidAnswer),
        }
    }
    fn search_done(&mut self, ok: bool, tables: &mut Tables) -> Result<Option<Step>, api::Error> {
        if !ok || self.count == 0 {
            return Err(api::Error::Missing);
        }
        self.stage = Stage::Permissions;
        let user = match &self.rows {
            Rows::Collaborators(rows) => rows.get(self.position).ok_or(api::Error::InvalidAnswer)?.user,
            Rows::None => match self.read {
                api::Read::Permission { user } => user,
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
                | api::Read::PullFiles { .. }
                | api::Read::Compare { .. }
                | api::Read::Checks { .. }
                | api::Read::Runs { .. }
                | api::Read::Run { .. }
                | api::Read::Tree { .. }
                | api::Read::Job { .. }
                | api::Read::File { .. } => return Err(api::Error::InvalidAnswer),
            },
            Rows::Items(_)
            | Rows::Comments(_)
            | Rows::Reviews(_)
            | Rows::Statuses(_)
            | Rows::Remarks(_)
            | Rows::Branches(_) => return Err(api::Error::InvalidAnswer),
        };
        let login = tables.user(self.repository.forge, user).ok_or(api::Error::Missing)?;
        Ok(Some(self.send(Request::Permission { at: self.at(), login: bytes::copy_of(login) })))
    }
    fn listed_done(&mut self, skipped: u32, bounds: Bounds, tables: &mut Tables) -> Result<Option<Step>, api::Error> {
        if self.stage == Stage::Pulls {
            if !self.more && self.count.saturating_add(skipped) >= self.limit {
                let page = self.page.checked_add(1).ok_or(api::Error::TooLarge)?;
                let page = self.paged(page, bounds);
                return Ok(Some(self.send(Request::Pulls { at: self.at(), page })));
            }
            return Ok(Some(self.pull_status(bounds)?));
        }
        if self.stage == Stage::Collaborators {
            self.more = self.count >= self.limit;
            self.stage = Stage::Permissions;
            return self.next_permission(tables);
        }
        let answer = self.finish_rows(skipped)?;
        self.terminal(answer, bounds)
    }
    pub(super) fn pull_checks(&mut self, answer: api::Answer, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let statuses = match answer {
            api::Answer::Checks(statuses) => statuses,
            api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Items { .. }
            | api::Answer::Item { .. }
            | api::Answer::Pull(_)
            | api::Answer::Reviews { .. }
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::Located { .. }
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
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
        };
        let ci = reading::checks_ci(&statuses);
        let answer = api::Answer::Pull(reading::pull(
            self.pull.take().expect("pull before status"),
            self.row.as_ref().expect("format").object_format,
            ci,
        )?);
        self.actions = None;
        self.terminal(answer, bounds)
    }
    fn pull_status(&mut self, bounds: Bounds) -> Result<Step, api::Error> {
        self.stage = Stage::Status;
        let commit = self.pull.as_ref().ok_or(api::Error::InvalidAnswer)?.head.sha.ok_or(api::Error::InvalidAnswer)?;
        if commit.format != self.row.as_ref().expect("resolved format").object_format {
            return Err(api::Error::InvalidAnswer);
        }
        let mut plan = crate::forgejo::plans::actions::Plan::statuses(
            commit.bytes,
            self.row.as_ref().expect("resolved format").clone(),
            bounds,
        )?;
        let step = plan.begin();
        self.actions = Some(plan);
        Ok(step)
    }

    fn next_permission(&mut self, tables: &mut Tables) -> Result<Option<Step>, api::Error> {
        let user = match &self.rows {
            Rows::Collaborators(rows) => rows.get(self.position),
            Rows::None
            | Rows::Items(_)
            | Rows::Comments(_)
            | Rows::Reviews(_)
            | Rows::Statuses(_)
            | Rows::Remarks(_)
            | Rows::Branches(_) => return Err(api::Error::InvalidAnswer),
        };
        if let Some(user) = user {
            let user = user.user;
            return Ok(Some(self.permission(user, tables)));
        }
        Ok(Some(Step::Done(Ok(self.finish_rows(0)?))))
    }
    fn finish_rows(&mut self, skipped: u32) -> Result<api::Answer, api::Error> {
        let more = self.more || self.count.saturating_add(skipped) >= self.limit;
        Ok(match core::mem::replace(&mut self.rows, Rows::None) {
            Rows::Items(rows) => api::Answer::Items { items: rows.into_boxed(), more, now: self.now },
            Rows::Comments(rows) => api::Answer::Item {
                item: self.item.take().ok_or(api::Error::InvalidAnswer)?,
                comments: rows.into_boxed(),
                more: self.more,
            },
            Rows::Reviews(rows) => api::Answer::Reviews { reviews: rows.into_boxed(), more },
            Rows::Remarks(rows) => api::Answer::Remarks { remarks: rows.into_boxed(), more: self.more },
            Rows::Branches(rows) => api::Answer::Branches(rows.into_boxed()),
            Rows::Collaborators(rows) => {
                api::Answer::Collaborators { collaborators: rows.into_boxed(), more: self.more }
            }
            Rows::None if self.stage == Stage::Protection => api::Answer::Protection(None),
            Rows::None | Rows::Statuses(_) => return Err(api::Error::InvalidAnswer),
        })
    }
}
