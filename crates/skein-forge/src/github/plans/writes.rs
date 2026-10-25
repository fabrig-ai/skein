//! Single-attempt writes and read prerequisites (forge-github.md, 4.2 and 7).
use super::{
    Body, Failure, Heard, failure as read_failure,
    plan::{Bounds, Exchange, Step},
    tables::{self, Repository, Tables},
};
use crate::{
    api,
    github::{
        document::Document,
        failure::{self, Asked, Drop, Verdict},
        header::Head,
        request::Request,
    },
};
use alloc::boxed::Box;

mod merge;
mod reviewers;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    New,
    Resolve,
    Pull,
    ResolveUser,
    PullGraphql,
    Enqueue,
    Mutate,
    Done,
}
#[derive(Debug)]
enum Response {
    None,
    Head,
    Body,
    Refusal { status: u16, head: Head },
}
#[derive(Debug)]
pub struct Plan {
    repository: api::Repository,
    write: api::Write,
    stage: Stage,
    response: Response,
    row: Option<Repository>,
    pending: Option<Exchange>,
    pinned: Option<api::Commit>,
    reviewers: Option<Box<reviewers::Reviewers>>,
}
impl Plan {
    #[must_use]
    pub fn new(repository: api::Repository, write: api::Write) -> Self {
        Self {
            repository,
            write,
            stage: Stage::New,
            response: Response::None,
            row: None,
            pending: None,
            pinned: None,
            reviewers: None,
        }
    }
    pub fn begin(&mut self, bounds: Bounds, word: &[u8], tables: &mut Tables) -> Result<Step, api::Error> {
        if self.stage != Stage::New {
            return Err(api::Error::InvalidAnswer);
        }
        self.reviewers_check(bounds)?;
        self.row = tables.repository(self.repository).cloned();
        if self.row.is_none() {
            self.stage = Stage::Resolve;
            return Ok(self.send(Request::Repository { id: u64::from(self.repository.repository) }));
        }
        self.initial(bounds, word)
    }
    pub fn pending(&mut self) -> Option<Exchange> {
        self.pending.take()
    }
    pub fn set_pending(&mut self, exchange: Exchange) {
        self.pending = Some(exchange);
    }
    fn send(&mut self, request: Request) -> Step {
        self.response = Response::Head;
        Step::send(request)
    }
    fn initial(&mut self, bounds: Bounds, word: &[u8]) -> Result<Step, api::Error> {
        match self.write {
            api::Write::Update { number } | api::Write::SetReviewers { number, .. } => {
                let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
                let request = Request::Pull { owner: row.owner.clone(), name: row.name.clone(), number };
                self.stage = Stage::Pull;
                Ok(self.send(request))
            }
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::DeleteBranch { .. } => self.mutation(bounds, word),
        }
    }
    fn mutation(&mut self, bounds: Bounds, word: &[u8]) -> Result<Step, api::Error> {
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let owner = row.owner.clone();
        let name = row.name.clone();
        let request = match &self.write {
            api::Write::Update { number } => {
                Request::Update { owner, name, number: *number, head: self.pinned.ok_or(api::Error::InvalidAnswer)? }
            }
            api::Write::Review { number, key, verdict, body } => Request::Review {
                owner,
                name,
                number: *number,
                verdict: *verdict,
                body: marked(word, key, body, bounds)?,
            },
            api::Write::Post { number, key, body } => {
                Request::Post { owner, name, number: *number, body: marked(word, key, body, bounds)? }
            }
            api::Write::CreateIssue { key, title, body } => {
                Request::CreateIssue { owner, name, title: title.clone(), body: marked(word, key, body, bounds)? }
            }
            api::Write::OpenPull { title, body, head, base } => Request::OpenPull {
                owner,
                name,
                title: title.clone(),
                body: body.clone(),
                head: head.clone(),
                base: base.clone(),
            },
            api::Write::CreateBranch { branch, commit } => {
                Request::CreateBranch { owner, name, branch: branch.clone(), commit: *commit }
            }
            api::Write::Status { commit, context, check } => {
                Request::Status { owner, name, commit: *commit, context: context.clone(), check: *check }
            }
            api::Write::Edit { number, title, body } => {
                Request::Edit { owner, name, number: *number, title: title.clone(), body: body.clone() }
            }
            api::Write::Reopen { number } => Request::Reopen { owner, name, number: *number },
            api::Write::Close { number } => Request::Close { owner, name, number: *number },
            api::Write::DeleteBranch { branch } => Request::DeleteBranch { owner, name, branch: branch.clone() },
            api::Write::SetReviewers { number, .. } => {
                return self.reviewers_mutation(*number);
            }
            api::Write::Merge { number, head } => Request::Merge {
                owner,
                name,
                number: *number,
                head: *head,
                method: row.merge.ok_or(api::Error::Refused)?,
            },
        };
        self.stage = Stage::Mutate;
        Ok(self.send(request))
    }
    fn result_kind(&self) -> Option<crate::github::document::Kind> {
        use crate::github::document::Kind;
        match self.write {
            api::Write::Merge { .. } => Some(Kind::Merge),
            api::Write::Review { .. } | api::Write::Post { .. } => Some(Kind::Id),
            api::Write::DeleteBranch { .. } => None,
            api::Write::CreateIssue { .. } | api::Write::OpenPull { .. } => Some(Kind::Number),
            api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. } => Some(Kind::Done),
        }
    }
    fn status(&self) -> u16 {
        if let Some(reviewers) = &self.reviewers {
            return if reviewers.added || reviewers.add.is_empty() { 200 } else { 201 };
        }
        match self.write {
            api::Write::Update { .. } => 202,
            api::Write::DeleteBranch { .. } => 204,
            api::Write::Post { .. }
            | api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. } => 201,
            api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. } => 200,
        }
    }
    fn mutating(&self) -> bool {
        self.stage == Stage::Mutate || self.stage == Stage::Enqueue
    }
    fn asked(&self) -> Asked {
        if self.stage == Stage::Enqueue {
            return Asked::Enqueue;
        }
        match &self.write {
            api::Write::Merge { .. } => Asked::Merge,
            api::Write::Update { .. } => Asked::Update,
            api::Write::Review { body, .. } => Asked::Review { empty: body.is_empty() },
            api::Write::Post { body, .. } => Asked::Write { empty: body.is_empty() },
            api::Write::CreateIssue { title, body, .. } => Asked::Write { empty: title.is_empty() || body.is_empty() },
            api::Write::OpenPull { title, body, .. } => Asked::OpenPull { empty: title.is_empty() || body.is_empty() },
            api::Write::DeleteBranch { .. } => Asked::DeleteBranch,
            api::Write::CreateBranch { .. } => Asked::CreateBranch,
            api::Write::Edit { title, body, .. } => {
                Asked::Write { empty: empty(title.as_deref()) || empty(body.as_deref()) }
            }
            api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Status { .. } => Asked::Write { empty: false },
        }
    }
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
        match (&heard, &self.response) {
            (Heard::Head { .. }, Response::Head)
            | (Heard::Done(_) | Heard::Row(_), Response::Body | Response::Refusal { .. })
            | (Heard::Failed(_), Response::Head | Response::Body | Response::Refusal { .. }) => {}
            (Heard::Head { .. }, Response::None | Response::Body | Response::Refusal { .. })
            | (Heard::Done(_) | Heard::Row(_), Response::None | Response::Head)
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
    fn malformed(&self, error: crate::github::decode::Malformed) -> api::Error {
        if self.mutating() {
            return api::Error::Timeout;
        }
        if read_failure::malformed(error) == api::Error::TooLarge {
            api::Error::TooLarge
        } else {
            api::Error::Unavailable
        }
    }

    fn invalid(&self) -> api::Error {
        if self.mutating() { api::Error::Timeout } else { api::Error::Unavailable }
    }
    fn terminal(&mut self, answer: api::Answer, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let size = crate::measure::answer(&answer).ok_or(self.invalid())?;
        if size > u64::from(bounds.answer_bytes) {
            return Err(self.invalid());
        }
        self.stage = Stage::Done;
        self.response = Response::None;
        Ok(Some(Step::Done(Ok(answer))))
    }
    fn refusal(
        &self,
        status: u16,
        head: &Head,
        body: Option<&crate::github::document::Refusal>,
        tables: &mut Tables,
    ) -> Result<api::Answer, api::Error> {
        let asked = if self.mutating() { self.asked() } else { Asked::Read };
        match failure::classify(asked, status, head, body) {
            Verdict::Fail(error, drop) => {
                match drop {
                    Drop::Nothing => {}
                    Drop::Row => tables.drop_repository(self.repository),
                }
                Err(if self.added() && (error == api::Error::Unavailable || matches_rate(error)) {
                    api::Error::Timeout
                } else if !self.mutating() && error == api::Error::Timeout {
                    api::Error::Unavailable
                } else {
                    error
                })
            }
            Verdict::BranchExists => Ok(api::Answer::Branch(api::BranchCreation::Exists)),
            Verdict::Enqueue => Err(self.invalid()),
        }
    }
    fn refusal_head(&self) -> Option<(u16, &Head)> {
        match &self.response {
            Response::Refusal { status, head } => Some((*status, head)),
            Response::None | Response::Head | Response::Body => None,
        }
    }
    fn advance(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, head } => {
                let expected = if self.mutating() { self.status() } else { 200 };
                if status == expected {
                    self.response = Response::Body;
                } else if (200..300).contains(&status) {
                    return Err(self.invalid());
                } else {
                    self.response = Response::Refusal { status, head };
                }
                Ok(None)
            }
            Heard::Row(_) => match self.response {
                Response::Refusal { .. } => Ok(None),
                Response::None | Response::Head | Response::Body => Err(api::Error::InvalidAnswer),
            },
            Heard::Failed(failed) => {
                if let Some((status, head)) = self.refusal_head() {
                    let answer = self.refusal(status, head, None, tables)?;
                    return self.terminal(answer, bounds);
                }
                Err(if self.mutating() {
                    match failed {
                        Failure::Unseen => {
                            if self.added() {
                                api::Error::Timeout
                            } else {
                                api::Error::Unavailable
                            }
                        }
                        Failure::Seen => api::Error::Timeout,
                    }
                } else {
                    api::Error::Unavailable
                })
            }
            Heard::Done(body) => self.done(body, bounds, word, tables),
        }
    }
    fn update_pull(
        &mut self,
        pull: crate::github::document::Pull,
        bounds: Bounds,
        word: &[u8],
    ) -> Result<Step, api::Error> {
        let number = match self.write {
            api::Write::Update { number } => number,
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::DeleteBranch { .. } => return Err(api::Error::InvalidAnswer),
        };
        if pull.number != number || crate::github::id::write_commit(&pull.head_commit).is_none() {
            return Err(self.invalid());
        }
        if u64::try_from(pull.reviewers.len()).ok().ok_or(api::Error::TooLarge)? > u64::from(bounds.rows) {
            return Err(api::Error::TooLarge);
        }
        let Ok(pull) = super::reading::pull(pull, api::Ci::None) else {
            return Err(self.invalid());
        };
        if crate::measure::pull(&pull).ok_or(api::Error::TooLarge)? > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        self.pinned = Some(pull.commit);
        self.mutation(bounds, word)
    }
    fn identified(&self, id: u64) -> Result<api::Answer, api::Error> {
        match self.write {
            api::Write::Review { .. } => Ok(api::Answer::Reviewed(id)),
            api::Write::Post { .. } => Ok(api::Answer::Commented(id)),
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Edit { .. }
            | api::Write::SetReviewers { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::DeleteBranch { .. } => Err(self.invalid()),
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "write completion exhaustively dispatches prerequisite and mutation documents"
    )]
    fn done(
        &mut self,
        body: Body,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        if let Some((status, head)) = self.refusal_head() {
            let refusal = match &body {
                Body::Document(Document::Error(refusal)) => Some(refusal),
                Body::Document(
                    Document::Delivery(_)
                    | Document::Installation(_)
                    | Document::Minted(_)
                    | Document::Viewer(_)
                    | Document::Merge(_)
                    | Document::Enqueued(_)
                    | Document::PullGraphql(_)
                    | Document::Graphql(_)
                    | Document::App(_)
                    | Document::HookConfig(_)
                    | Document::Conversion(_)
                    | Document::User(_)
                    | Document::Compare(_)
                    | Document::Permission(_)
                    | Document::Id(_)
                    | Document::Number(_)
                    | Document::Done
                    | Document::Ruleset(_)
                    | Document::ClassicProtection(_)
                    | Document::BotUser(_)
                    | Document::Repository(_)
                    | Document::Branch(_)
                    | Document::Issue(_)
                    | Document::Pull(_)
                    | Document::Job(_)
                    | Document::Combined(_)
                    | Document::ActionsPermissions(_)
                    | Document::Workflows { .. }
                    | Document::Rows,
                )
                | Body::Raw { .. }
                | Body::Empty
                | Body::Malformed(_) => None,
            };
            if self.stage == Stage::Mutate
                && self.asked() == Asked::Merge
                && failure::classify(Asked::Merge, status, head, refusal) == Verdict::Enqueue
            {
                return Ok(Some(self.merge_pull()?));
            }
            let answer = self.refusal(status, head, refusal, tables)?;
            return self.terminal(answer, bounds);
        }
        match body {
            Body::Document(Document::PullGraphql(pull)) if self.stage == Stage::PullGraphql => {
                return Ok(Some(self.enqueue(pull)?));
            }
            Body::Document(Document::Graphql(refusal))
                if self.stage == Stage::PullGraphql || self.stage == Stage::Enqueue =>
            {
                return Err(if self.stage == Stage::Enqueue {
                    failure::graphql(&refusal.kind, &refusal.message)
                } else {
                    api::Error::Unavailable
                });
            }
            Body::Document(Document::Enqueued(_)) if self.stage == Stage::Enqueue => {
                return self.terminal(api::Answer::Queued, bounds);
            }
            Body::Document(Document::Merge(commit)) if self.stage == Stage::Mutate && self.asked() == Asked::Merge => {
                if crate::github::id::write_commit(&commit).is_none() {
                    return Err(self.invalid());
                }
                return self.terminal(api::Answer::Merged(commit), bounds);
            }
            Body::Malformed(error) => return Err(self.malformed(error)),
            Body::Document(Document::Repository(repository)) if self.stage == Stage::Resolve => {
                let id = crate::github::id::repository(repository.id)?;
                if id == 0 || id != self.repository.repository {
                    return Err(self.invalid());
                }
                let row = tables::repository(&repository);
                tables.keep_repository(self.repository, row.clone());
                tables.keep_user(self.repository.forge, repository.owner_id, repository.owner);
                self.row = Some(row);
                return Ok(Some(self.initial(bounds, word)?));
            }
            Body::Document(Document::Pull(pull)) if self.stage == Stage::Pull => {
                if self.reviewers_given().is_some() {
                    return self.reviewers_pull(pull, bounds, tables);
                }
                return Ok(Some(self.update_pull(pull, bounds, word)?));
            }
            Body::Document(Document::User(user)) if self.stage == Stage::ResolveUser => {
                return self.reviewers_user(user, bounds, tables);
            }
            Body::Document(Document::Done) if self.stage == Stage::Mutate && self.reviewers.is_some() => {
                return self.reviewers_done(bounds);
            }
            Body::Document(Document::Id(id))
                if self.stage == Stage::Mutate && self.result_kind() == Some(crate::github::document::Kind::Id) =>
            {
                let answer = self.identified(id)?;
                return self.terminal(answer, bounds);
            }
            Body::Document(Document::Number(number))
                if self.stage == Stage::Mutate && self.result_kind() == Some(crate::github::document::Kind::Number) =>
            {
                return self.terminal(api::Answer::Created(number), bounds);
            }
            Body::Empty if self.stage == Stage::Mutate && self.asked() == Asked::DeleteBranch => {}
            Body::Document(Document::Done)
                if self.stage == Stage::Mutate && self.result_kind() == Some(crate::github::document::Kind::Done) => {}
            Body::Document(
                Document::Delivery(_)
                | Document::Installation(_)
                | Document::Minted(_)
                | Document::Viewer(_)
                | Document::Merge(_)
                | Document::Enqueued(_)
                | Document::PullGraphql(_)
                | Document::Graphql(_)
                | Document::App(_)
                | Document::HookConfig(_)
                | Document::Conversion(_)
                | Document::User(_)
                | Document::Compare(_)
                | Document::Permission(_)
                | Document::Id(_)
                | Document::Number(_)
                | Document::Done
                | Document::Ruleset(_)
                | Document::ClassicProtection(_)
                | Document::BotUser(_)
                | Document::Repository(_)
                | Document::Branch(_)
                | Document::Issue(_)
                | Document::Pull(_)
                | Document::Job(_)
                | Document::Combined(_)
                | Document::ActionsPermissions(_)
                | Document::Workflows { .. }
                | Document::Rows
                | Document::Error(_),
            )
            | Body::Empty
            | Body::Raw { .. } => return Err(self.invalid()),
        }
        let answer = if self.asked() == Asked::CreateBranch {
            api::Answer::Branch(api::BranchCreation::Created)
        } else {
            api::Answer::Done
        };
        self.terminal(answer, bounds)
    }
}

fn empty(text: Option<&[u8]>) -> bool {
    match text {
        Some(text) => text.is_empty(),
        None => false,
    }
}

fn marked(word: &[u8], key: &[u8], body: &[u8], bounds: Bounds) -> Result<Box<[u8]>, api::Error> {
    if !crate::marker::valid_word(word) {
        return Err(api::Error::Refused);
    }
    crate::marker::encode(word, key, body, bounds.body_bytes).ok_or(api::Error::TooLarge)
}

fn matches_rate(error: api::Error) -> bool {
    match error {
        api::Error::RateLimited { .. } => true,
        api::Error::Unavailable
        | api::Error::Timeout
        | api::Error::Forbidden
        | api::Error::Missing
        | api::Error::MissingJob
        | api::Error::TooLarge
        | api::Error::Empty
        | api::Error::Full
        | api::Error::Exists
        | api::Error::NothingToMerge
        | api::Error::Closed
        | api::Error::Stale
        | api::Error::Conflict
        | api::Error::Protected
        | api::Error::Refused
        | api::Error::InvalidAnswer
        | api::Error::Busy
        | api::Error::NotConnected
        | api::Error::NotInstalled => false,
    }
}
