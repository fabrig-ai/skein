//! One tokenizer event in, one GitHub decoder step out.
use super::{
    document::{self, Document, Kind, Row},
    limits::Limits,
    request::Request,
};
pub use crate::forgejo::decode::Malformed;
use alloc::boxed::Box;
use skein_json::{Token, tokenizer};
use skein_lib::List;
mod app;
mod checks;
pub(crate) mod delivery;
mod files;
mod issues;
mod log;
mod people;
mod protection;
mod pull_graphql;
mod pulls;
mod reviews;
mod walk;
use walk::{Move, Node, Walk};
/// The result of one event, with its next demand returned beside it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step {
    Demand,
    Row(Row),
    Done(Document),
    Malformed(Malformed),
}
/// Retains only the selected document and one streaming row.
#[derive(Debug)]
pub struct Decoder {
    walk: Walk,
    kind: Kind,
    app: app::State,
    log: log::State,
    issues: issues::State,
    pulls: pulls::State,
    pull_graphql: pull_graphql::State,
    checks: checks::State,
    user: document::User,
    permission: crate::api::Permission,
    collaborator: document::Collaborator,
    reviews: reviews::State,
    files: files::State,
    protection: protection::State,
    repository: document::Repository,
    branch: document::Branch,
    branch_long: bool,
    message: Box<[u8]>,
    errors: List<Box<[u8]>>,
    error: Box<[u8]>,
    terminal: bool,
    result: u64,
    actions: bool,
    workflow_total: u64,
    workflow_count: u32,
}
/// Starts a decoder; its first tokenizer demand is Next.
/// Whole-delivery reads use `log_delivery_decoder` with their listed event.
#[must_use]
pub fn decoder(kind: Kind, limits: &Limits) -> Decoder {
    Decoder {
        walk: Walk::new(kind, limits),
        kind,
        app: app::State::new(kind, limits.inner),
        log: log::State::new(limits),
        issues: issues::State::new(limits.inner),
        pulls: pulls::State::new(limits.inner),
        pull_graphql: pull_graphql::State::new(limits.inner),
        checks: checks::State::new(),
        user: document::User { id: 0, login: Box::from([]) },
        permission: crate::api::Permission::None,
        collaborator: people::collaborator(),
        reviews: reviews::State::new(),
        files: files::State::new(limits.inner),
        protection: protection::State::new(limits.inner),
        repository: document::Repository {
            id: 0,
            owner: Box::from([]),
            owner_id: 0,
            organization: false,
            name: Box::from([]),
            default_branch: Box::from([]),
            html_url: Box::from([]),
            merge: false,
            squash: false,
            rebase: false,
        },
        branch: document::Branch { name: Box::from([]), commit: [0; 32] },
        branch_long: false,
        message: Box::from([]),
        errors: List::with_capacity(limits.inner),
        error: Box::from([]),
        terminal: false,
        result: 0,
        actions: false,
        workflow_total: 0,
        workflow_count: 0,
    }
}
/// Starts a whole-delivery read using its listed row's bounded event word.
#[must_use]
pub fn log_delivery_decoder(event: &[u8], limits: &Limits) -> Decoder {
    let mut result = decoder(Kind::LogDelivery, limits);
    result.log.expected(skein_lib::bytes::copy_of(event));
    result
}
/// Starts a comparison decoder that requires only the merge base.
#[must_use]
pub fn merge_base_decoder(limits: &Limits) -> Decoder {
    let mut result = decoder(Kind::Compare, limits);
    result.walk = Walk::merge_base(limits);
    result
}
/// The document a request's 2xx answer decodes as, or `None` when it is no
/// document: a raw body, an empty answer, or a read whose decoder is not built.
/// An answer at 300 or above decodes as `Kind::Error`, whatever the request.
#[must_use]
pub fn kind(request: &Request) -> Option<Kind> {
    Some(match request {
        Request::ActionsPermissions { .. } => Kind::ActionsPermissions,
        Request::Workflows { .. } => Kind::Workflows,
        Request::LogPage { .. } => Kind::LogRows,
        Request::LogDelivery { .. } => Kind::LogDelivery,
        Request::Installations { .. } => Kind::InstallationRows,
        Request::Installation { .. } => Kind::Installation,
        Request::Mint { .. } => Kind::Minted,
        Request::Reachable { .. } => Kind::ReachableRows,
        Request::Merge { .. } => Kind::Merge,
        Request::Enqueue { .. } => Kind::Enqueue,
        Request::PullGraphql { .. } => Kind::PullGraphql,
        Request::Viewer => Kind::Viewer,
        Request::App => Kind::App,
        Request::HookConfig | Request::SetHook { .. } => Kind::HookConfig,
        Request::Conversion { .. } => Kind::Conversion,
        Request::BotUser { .. } => Kind::BotUser,
        Request::Repository { .. } | Request::Locate { .. } => Kind::Repository,
        Request::Issues { .. } => Kind::IssueRows,
        Request::Issue { .. } => Kind::Issue,
        Request::Comments { .. } => Kind::CommentRows,
        Request::Pull { .. } => Kind::Pull,
        Request::PullsFor { .. } => Kind::PullRows,
        Request::CombinedStatus { .. } => Kind::CombinedStatus,
        Request::CheckRuns { .. } => Kind::CheckRuns,
        Request::Job { .. } => Kind::Job,
        Request::PullFiles { .. } => Kind::FileRows,
        Request::Compare { .. } => Kind::Compare,
        Request::Reviews { .. } => Kind::ReviewRows,
        Request::ReviewComments { .. } => Kind::RemarkRows,
        Request::Collaborators { .. } => Kind::CollaboratorRows,
        Request::User { .. } => Kind::User,
        Request::Permission { .. } => Kind::Permission,
        Request::RulesForBranch { .. } => Kind::RuleRows,
        Request::Ruleset { .. } => Kind::Ruleset,
        Request::ClassicProtection { .. } => Kind::ClassicProtection,
        Request::Reviewers { .. }
        | Request::Update { .. }
        | Request::CreateBranch { .. }
        | Request::Status { .. }
        | Request::Edit { .. }
        | Request::Close { .. }
        | Request::Reopen { .. } => Kind::Done,
        Request::Review { .. } | Request::Post { .. } => Kind::Id,
        Request::CreateIssue { .. } | Request::OpenPull { .. } => Kind::Number,
        Request::Branch { .. } => Kind::Branch,
        Request::Branches { .. } => Kind::BranchRows,
        // Raw bodies, and an uninstall's empty answer.
        Request::Raw { .. }
        | Request::JobLog { .. }
        | Request::Followed { .. }
        | Request::DeleteBranch { .. }
        | Request::Uninstall { .. } => {
            return None;
        }
    })
}
impl Decoder {
    /// Consumes one tokenizer event and returns the next demand and progress.
    pub fn step(&mut self, event: tokenizer::Event) -> (tokenizer::Request, Step) {
        if self.terminal {
            return (tokenizer::Request::Close, Step::Malformed(Malformed::Shape { field: b"" }));
        }
        let result = if self.log.active() {
            match self.log.payload(event) {
                Ok(demand) => Ok((demand, Step::Demand)),
                Err(error) => Err(error),
            }
        } else {
            match self.walk.step(event) {
                Ok(movement) => self.movement(movement),
                Err(error) => Err(error),
            }
        };
        match result {
            Ok((demand, step)) => (demand, step),
            Err(error) => {
                let error = if self.kind == Kind::LogRows || self.kind == Kind::LogDelivery {
                    match error {
                        Malformed::Json(error) => log::json_error(error),
                        Malformed::TooLong { field } => Malformed::Bound { field },
                        Malformed::Bound { .. }
                        | Malformed::Shape { .. }
                        | Malformed::Missing { .. }
                        | Malformed::Range { .. }
                        | Malformed::Id { .. }
                        | Malformed::Time { .. }
                        | Malformed::Value { .. } => error,
                    }
                } else {
                    error
                };
                self.terminal = true;
                (tokenizer::Request::Close, Step::Malformed(error))
            }
        }
    }
    #[expect(clippy::too_many_lines, reason = "the dispatcher exhaustively routes each document node at its end")]
    fn movement(&mut self, movement: Move) -> Result<(tokenizer::Request, Step), Malformed> {
        match movement {
            Move::PayloadStart(token) => return Ok((self.log.start(token)?, Step::Demand)),
            Move::End(Node::Workflow) => {
                self.workflow_count =
                    self.workflow_count.checked_add(1).ok_or(Malformed::Bound { field: b"workflows" })?;
                if self.workflow_count > 1 {
                    return Err(Malformed::Bound { field: b"workflows" });
                }
            }
            Move::End(Node::JobStep) => self.checks.step_end(),
            Move::End(Node::LogRow) => return Ok((tokenizer::Request::Next, Step::Row(Row::Log(self.log.row())))),
            Move::End(Node::PullGraphql(node)) => self.pull_graphql.end(node)?,
            Move::Demand(demand) => return Ok((demand, Step::Demand)),
            Move::Scalar(node, key, token) => self.scalar(node, key, token)?,
            Move::End(
                node @ (Node::Installation
                | Node::InstallationAccount
                | Node::InstallationRows
                | Node::Minted
                | Node::MintPermissions
                | Node::ReachableEnvelope
                | Node::ReachableRows
                | Node::Reachable
                | Node::AppOwner
                | Node::ViewerEnvelope
                | Node::ViewerData
                | Node::Viewer
                | Node::GraphqlErrors
                | Node::GraphqlError
                | Node::App
                | Node::AppPermissions
                | Node::AppEvents
                | Node::Hook
                | Node::Conversion
                | Node::BotUser),
            ) => {
                if let Some(row) = self.app.end(node)? {
                    return Ok((tokenizer::Request::Next, Step::Row(row)));
                }
            }
            Move::End(
                node @ (Node::IssueRows
                | Node::Issue
                | Node::IssueUser
                | Node::Labels
                | Node::Label
                | Node::PullRequest
                | Node::CommentRows
                | Node::Comment
                | Node::CommentUser),
            ) => {
                if let Some(row) = self.issues.end(node, self.kind)? {
                    return Ok((tokenizer::Request::Next, Step::Row(row)));
                }
            }
            Move::End(
                node @ (Node::PullRows
                | Node::PullObject
                | Node::PullRef { .. }
                | Node::Reviewers
                | Node::Reviewer
                | Node::CombinedObject
                | Node::Contexts
                | Node::Context
                | Node::RunsObject
                | Node::Runs),
            ) => {
                if let Some(row) = self.pulls.end(node, self.kind)? {
                    return Ok((tokenizer::Request::Next, Step::Row(row)));
                }
            }
            Move::End(
                node @ (Node::ReviewRows
                | Node::Review
                | Node::ReviewUser
                | Node::RemarkRows
                | Node::Remark
                | Node::RemarkUser),
            ) => {
                if let Some(row) = self.reviews.end(node) {
                    return Ok((tokenizer::Request::Next, Step::Row(row)));
                }
            }
            Move::BodyLong(Some(Node::BranchName), _) => self.branch_long = true,
            Move::BodyLong(Some(node), length) => {
                self.issues.long(node, length);
                self.reviews.long(node, length);
            }
            Move::End(
                node @ (Node::Compare
                | Node::MergeBaseOnly
                | Node::FileRows
                | Node::FileRow
                | Node::CompareCommits
                | Node::CompareCommit
                | Node::CompareFiles
                | Node::CompareFile
                | Node::MergeBase),
            ) => {
                if let Some(row) = self.files.end(node)? {
                    return Ok((tokenizer::Request::Next, Step::Row(row)));
                }
            }
            Move::End(Node::Protection(node)) => {
                if let Some(row) = self.protection.end(node)? {
                    return Ok((tokenizer::Request::Next, Step::Row(row)));
                }
            }
            Move::End(Node::RunItem) => {
                let row = self.checks.end()?;
                return Ok((tokenizer::Request::Next, Step::Row(Row::Run(row))));
            }
            Move::End(Node::Collaborator) => {
                let row = core::mem::replace(&mut self.collaborator, people::collaborator());
                return Ok((tokenizer::Request::Next, Step::Row(Row::Collaborator(row))));
            }
            Move::End(Node::BranchName) => {
                let name = core::mem::replace(&mut self.branch.name, Box::from([]));
                let row = if core::mem::replace(&mut self.branch_long, false) {
                    Row::SkippedBranch
                } else {
                    Row::Branch(name)
                };
                return Ok((tokenizer::Request::Next, Step::Row(row)));
            }
            Move::End(Node::ErrorEntry) => {
                let error = core::mem::replace(&mut self.error, Box::from([]));
                if self.errors.push(error).is_err() {
                    return Err(Malformed::Bound { field: b"errors" });
                }
            }
            Move::End(
                Node::ActionsPermissions
                | Node::Workflows
                | Node::WorkflowRows
                | Node::LogRows
                | Node::LogDelivery
                | Node::LogRequest
                | Node::LogHeaders
                | Node::RunOutput
                | Node::RunApp
                | Node::Job
                | Node::JobSteps
                | Node::CollaboratorRows
                | Node::User
                | Node::Permission
                | Node::IdResult
                | Node::NumberResult
                | Node::Done
                | Node::Repository
                | Node::Owner
                | Node::Branch
                | Node::Commit
                | Node::BranchRows
                | Node::Error
                | Node::Errors,
            ) => {}
            Move::PayloadScalar(..) | Move::PayloadEnd | Move::BodyLong(None, _) => {
                unreachable!("document walker root")
            }
            Move::Done => {
                self.terminal = true;
                let document = match self.kind {
                    Kind::ActionsPermissions => Document::ActionsPermissions(self.actions),
                    Kind::Workflows => Document::Workflows { total: self.workflow_total, count: self.workflow_count },
                    Kind::LogDelivery => Document::Delivery(self.log.document()?),
                    Kind::Installation
                    | Kind::InstallationRows
                    | Kind::Minted
                    | Kind::ReachableRows
                    | Kind::Viewer
                    | Kind::App
                    | Kind::HookConfig
                    | Kind::Conversion
                    | Kind::BotUser => self.app.document()?,
                    Kind::PullGraphql => match self.app.graphql() {
                        Some(refusal) => Document::Graphql(refusal.clone()),
                        None => self.pull_graphql.document()?,
                    },
                    Kind::Merge => Document::Merge(self.branch.commit),
                    Kind::Enqueue => match self.app.graphql() {
                        Some(refusal) => Document::Graphql(refusal.clone()),
                        None => self.pull_graphql.enqueued()?,
                    },
                    Kind::Ruleset => Document::Ruleset(self.protection.ruleset()),
                    Kind::ClassicProtection => Document::ClassicProtection(self.protection.classic()),
                    Kind::Job => Document::Job(self.checks.job()),
                    Kind::Compare => Document::Compare(self.files.document()),
                    Kind::User => Document::User(self.user.clone()),
                    Kind::Permission => Document::Permission(self.permission),
                    Kind::Id => Document::Id(self.result),
                    Kind::Number => Document::Number(self.result),
                    Kind::Done => Document::Done,
                    Kind::Pull => Document::Pull(self.pulls.pull()),
                    Kind::CombinedStatus => Document::Combined(self.pulls.combined()),
                    Kind::Repository => Document::Repository(self.repository.clone()),
                    Kind::Branch => Document::Branch(self.branch.clone()),
                    Kind::RuleRows
                    | Kind::LogRows
                    | Kind::FileRows
                    | Kind::ReviewRows
                    | Kind::RemarkRows
                    | Kind::CollaboratorRows
                    | Kind::PullRows
                    | Kind::CheckRuns
                    | Kind::BranchRows
                    | Kind::IssueRows
                    | Kind::CommentRows => Document::Rows,
                    Kind::Issue => Document::Issue(self.issues.document()),
                    Kind::Error => Document::Error(document::Refusal {
                        message: core::mem::replace(&mut self.message, Box::from([])),
                        errors: core::mem::replace(&mut self.errors, List::with_capacity(0)).into_boxed(),
                    }),
                };
                return Ok((tokenizer::Request::Close, Step::Done(document)));
            }
        }
        Ok((self.walk.demand(), Step::Demand))
    }
    #[expect(clippy::too_many_lines, reason = "the dispatcher exhaustively routes every document field")]
    fn scalar(&mut self, node: Node, key: &'static [u8], token: Token) -> Result<(), Malformed> {
        match (node, key, token) {
            (Node::ActionsPermissions, b"enabled", token) => self.actions = token == Token::True,
            (Node::Workflows, b"total_count", Token::Number(text)) => {
                self.workflow_total = crate::forgejo::id::unsigned(&text).ok_or(Malformed::Range { field: key })?;
            }
            (Node::Workflow, b"id", Token::Number(text)) => {
                if crate::forgejo::id::unsigned(&text).ok_or(Malformed::Range { field: key })? == 0 {
                    return Err(Malformed::Value { field: key });
                }
            }
            (Node::Workflow, b"name" | b"path" | b"state", Token::String(text)) => {
                if text.is_empty() {
                    return Err(Malformed::Value { field: key });
                }
            }

            (Node::Protection(node), key, token) => return self.protection.scalar(node, key, token),
            (
                node @ (Node::LogRows | Node::LogRow | Node::LogDelivery | Node::LogRequest | Node::LogHeaders),
                key,
                token,
            ) => return self.log.scalar(node, key, token),
            (Node::PullGraphql(node), key, token) => return self.pull_graphql.scalar(node, key, token),
            (node @ (Node::RunItem | Node::RunOutput | Node::RunApp | Node::Job | Node::JobStep), key, token) => {
                return self.checks.scalar(node, key, token);
            }
            (Node::IdResult, b"id", Token::Number(text)) | (Node::NumberResult, b"number", Token::Number(text)) => {
                self.result = crate::forgejo::id::unsigned(&text).ok_or(Malformed::Range { field: key })?;
            }
            (
                node @ (Node::Compare
                | Node::MergeBaseOnly
                | Node::FileRows
                | Node::FileRow
                | Node::CompareCommits
                | Node::CompareCommit
                | Node::CompareFiles
                | Node::CompareFile
                | Node::MergeBase),
                key,
                token,
            ) => return self.files.scalar(node, key, token),
            (
                node @ (Node::ReviewRows
                | Node::Review
                | Node::ReviewUser
                | Node::RemarkRows
                | Node::Remark
                | Node::RemarkUser),
                key,
                token,
            ) => {
                return self.reviews.scalar(node, key, token);
            }
            (
                node @ (Node::PullRows
                | Node::PullObject
                | Node::PullRef { .. }
                | Node::Reviewers
                | Node::Reviewer
                | Node::CombinedObject
                | Node::Contexts
                | Node::Context
                | Node::RunsObject
                | Node::Runs),
                key,
                token,
            ) => return self.pulls.scalar(node, key, token),
            (
                node @ (Node::IssueRows
                | Node::Issue
                | Node::IssueUser
                | Node::Labels
                | Node::Label
                | Node::PullRequest
                | Node::CommentRows
                | Node::Comment
                | Node::CommentUser),
                key,
                token,
            ) => return self.issues.scalar(node, key, token),
            (
                node @ (Node::Installation
                | Node::InstallationAccount
                | Node::InstallationRows
                | Node::Minted
                | Node::MintPermissions
                | Node::ReachableEnvelope
                | Node::ReachableRows
                | Node::Reachable
                | Node::AppOwner
                | Node::ViewerEnvelope
                | Node::ViewerData
                | Node::Viewer
                | Node::GraphqlErrors
                | Node::GraphqlError
                | Node::App
                | Node::AppPermissions
                | Node::AppEvents
                | Node::Hook
                | Node::Conversion
                | Node::BotUser),
                key,
                token,
            ) => return self.app.scalar(node, key, token),
            (Node::Collaborator, b"id", Token::Number(text)) => {
                self.collaborator.id = crate::forgejo::id::unsigned(&text).ok_or(Malformed::Range { field: key })?;
            }
            (Node::Collaborator, b"login", Token::String(text)) => self.collaborator.login = Some(text),
            (Node::Collaborator, b"role_name", Token::String(text)) => {
                self.collaborator.permission = people::permission(&text)?;
            }
            (Node::User, b"id", Token::Number(text)) => {
                self.user.id = crate::forgejo::id::unsigned(&text).ok_or(Malformed::Range { field: key })?;
            }
            (Node::User, b"login", Token::String(text)) => self.user.login = text,
            (Node::Permission, b"role_name", Token::String(text)) => self.permission = people::permission(&text)?,
            (Node::Repository, b"id", Token::Number(text)) => {
                self.repository.id = crate::forgejo::id::unsigned(&text).expect("walker checked integer");
            }
            (Node::Owner, b"id", Token::Number(text)) => {
                self.repository.owner_id = crate::forgejo::id::unsigned(&text).expect("walker checked integer");
            }
            (Node::Owner, b"login", Token::String(text)) => self.repository.owner = text,
            (Node::Owner, b"type", Token::String(text)) => {
                self.repository.organization = text.as_ref() == b"Organization";
            }
            (Node::Repository, b"name", Token::String(text)) => self.repository.name = text,
            (Node::Repository, b"default_branch", Token::String(text)) => self.repository.default_branch = text,
            (Node::Repository, b"html_url", Token::String(text)) => self.repository.html_url = text,
            (Node::Repository, b"allow_merge_commit", token) => self.repository.merge = token == Token::True,
            (Node::Repository, b"allow_squash_merge", token) => self.repository.squash = token == Token::True,
            (Node::Repository, b"allow_rebase_merge", token) => self.repository.rebase = token == Token::True,
            (Node::Branch | Node::BranchName, b"name", Token::String(text)) => self.branch.name = text,
            (Node::Commit, b"sha", Token::String(text)) => {
                self.branch.commit = super::id::commit(&text).ok_or(Malformed::Id { field: key })?;
            }
            (Node::Error, b"message", Token::String(text)) => self.message = text,
            (Node::ErrorEntry, b"message", Token::String(text)) => self.error = text,
            (Node::Errors, b"message", Token::String(text)) => {
                if self.errors.push(text).is_err() {
                    return Err(Malformed::Bound { field: b"errors" });
                }
            }
            (
                Node::ActionsPermissions
                | Node::Workflows
                | Node::WorkflowRows
                | Node::Workflow
                | Node::Collaborator
                | Node::CollaboratorRows
                | Node::JobSteps
                | Node::User
                | Node::Permission
                | Node::IdResult
                | Node::NumberResult
                | Node::Done
                | Node::Repository
                | Node::Owner
                | Node::Branch
                | Node::BranchName
                | Node::Commit
                | Node::Error
                | Node::ErrorEntry
                | Node::BranchRows
                | Node::Errors,
                _,
                Token::ObjectStart
                | Token::ObjectEnd
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::Key(_)
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            ) => unreachable!("GitHub field table"),
        }
        Ok(())
    }
}
