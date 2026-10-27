//! Forge writes, with validation before resolution and no retry
//! (forge.md, 3.2–3.4).
use crate::api;
use crate::forgejo::plans::{
    Body, Heard,
    failure::{self, Asked, Drop},
    plan::{Bounds, Exchange, Step},
    reading,
    tables::{Repository, Tables},
};
use crate::forgejo::{
    document as d,
    id::ObjectId,
    request::{self as r, Request},
};
use crate::marker;
use alloc::boxed::Box;
use skein_lib::{Duration, List};
#[derive(Debug)]
enum Stage {
    Write,
    Head,
    Base,
    Compare,
    Reviewers,
    User,
    Add,
    Remove,
    Merged,
}
#[derive(Debug)]
pub struct Plan {
    repository: api::Repository,
    write: api::Write,
    row: Option<Repository>,
    resolving: bool,
    stage: Stage,
    head: Option<ObjectId>,
    missing: List<u64>,
    add: List<Box<[u8]>>,
    remove: List<Box<[u8]>>,
    position: u32,
    status: u16,
    retry: Option<Duration>,
    request: Option<Exchange>,
}
impl Plan {
    pub fn pending(&mut self) -> Option<Exchange> {
        self.request.take()
    }
    pub fn set_pending(&mut self, request: Exchange) {
        self.request = Some(request);
    }
    #[must_use]
    pub fn new(repository: api::Repository, write: api::Write) -> Self {
        Self {
            repository,
            write,
            row: None,
            resolving: false,
            stage: Stage::Write,
            head: None,
            missing: List::with_capacity(0),
            add: List::with_capacity(0),
            remove: List::with_capacity(0),
            position: 0,
            status: 0,
            retry: None,
            request: None,
        }
    }
    pub fn begin(&mut self, bounds: Bounds, word: &[u8], tables: &mut Tables) -> Result<Step, api::Error> {
        validate(&self.write)?;
        self.row = tables.repository(self.repository).cloned();
        if self.row.is_none() {
            self.resolving = true;
            return Ok(Step::send(Request::RepositoryById { id: u64::from(self.repository.repository) }));
        }
        self.initial(bounds, word)
    }
    fn at(&self) -> r::Repo {
        let row = self.row.as_ref().expect("resolved repository");
        r::Repo { owner: row.owner.clone(), name: row.name.clone() }
    }
    fn id(&self, bytes: api::Commit) -> ObjectId {
        ObjectId { bytes, format: self.row.as_ref().expect("repository format").object_format }
    }
    fn initial(&mut self, bounds: Bounds, word: &[u8]) -> Result<Step, api::Error> {
        self.stage = Stage::Write;
        let at = self.at();
        let request = match self.write.clone() {
            api::Write::Dispatch { workflow, branch, inputs } => Request::Dispatch { at, workflow, branch, inputs },
            api::Write::Rerun { attempt: job } => Request::Rerun { at, job: job.job },
            api::Write::CreateIssue { key, title, body } => {
                Request::CreateIssue { at, title, body: marked(word, &key, &body, bounds)? }
            }
            api::Write::Post { number, key, body } => {
                Request::PostComment { at, number, body: marked(word, &key, &body, bounds)? }
            }
            api::Write::Review { number, key, verdict, body } => {
                let event = match verdict {
                    api::Verdict::Approve => r::ReviewEvent::Approved,
                    api::Verdict::RequestChanges => r::ReviewEvent::RequestChanges,
                    api::Verdict::Comment => r::ReviewEvent::Comment,
                };
                Request::PostReview { at, number, event, body: marked(word, &key, &body, bounds)? }
            }
            api::Write::Edit { number, title, body } => Request::EditIssue { at, number, title, body, state: None },
            api::Write::Close { number } => {
                Request::EditIssue { at, number, title: None, body: None, state: Some(d::IssueState::Closed) }
            }
            api::Write::Reopen { number } => {
                Request::EditIssue { at, number, title: None, body: None, state: Some(d::IssueState::Open) }
            }
            api::Write::Update { number, style } => Request::Update {
                at,
                number,
                style: Some(match style {
                    api::UpdateStyle::Merge => r::UpdateStyle::Merge,
                    api::UpdateStyle::Rebase => r::UpdateStyle::Rebase,
                }),
            },
            api::Write::Status { commit, context, check, description, link } => {
                let state = match check {
                    api::Check::Pending => d::StatusState::Pending,
                    api::Check::Passed => d::StatusState::Success,
                    api::Check::Failed => d::StatusState::Failure,
                };
                Request::PostStatus { at, commit: self.id(commit), state, context, description, link }
            }
            api::Write::CreateBranch { branch, commit } => Request::CreateBranch { at, branch, from: self.id(commit) },
            api::Write::DeleteBranch { branch } => Request::DeleteBranch { at, branch },
            api::Write::OpenPull { head, .. } => {
                self.stage = Stage::Head;
                Request::Branch { at, name: head }
            }
            api::Write::SetReviewers { number, .. } => {
                self.stage = Stage::Reviewers;
                Request::Pull { at, number }
            }
            api::Write::Merge { number, head, how } => Request::Merge {
                at,
                number,
                style: match how {
                    api::MergeHow::Style => self.row.as_ref().expect("style").merge_style,
                    api::MergeHow::FastForwardOnly => d::MergeStyle::FastForwardOnly,
                },
                head: self.id(head),
            },
        };
        Ok(Step::send(request))
    }
    fn asked(&self) -> Asked {
        match self.write {
            api::Write::OpenPull { .. } => {
                if self.before_write() {
                    Asked::BeforeWrite
                } else {
                    Asked::OpenPull
                }
            }
            api::Write::Merge { .. } => Asked::Merge,
            api::Write::Update { .. } => Asked::Update,
            api::Write::DeleteBranch { .. } => Asked::DeleteBranch,
            api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::CreateIssue { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. } => Asked::Write,
        }
    }
    fn expected(&self) -> u16 {
        if self.resolving {
            return 200;
        }
        match self.stage {
            Stage::Head | Stage::Base | Stage::Compare | Stage::Reviewers | Stage::User | Stage::Merged => return 200,
            Stage::Add => return 201,
            Stage::Remove => return 204,
            Stage::Write => {}
        }
        match self.write {
            api::Write::Update { .. } | api::Write::Merge { .. } | api::Write::Review { .. } => 200,
            api::Write::Dispatch { .. } | api::Write::Rerun { .. } | api::Write::DeleteBranch { .. } => 204,
            api::Write::CreateIssue { .. }
            | api::Write::Post { .. }
            | api::Write::Edit { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::OpenPull { .. } => 201,
            api::Write::SetReviewers { .. } => unreachable!("reviewer exchanges have their own stages"),
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "write exchanges classify refusals before retaining their answer document"
    )]
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, fields } => {
                self.status = status;
                self.retry = fields.retry_after;
                Ok(None)
            }
            Heard::Row(row) => match row {
                d::Row::User(user) => {
                    tables.keep_user(self.repository.forge, user.id, user.login);
                    Ok(None)
                }
                d::Row::Found(_)
                | d::Row::Reachable(_)
                | d::Row::Token(_)
                | d::Row::Issue(_)
                | d::Row::Comment(_)
                | d::Row::Status(_)
                | d::Row::Review(_)
                | d::Row::ReviewComment(_)
                | d::Row::Branch(_)
                | d::Row::Protection(_)
                | d::Row::Pull(_)
                | d::Row::File(_)
                | d::Row::Commit(_)
                | d::Row::Artifact(_)
                | d::Row::CommitMessage(_)
                | d::Row::Entry(_)
                | d::Row::Run(_)
                | d::Row::Hook(_)
                | d::Row::Team(_)
                | d::Row::Step(_)
                | d::Row::Job(_) => Ok(None),
            },
            Heard::Failed(fault) => Err(self.error(failure::transport(fault))),
            Heard::Done(body) => {
                if !self.resolving && self.status == 409 {
                    match self.write {
                        api::Write::CreateBranch { .. } => {
                            return Ok(Some(Step::Done(Ok(api::Answer::Branch(api::BranchCreation::Exists)))));
                        }
                        api::Write::Update { style: api::UpdateStyle::Rebase, .. } => {
                            return Ok(Some(Step::Done(Err(api::Error::Conflict))));
                        }
                        api::Write::CreateIssue { .. }
                        | api::Write::Post { .. }
                        | api::Write::Review { .. }
                        | api::Write::Edit { .. }
                        | api::Write::Close { .. }
                        | api::Write::Reopen { .. }
                        | api::Write::Status { .. }
                        | api::Write::Update { style: api::UpdateStyle::Merge, .. }
                        | api::Write::Dispatch { .. }
                        | api::Write::Rerun { .. }
                        | api::Write::DeleteBranch { .. }
                        | api::Write::OpenPull { .. }
                        | api::Write::SetReviewers { .. }
                        | api::Write::Merge { .. } => {}
                    }
                }
                if self.status != self.expected() {
                    let refusal = match &body {
                        Body::Document(d::Document::Error(error)) => match &error.message {
                            d::Body::Kept(message) => crate::forgejo::failure::refusal(self.status, message),
                            d::Body::Long(_) => None,
                        },
                        Body::Document(_) | Body::Empty | Body::Raw { .. } | Body::Malformed(_) => None,
                    };
                    let classified = failure::classify(self.asked(), self.status, refusal, self.retry);
                    if classified.drop != Drop::Nothing {
                        tables.drop_repository(self.repository);
                    }
                    return Ok(Some(Step::Done(Err(self.error(classified.error)))));
                }
                let document = match body {
                    Body::Document(document) => Some(document),
                    Body::Empty => None,
                    Body::Malformed(crate::forgejo::decode::Malformed::Bound { .. }) => {
                        return Err(self.error(if self.before_write() {
                            api::Error::TooLarge
                        } else {
                            api::Error::Timeout
                        }));
                    }
                    Body::Malformed(_) | Body::Raw { .. } => return Err(self.error(api::Error::Timeout)),
                };
                if let Some(document) = &document {
                    self.check_pull_ids(document)?;
                    self.check_repository(document)?;
                    crate::forgejo::plans::reads::keep::keep_document(document, self.repository.forge, tables);
                }
                if self.resolving {
                    let Some(d::Document::Repository(repository)) = document else {
                        return Err(self.error(api::Error::Timeout));
                    };
                    let row = reading::repository(repository);
                    tables.keep_repository(self.repository, row.clone());
                    self.row = Some(row);
                    self.resolving = false;
                    return Ok(Some(self.initial(bounds, word)?));
                }
                let step = match self.next(document, tables) {
                    Ok(step) => step,
                    Err(error) => return Err(self.error(error)),
                };
                Ok(Some(step))
            }
        }
    }
    fn check_repository(&self, document: &d::Document) -> Result<(), api::Error> {
        if !self.resolving {
            return Ok(());
        }
        let repository = match document {
            d::Document::Repository(repository) => repository,
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
            | d::Document::Error(_) => return Err(self.error(api::Error::Timeout)),
        };
        let Ok(id) = u32::try_from(repository.id) else {
            return Err(api::Error::TooLarge);
        };
        if id != self.repository.repository {
            return Err(self.error(api::Error::Timeout));
        }
        Ok(())
    }
    fn before_write(&self) -> bool {
        match &self.write {
            api::Write::OpenPull { .. } => {
                self.resolving
                    || match self.stage {
                        Stage::Head | Stage::Base | Stage::Compare => true,
                        Stage::Write | Stage::Reviewers | Stage::User | Stage::Add | Stage::Remove | Stage::Merged => {
                            false
                        }
                    }
            }
            api::Write::CreateIssue { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. } => false,
        }
    }
    fn check_pull_ids(&self, document: &d::Document) -> Result<(), api::Error> {
        match document {
            d::Document::Pull(pull) => {
                let format = self.row.as_ref().expect("resolved pull").object_format;
                for id in [pull.head.sha, pull.base.sha, pull.merge_base, pull.merge_commit_sha].into_iter().flatten() {
                    if reading::commit(id, format).is_none() {
                        return Err(self.error(api::Error::Timeout));
                    }
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
            | d::Document::Listed { .. }
            | d::Document::Issue(_)
            | d::Document::Comment(_)
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
            | d::Document::Error(_) => {}
        }
        Ok(())
    }
    #[must_use]
    pub fn error(&self, error: api::Error) -> api::Error {
        match self.stage {
            Stage::Merged => api::Error::Timeout,
            Stage::Write
            | Stage::Head
            | Stage::Base
            | Stage::Compare
            | Stage::Reviewers
            | Stage::User
            | Stage::Add
            | Stage::Remove => failure::before_write(self.asked(), error),
        }
    }
    fn next(&mut self, document: Option<d::Document>, tables: &mut Tables) -> Result<Step, api::Error> {
        match self.stage {
            Stage::Write => match self.write {
                api::Write::Merge { number, .. } => {
                    if document.is_some() {
                        return Err(api::Error::Timeout);
                    }
                    self.stage = Stage::Merged;
                    Ok(Step::send(Request::Pull { at: self.at(), number }))
                }
                api::Write::CreateIssue { .. }
                | api::Write::OpenPull { .. }
                | api::Write::Post { .. }
                | api::Write::Review { .. }
                | api::Write::Edit { .. }
                | api::Write::SetReviewers { .. }
                | api::Write::Close { .. }
                | api::Write::Reopen { .. }
                | api::Write::Update { .. }
                | api::Write::Status { .. }
                | api::Write::CreateBranch { .. }
                | api::Write::Dispatch { .. }
                | api::Write::Rerun { .. }
                | api::Write::DeleteBranch { .. } => Ok(Step::Done(Ok(self.answer(document)?))),
            },
            Stage::Head | Stage::Base => self.branch(document),
            Stage::Compare => {
                let Some(d::Document::Compare(compare)) = document else {
                    return Err(api::Error::InvalidAnswer);
                };
                if compare.total_commits == 0 {
                    return Ok(Step::Done(Err(api::Error::NothingToMerge)));
                }
                let request = match &self.write {
                    api::Write::OpenPull { title, body, head, base } => Request::OpenPull {
                        at: self.at(),
                        title: title.clone(),
                        body: body.clone(),
                        head: head.clone(),
                        base: base.clone(),
                    },
                    api::Write::CreateIssue { .. }
                    | api::Write::Post { .. }
                    | api::Write::Review { .. }
                    | api::Write::Edit { .. }
                    | api::Write::SetReviewers { .. }
                    | api::Write::Close { .. }
                    | api::Write::Reopen { .. }
                    | api::Write::Merge { .. }
                    | api::Write::Update { .. }
                    | api::Write::Status { .. }
                    | api::Write::CreateBranch { .. }
                    | api::Write::Dispatch { .. }
                    | api::Write::Rerun { .. }
                    | api::Write::DeleteBranch { .. } => unreachable!("opening's comparison"),
                };
                self.stage = Stage::Write;
                Ok(Step::send(request))
            }
            Stage::Reviewers => {
                let Some(d::Document::Pull(pull)) = document else {
                    return Err(api::Error::Timeout);
                };
                Ok(self.reviewers(pull, tables))
            }
            Stage::User => {
                let Some(d::Document::Search { ok: true }) = document else {
                    return Err(api::Error::Missing);
                };
                let id = *self.missing.get(self.position).expect("looked-up reviewer");
                let Some(login) = tables.user(self.repository.forge, id) else {
                    return Err(api::Error::Missing);
                };
                self.add.push(Box::from(login)).expect("missing count");
                self.position = self.position.saturating_add(1);
                Ok(self.lookup(tables))
            }
            Stage::Add => {
                self.stage = Stage::Remove;
                Ok(self.reviewer_write())
            }
            Stage::Remove => Ok(Step::Done(Ok(api::Answer::Done))),
            Stage::Merged => {
                let Some(d::Document::Pull(pull)) = document else {
                    return Err(api::Error::Timeout);
                };
                let Some(commit) = pull.merge_commit_sha else {
                    return Err(api::Error::Timeout);
                };
                let Some(commit) = reading::commit(commit, self.row.as_ref().expect("format").object_format) else {
                    return Err(api::Error::Timeout);
                };
                Ok(Step::Done(Ok(api::Answer::Merged(commit))))
            }
        }
    }
    fn reviewers(&mut self, pull: d::PullRequest, tables: &mut Tables) -> Step {
        let reviewers = match &self.write {
            api::Write::SetReviewers { reviewers, .. } => reviewers,
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. } => unreachable!("reviewer plan"),
        };
        let capacity = u32::try_from(reviewers.len()).expect("bounded op");
        self.missing = List::with_capacity(capacity);
        self.add = List::with_capacity(capacity);
        self.remove = List::with_capacity(u32::try_from(pull.requested_reviewers.len()).expect("bounded inner"));
        for id in reviewers {
            let mut found = false;
            for user in &pull.requested_reviewers {
                found |= user.id == *id;
            }
            if !found && !self.missing.as_slice().contains(id) {
                self.missing.push(*id).expect("desired count");
            }
        }
        for user in pull.requested_reviewers {
            if !reviewers.contains(&user.id) {
                self.remove.push(user.login).expect("current count");
            }
        }
        self.lookup(tables)
    }
    fn branch(&mut self, document: Option<d::Document>) -> Result<Step, api::Error> {
        let Some(d::Document::Branch(branch)) = document else {
            return Err(api::Error::InvalidAnswer);
        };
        if reading::commit(branch.commit, self.row.as_ref().expect("format").object_format).is_none() {
            return Err(api::Error::InvalidAnswer);
        }
        let base = match &self.write {
            api::Write::OpenPull { base, .. } => base.clone(),
            api::Write::CreateIssue { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. } => unreachable!("opening's branches"),
        };
        let request = match self.stage {
            Stage::Head => {
                self.head = Some(branch.commit);
                self.stage = Stage::Base;
                Request::Branch { at: self.at(), name: base }
            }
            Stage::Base => {
                self.stage = Stage::Compare;
                let after = self.head.expect("head branch");
                Request::Compare { at: self.at(), before: branch.commit, after, files: false }
            }
            Stage::Write
            | Stage::Compare
            | Stage::Reviewers
            | Stage::User
            | Stage::Add
            | Stage::Remove
            | Stage::Merged => unreachable!("branch stages"),
        };
        Ok(Step::send(request))
    }
    fn lookup(&mut self, tables: &mut Tables) -> Step {
        for _ in self.position..self.missing.len() {
            let id = *self.missing.get(self.position).expect("missing reviewer");
            let Some(login) = tables.user(self.repository.forge, id) else {
                self.stage = Stage::User;
                return Step::send(Request::UserSearch { uid: id, limit: Some(1) });
            };
            self.add.push(Box::from(login)).expect("missing count");
            self.position = self.position.saturating_add(1);
        }
        self.stage = Stage::Add;
        self.reviewer_write()
    }
    fn reviewer_write(&mut self) -> Step {
        let number = match self.write {
            api::Write::SetReviewers { number, .. } => number,
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. } => unreachable!("reviewer plan"),
        };
        match self.stage {
            Stage::Add => {
                if !self.add.is_empty() {
                    let reviewers = core::mem::replace(&mut self.add, List::with_capacity(0)).into_boxed();
                    return Step::send(Request::RequestReviewers { at: self.at(), number, reviewers });
                }
                self.stage = Stage::Remove;
            }
            Stage::Remove => {}
            Stage::Write
            | Stage::Head
            | Stage::Base
            | Stage::Compare
            | Stage::Reviewers
            | Stage::User
            | Stage::Merged => unreachable!("reviewer writes"),
        }
        if self.remove.is_empty() {
            return Step::Done(Ok(api::Answer::Done));
        }
        let reviewers = core::mem::replace(&mut self.remove, List::with_capacity(0)).into_boxed();
        Step::send(Request::RemoveReviewers { at: self.at(), number, reviewers })
    }
    fn answer(&self, document: Option<d::Document>) -> Result<api::Answer, api::Error> {
        match &self.write {
            api::Write::CreateIssue { .. } => match document {
                Some(d::Document::Issue(issue)) => Ok(api::Answer::Created(issue.number)),
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::Post { .. } => match document {
                Some(d::Document::Comment(comment)) => Ok(api::Answer::Commented(comment.id)),
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::Review { .. } => match document {
                Some(d::Document::Review(review)) => {
                    if reading::commit(review.commit_id, self.row.as_ref().expect("format").object_format).is_none() {
                        return Err(api::Error::Timeout);
                    }
                    Ok(api::Answer::Reviewed(review.id))
                }
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::Edit { .. } | api::Write::Close { .. } | api::Write::Reopen { .. } => match document {
                Some(d::Document::Issue(_)) => Ok(api::Answer::Done),
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::Status { .. } => match document {
                Some(d::Document::Status(_)) => Ok(api::Answer::Done),
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::CreateBranch { .. } => match document {
                Some(d::Document::Branch(branch)) => {
                    if reading::commit(branch.commit, self.row.as_ref().expect("format").object_format).is_none() {
                        return Err(api::Error::Timeout);
                    }
                    Ok(api::Answer::Branch(api::BranchCreation::Created))
                }
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::Update { .. }
            | api::Write::DeleteBranch { .. } => match document {
                None => Ok(api::Answer::Done),
                Some(_) => Err(api::Error::Timeout),
            },
            api::Write::OpenPull { .. } => match document {
                Some(d::Document::Pull(pull)) => Ok(api::Answer::Created(pull.number)),
                Some(_) | None => Err(api::Error::Timeout),
            },
            api::Write::SetReviewers { .. } | api::Write::Merge { .. } => Err(api::Error::Timeout),
        }
    }
}
fn marked(word: &[u8], key: &[u8], body: &[u8], bounds: Bounds) -> Result<Box<[u8]>, api::Error> {
    marker::encode(word, key, body, bounds.body_bytes).ok_or(api::Error::Refused)
}
fn text(bytes: &[u8]) -> Result<(), api::Error> {
    let l = skein_json::writer::Limits { length: u32::MAX, depth: 1 };
    let mut json = skein_json::writer::Encoder::measure(&l);
    json.string(bytes);
    match json.measured() {
        Ok(_) => Ok(()),
        Err(_) => Err(api::Error::Refused),
    }
}
fn validate(write: &api::Write) -> Result<(), api::Error> {
    match write {
        api::Write::CreateIssue { title, body, .. } => {
            text(title)?;
            text(body)
        }
        api::Write::OpenPull { title, body, head, base } => {
            text(title)?;
            text(body)?;
            text(head)?;
            text(base)
        }
        api::Write::Post { body, .. } | api::Write::Review { body, .. } => text(body),
        api::Write::Edit { title, body, .. } => {
            if let Some(title) = title {
                text(title)?;
            }
            if let Some(body) = body {
                text(body)?;
            }
            Ok(())
        }
        api::Write::Status { context, description, link, .. } => {
            text(context)?;
            text(description)?;
            text(link)
        }
        api::Write::Dispatch { workflow, branch, inputs } => {
            text(workflow)?;
            text(branch)?;
            for input in inputs {
                text(&input.name)?;
                text(&input.value)?;
            }
            Ok(())
        }
        api::Write::CreateBranch { branch, .. } | api::Write::DeleteBranch { branch } => text(branch),
        api::Write::Rerun { .. }
        | api::Write::SetReviewers { .. }
        | api::Write::Close { .. }
        | api::Write::Reopen { .. }
        | api::Write::Merge { .. }
        | api::Write::Update { .. } => Ok(()),
    }
}
