//! Retained App fields, with secrets wrapped at their scalar event.
use super::super::document::{self, Document, Kind, Row};
use super::{Malformed, walk::Node};
use alloc::boxed::Box;
use skein_json::Token;
use skein_lib::{List, Secret, Wall};

#[derive(Debug)]
pub(super) struct State {
    kind: Kind,
    installation: document::Installation,
    minted: document::Minted,
    reachable: document::Reachable,
    viewer: document::Viewer,
    graphql: document::GraphqlRefusal,
    graphql_error: document::GraphqlRefusal,
    app: document::App,
    permissions: List<document::Permission>,
    events: List<Box<[u8]>>,
    permission_name: Box<[u8]>,
    hook: document::Hook,
    conversion: document::Conversion,
    bot: u64,
    has_viewer: bool,
    has_graphql: bool,
}
fn installation() -> document::Installation {
    document::Installation {
        id: 0,
        login: Box::from([]),
        account: 0,
        kind: document::Account::User,
        created: Wall::EPOCH,
        suspended: false,
    }
}
fn reachable() -> document::Reachable {
    document::Reachable { id: 0, owner: 0, login: Box::from([]), name: Box::from([]), private: false }
}
impl State {
    pub(super) fn new(kind: Kind, inner: u32) -> Self {
        let capacity = if kind == Kind::App { inner } else { 0 };
        Self {
            kind,
            installation: installation(),
            minted: document::Minted {
                permissions: document::Levels::NONE,
                token: Secret::new(Box::from([])),
                expires: Wall::EPOCH,
            },
            reachable: reachable(),
            viewer: document::Viewer { id: 0, login: Box::from([]) },
            graphql: document::GraphqlRefusal { kind: Box::from([]), message: Box::from([]) },
            graphql_error: document::GraphqlRefusal { kind: Box::from([]), message: Box::from([]) },
            app: document::App { id: 0, slug: Box::from([]), permissions: Box::from([]), events: Box::from([]) },
            permissions: List::with_capacity(capacity),
            events: List::with_capacity(capacity),
            permission_name: Box::from([]),
            hook: document::Hook { url: Box::from([]) },
            conversion: document::Conversion {
                id: 0,
                slug: Box::from([]),
                owner: Box::from([]),
                owner_id: 0,
                pem: Secret::new(Box::from([])),
                webhook_secret: Secret::new(Box::from([])),
            },
            bot: 0,
            has_viewer: false,
            has_graphql: false,
        }
    }
    #[expect(clippy::too_many_lines, reason = "App container dispatch exhaustively rejects the other document nodes")]
    pub(super) fn end(&mut self, node: Node) -> Result<Option<Row>, Malformed> {
        match node {
            Node::Installation => {
                if self.kind == Kind::InstallationRows {
                    return Ok(Some(Row::Installation(core::mem::replace(&mut self.installation, installation()))));
                }
            }
            Node::Reachable => return Ok(Some(Row::Reachable(core::mem::replace(&mut self.reachable, reachable())))),
            Node::Viewer => self.has_viewer = true,
            Node::GraphqlError => {
                let error = core::mem::replace(
                    &mut self.graphql_error,
                    document::GraphqlRefusal { kind: Box::from([]), message: Box::from([]) },
                );
                if !self.has_graphql
                    || super::super::failure::graphql(&self.graphql.kind, &self.graphql.message)
                        == crate::api::Error::Stale
                {
                    self.graphql = error;
                }
                self.has_graphql = true;
            }
            Node::GraphqlErrors => {
                if !self.has_graphql {
                    return Err(Malformed::Missing { field: b"type" });
                }
            }
            Node::InstallationAccount
            | Node::InstallationRows
            | Node::Minted
            | Node::MintPermissions
            | Node::ReachableEnvelope
            | Node::ReachableRows
            | Node::AppOwner
            | Node::ViewerEnvelope
            | Node::ViewerData
            | Node::App
            | Node::AppPermissions
            | Node::AppEvents
            | Node::Hook
            | Node::Conversion
            | Node::BotUser => {}
            Node::PullGraphql(_)
            | Node::RunOutput
            | Node::RunApp
            | Node::Job
            | Node::JobSteps
            | Node::JobStep
            | Node::User
            | Node::Compare
            | Node::MergeBaseOnly
            | Node::FileRows
            | Node::FileRow
            | Node::CompareCommits
            | Node::CompareCommit
            | Node::CompareFiles
            | Node::CompareFile
            | Node::MergeBase
            | Node::ReviewRows
            | Node::Review
            | Node::ReviewUser
            | Node::RemarkRows
            | Node::Remark
            | Node::RemarkUser
            | Node::CollaboratorRows
            | Node::Collaborator
            | Node::Permission
            | Node::Repository
            | Node::Owner
            | Node::Branch
            | Node::Commit
            | Node::BranchRows
            | Node::BranchName
            | Node::Error
            | Node::Errors
            | Node::IssueRows
            | Node::Issue
            | Node::IssueUser
            | Node::Labels
            | Node::Label
            | Node::PullRequest
            | Node::CommentRows
            | Node::Comment
            | Node::CommentUser
            | Node::PullRows
            | Node::PullObject
            | Node::PullRef { .. }
            | Node::Reviewers
            | Node::Reviewer
            | Node::CombinedObject
            | Node::Contexts
            | Node::Context
            | Node::RunsObject
            | Node::Runs
            | Node::RunItem
            | Node::Protection(_)
            | Node::IdResult
            | Node::NumberResult
            | Node::Done
            | Node::LogRows
            | Node::LogRow
            | Node::LogDelivery
            | Node::LogRequest
            | Node::LogHeaders
            | Node::ActionsPermissions
            | Node::Workflows
            | Node::WorkflowRows
            | Node::Workflow
            | Node::ErrorEntry => unreachable!("App container dispatch"),
        }
        Ok(None)
    }
    pub(super) fn graphql(&self) -> Option<&document::GraphqlRefusal> {
        if self.has_graphql { Some(&self.graphql) } else { None }
    }
    pub(super) fn document(&self) -> Result<Document, Malformed> {
        Ok(match self.kind {
            Kind::Installation => Document::Installation(self.installation.clone()),
            Kind::InstallationRows | Kind::ReachableRows => Document::Rows,
            Kind::Minted => Document::Minted(self.minted.clone()),
            Kind::Viewer => {
                if self.has_graphql {
                    Document::Graphql(self.graphql.clone())
                } else if self.has_viewer {
                    Document::Viewer(self.viewer.clone())
                } else {
                    return Err(Malformed::Missing { field: b"viewer" });
                }
            }
            Kind::App => Document::App(document::App {
                id: self.app.id,
                slug: self.app.slug.clone(),
                permissions: self.permissions.clone().into_boxed(),
                events: self.events.clone().into_boxed(),
            }),
            Kind::HookConfig => Document::HookConfig(self.hook.clone()),
            Kind::Conversion => Document::Conversion(self.conversion.clone()),
            Kind::BotUser => Document::BotUser(self.bot),
            Kind::Merge
            | Kind::Enqueue
            | Kind::PullGraphql
            | Kind::User
            | Kind::Compare
            | Kind::FileRows
            | Kind::ReviewRows
            | Kind::RemarkRows
            | Kind::CollaboratorRows
            | Kind::Permission
            | Kind::Repository
            | Kind::Branch
            | Kind::BranchRows
            | Kind::LogRows
            | Kind::LogDelivery
            | Kind::IssueRows
            | Kind::Issue
            | Kind::CommentRows
            | Kind::Pull
            | Kind::PullRows
            | Kind::CombinedStatus
            | Kind::Job
            | Kind::CheckRuns
            | Kind::Id
            | Kind::Number
            | Kind::Done
            | Kind::RuleRows
            | Kind::Ruleset
            | Kind::ClassicProtection
            | Kind::ActionsPermissions
            | Kind::Workflows
            | Kind::Error => unreachable!("App document dispatch"),
        })
    }
    #[expect(
        clippy::too_many_lines,
        reason = "the field dispatcher includes every document node in its unreachable arm"
    )]
    pub(super) fn scalar(&mut self, node: Node, key: &'static [u8], token: Token) -> Result<(), Malformed> {
        match (node, key, token) {
            (Node::Installation, b"id", Token::Number(text)) => self.installation.id = unsigned(&text),
            (Node::Installation, b"created_at", Token::String(text)) => self.installation.created = time(&text, key)?,
            (Node::Installation, b"suspended_at", Token::True) => self.installation.suspended = true,
            (Node::InstallationAccount, b"login", Token::String(text)) => self.installation.login = text,
            (Node::InstallationAccount, b"id", Token::Number(text)) => self.installation.account = unsigned(&text),
            (Node::InstallationAccount, b"type", Token::String(text)) => {
                self.installation.kind = match text.as_ref() {
                    b"User" => document::Account::User,
                    b"Organization" => document::Account::Organization,
                    _ => return Err(Malformed::Value { field: key }),
                };
            }
            (Node::MintPermissions, b"contents", Token::String(text)) => {
                self.minted.permissions.contents = document::Level::read(&text);
            }
            (Node::MintPermissions, b"pull_requests", Token::String(text)) => {
                self.minted.permissions.pull_requests = document::Level::read(&text);
            }
            (Node::Minted, b"token", Token::String(text)) => self.minted.token = Secret::new(text),
            (Node::Minted, b"expires_at", Token::String(text)) => self.minted.expires = time(&text, key)?,
            (Node::Reachable, b"id", Token::Number(text)) => self.reachable.id = unsigned(&text),
            (Node::Reachable, b"name", Token::String(text)) => self.reachable.name = text,
            (Node::Reachable, b"private", token) => self.reachable.private = token == Token::True,
            (Node::AppOwner, b"id", Token::Number(text)) => {
                if self.kind == Kind::Conversion {
                    self.conversion.owner_id = unsigned(&text);
                } else {
                    self.reachable.owner = unsigned(&text);
                }
            }
            (Node::AppOwner, b"login", Token::String(text)) => {
                if self.kind == Kind::Conversion {
                    self.conversion.owner = text;
                } else {
                    self.reachable.login = text;
                }
            }
            (Node::Viewer, b"databaseId", Token::Number(text)) => self.viewer.id = unsigned(&text),
            (Node::Viewer, b"login", Token::String(text)) => self.viewer.login = text,
            (Node::GraphqlError, b"type", Token::String(text)) => self.graphql_error.kind = text,
            (Node::GraphqlError, b"message", Token::String(text)) => self.graphql_error.message = text,
            (Node::AppPermissions, b"name", Token::String(text)) => self.permission_name = text,
            (Node::AppPermissions, b"access", Token::String(text)) => {
                let access = document::Access::read(&text).ok_or(Malformed::Value { field: b"permissions" })?;
                let name = core::mem::replace(&mut self.permission_name, Box::from([]));
                self.permissions
                    .push(document::Permission { name, access })
                    .or(Err(Malformed::Bound { field: b"permissions" }))?;
            }
            (Node::AppEvents, b"events", Token::String(text)) => {
                self.events.push(text).or(Err(Malformed::Bound { field: b"events" }))?;
            }
            (Node::App, b"id", Token::Number(text)) => self.app.id = unsigned(&text),
            (Node::App, b"slug", Token::String(text)) => self.app.slug = text,
            (Node::Hook, b"url", Token::String(text)) => self.hook.url = text,
            (Node::Conversion, b"id", Token::Number(text)) => self.conversion.id = unsigned(&text),
            (Node::Conversion, b"slug", Token::String(text)) => self.conversion.slug = text,
            (Node::Conversion, b"pem", Token::String(text)) => self.conversion.pem = Secret::new(text),
            (Node::Conversion, b"webhook_secret", Token::String(text)) => {
                self.conversion.webhook_secret = Secret::new(text);
            }
            (Node::BotUser, b"id", Token::Number(text)) => self.bot = unsigned(&text),
            (
                Node::Installation
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
                | Node::BotUser
                | Node::User
                | Node::Compare
                | Node::MergeBaseOnly
                | Node::FileRows
                | Node::FileRow
                | Node::CompareCommits
                | Node::CompareCommit
                | Node::CompareFiles
                | Node::CompareFile
                | Node::MergeBase
                | Node::ReviewRows
                | Node::Review
                | Node::ReviewUser
                | Node::RemarkRows
                | Node::Remark
                | Node::RemarkUser
                | Node::CollaboratorRows
                | Node::Collaborator
                | Node::Permission
                | Node::Repository
                | Node::Owner
                | Node::Branch
                | Node::Commit
                | Node::BranchRows
                | Node::BranchName
                | Node::Error
                | Node::Errors
                | Node::IssueRows
                | Node::Issue
                | Node::IssueUser
                | Node::Labels
                | Node::Label
                | Node::PullRequest
                | Node::CommentRows
                | Node::Comment
                | Node::CommentUser
                | Node::PullRows
                | Node::PullObject
                | Node::PullRef { .. }
                | Node::Reviewers
                | Node::Reviewer
                | Node::CombinedObject
                | Node::Contexts
                | Node::Context
                | Node::RunsObject
                | Node::Runs
                | Node::RunItem
                | Node::PullGraphql(_)
                | Node::RunOutput
                | Node::RunApp
                | Node::Job
                | Node::Protection(_)
                | Node::JobSteps
                | Node::JobStep
                | Node::IdResult
                | Node::NumberResult
                | Node::Done
                | Node::ActionsPermissions
                | Node::Workflows
                | Node::WorkflowRows
                | Node::Workflow
                | Node::ErrorEntry
                | Node::LogRows
                | Node::LogRow
                | Node::LogDelivery
                | Node::LogRequest
                | Node::LogHeaders,
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
            ) => unreachable!("App field table"),
        }
        Ok(())
    }
}
fn unsigned(text: &[u8]) -> u64 {
    crate::forgejo::id::unsigned(text).expect("walker checked integer")
}
pub(super) fn time(text: &[u8], field: &'static [u8]) -> Result<Wall, Malformed> {
    crate::forgejo::time::read(text).ok_or(Malformed::Time { field })
}
