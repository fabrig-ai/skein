//! GitHub's bounded field walker, independent of Forgejo's tables.
use super::super::delivery::{Ci, Event as Hook};
use super::super::{
    document,
    limits::{DEPTH, Limits},
};
use super::{Malformed, pull_graphql::Node as PullNode};
use skein_json::{
    Token,
    tokenizer::{self, Event, Request},
};
use skein_lib::Stack;
mod protection;
use super::protection::Node as P;
/// Container roles in the first GitHub documents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Node {
    ActionsPermissions,
    Workflows,
    WorkflowRows,
    Workflow,
    Protection(P),
    LogRows,
    LogRow,
    LogDelivery,
    LogRequest,
    LogHeaders,
    IdResult,
    NumberResult,
    Done,
    Installation,
    InstallationAccount,
    InstallationRows,
    Minted,
    MintPermissions,
    ReachableEnvelope,
    ReachableRows,
    Reachable,
    AppOwner,
    PullGraphql(PullNode),
    ViewerEnvelope,
    ViewerData,
    Viewer,
    GraphqlErrors,
    GraphqlError,
    App,
    AppPermissions,
    AppEvents,
    Hook,
    Conversion,
    BotUser,
    User,
    Permission,
    CollaboratorRows,
    RemarkRows,
    ReviewRows,
    Compare,
    MergeBaseOnly,
    FileRows,
    FileRow,
    CompareCommits,
    CompareCommit,
    CompareFiles,
    CompareFile,
    MergeBase,
    Review,
    ReviewUser,
    Remark,
    RemarkUser,
    Collaborator,
    Repository,
    Owner,
    Branch,
    Commit,
    BranchRows,
    BranchName,
    Error,
    Errors,
    ErrorEntry,
    IssueRows,
    Issue,
    IssueUser,
    Labels,
    Label,
    PullRequest,
    CommentRows,
    Comment,
    CommentUser,
    PullRows,
    PullObject,
    PullRef { head: bool },
    Reviewers,
    Reviewer,
    CombinedObject,
    Contexts,
    Context,
    RunsObject,
    Runs,
    RunItem,
    RunOutput,
    RunApp,
    Job,
    JobSteps,
    JobStep,
}
/// Payload roles share the walker without extending the document-only Node vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Payload {
    Root(Hook),
    Repository,
    RepositoryOwner,
    Item,
    Number,
    Body,
    Head,
    Sender,
    Installation,
    InstallationId,
    Account,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Document(Node),
    Payload(Payload),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Read {
    Text,
    Login,
    Name,
    Body,
    LongBody,
    LongText,
    Reason,
    Secret,
    Presence,
    Number,
    Boolean,
    Container(Role),
    Payload,
}
#[derive(Clone, Copy, Debug)]
struct Field {
    key: &'static [u8],
    read: Read,
    required: bool,
    nullable: bool,
}
const fn field(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: true, nullable: false }
}
const fn optional(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: false, nullable: false }
}
const fn nullable(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: true, nullable: true }
}
const fn pull_node(key: &'static [u8], node: PullNode) -> Field {
    field(key, Read::Container(Role::Document(Node::PullGraphql(node))))
}
fn pull_fields(node: PullNode) -> &'static [Field] {
    match node {
        PullNode::EnqueueEnvelope => {
            const {
                &[
                    optional(b"data", Read::Container(Role::Document(Node::PullGraphql(PullNode::EnqueueData)))),
                    optional(b"errors", Read::Container(Role::Document(Node::GraphqlErrors))),
                ]
            }
        }
        PullNode::Envelope => {
            const {
                &[
                    optional(b"data", Read::Container(Role::Document(Node::PullGraphql(PullNode::Data)))),
                    optional(b"errors", Read::Container(Role::Document(Node::GraphqlErrors))),
                ]
            }
        }
        PullNode::EnqueueData => {
            const {
                &[optional(
                    b"enqueuePullRequest",
                    Read::Container(Role::Document(Node::PullGraphql(PullNode::Enqueue))),
                )]
            }
        }
        PullNode::Enqueue => {
            const { &[optional(b"mergeQueueEntry", Read::Container(Role::Document(Node::PullGraphql(PullNode::Queue))))] }
        }
        PullNode::Data => {
            const { &[optional(b"repository", Read::Container(Role::Document(Node::PullGraphql(PullNode::Repository))))] }
        }
        PullNode::Repository => {
            const { &[optional(b"pullRequest", Read::Container(Role::Document(Node::PullGraphql(PullNode::Pull))))] }
        }
        PullNode::Pull => {
            const {
                &[
                    field(b"id", Read::Text),
                    nullable(b"mergeCommit", Read::Container(Role::Document(Node::PullGraphql(PullNode::Commit)))),
                    nullable(b"mergeQueueEntry", Read::Container(Role::Document(Node::PullGraphql(PullNode::Queue)))),
                    pull_node(b"timelineItems", PullNode::Timeline),
                ]
            }
        }
        PullNode::Commit => const { &[field(b"oid", Read::Text)] },
        PullNode::Queue => const { &[field(b"position", Read::Number)] },
        PullNode::Timeline => const { &[pull_node(b"nodes", PullNode::Nodes)] },
        PullNode::Removal => const { &[nullable(b"reason", Read::Reason)] },
        PullNode::Nodes => &[],
    }
}
fn issue_fields() -> &'static [Field] {
    const {
        &[
            field(b"number", Read::Number),
            field(b"state", Read::Text),
            field(b"user", Read::Container(Role::Document(Node::IssueUser))),
            optional(b"labels", Read::Container(Role::Document(Node::Labels))),
            field(b"updated_at", Read::Text),
            field(b"title", Read::Text),
            optional(b"body", Read::LongBody),
            optional(b"pull_request", Read::Container(Role::Document(Node::PullRequest))),
        ]
    }
}
fn comment_fields() -> &'static [Field] {
    const {
        &[
            field(b"id", Read::Number),
            field(b"user", Read::Container(Role::Document(Node::CommentUser))),
            optional(b"created_at", Read::Text),
            optional(b"updated_at", Read::Text),
            optional(b"body", Read::LongBody),
        ]
    }
}
#[expect(clippy::too_many_lines, reason = "one exhaustive table lists the fields of every document node")]
fn document_fields(node: Node) -> &'static [Field] {
    match node {
        Node::ActionsPermissions => const { &[field(b"enabled", Read::Boolean)] },
        Node::Workflows => {
            const {
                &[
                    field(b"total_count", Read::Number),
                    field(b"workflows", Read::Container(Role::Document(Node::WorkflowRows))),
                ]
            }
        }
        Node::Workflow => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"name", Read::Text),
                    field(b"path", Read::Text),
                    field(b"state", Read::Text),
                ]
            }
        }
        Node::Protection(node) => protection::fields(node),
        Node::LogRow => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"guid", Read::Text),
                    field(b"event", Read::Text),
                    optional(b"action", Read::Text),
                    optional(b"installation_id", Read::Number),
                    optional(b"repository_id", Read::Number),
                    field(b"status_code", Read::Number),
                    field(b"delivered_at", Read::Text),
                    field(b"redelivery", Read::Boolean),
                ]
            }
        }
        Node::LogDelivery => {
            const { &[field(b"event", Read::Text), field(b"request", Read::Container(Role::Document(Node::LogRequest)))] }
        }
        Node::LogRequest => {
            const {
                &[
                    field(b"headers", Read::Container(Role::Document(Node::LogHeaders))),
                    field(b"payload", Read::Payload),
                ]
            }
        }
        Node::LogHeaders => const { &[field(b"X-GitHub-Event", Read::Text), field(b"X-GitHub-Delivery", Read::Text)] },
        Node::PullGraphql(node) => pull_fields(node),
        Node::Compare => {
            const {
                &[
                    field(b"total_commits", Read::Number),
                    field(b"behind_by", Read::Number),
                    field(b"commits", Read::Container(Role::Document(Node::CompareCommits))),
                    field(b"files", Read::Container(Role::Document(Node::CompareFiles))),
                    optional(b"merge_base_commit", Read::Container(Role::Document(Node::MergeBase))),
                ]
            }
        }
        Node::MergeBaseOnly => {
            const { &[field(b"merge_base_commit", Read::Container(Role::Document(Node::MergeBase)))] }
        }
        Node::FileRow => {
            const {
                &[
                    field(b"filename", Read::Text),
                    field(b"status", Read::Text),
                    optional(b"previous_filename", Read::Text),
                ]
            }
        }
        Node::CompareFile => const { &[field(b"filename", Read::Text)] },
        Node::Review => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"user", Read::Container(Role::Document(Node::ReviewUser))),
                    field(b"state", Read::Text),
                    field(b"commit_id", Read::Text),
                    optional(b"body", Read::LongBody),
                    optional(b"submitted_at", Read::Text),
                    field(b"author_association", Read::Text),
                ]
            }
        }
        Node::Remark => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"user", Read::Container(Role::Document(Node::RemarkUser))),
                    field(b"path", Read::Text),
                    optional(b"line", Read::Number),
                    optional(b"original_line", Read::Number),
                    optional(b"body", Read::LongBody),
                ]
            }
        }
        Node::Collaborator => {
            const { &[field(b"id", Read::Number), field(b"role_name", Read::Text), optional(b"login", Read::Text)] }
        }
        Node::User => const { &[field(b"id", Read::Number), field(b"login", Read::Text)] },
        Node::Permission => const { &[field(b"role_name", Read::Text)] },
        Node::NumberResult => const { &[field(b"number", Read::Number)] },
        Node::PullObject => {
            const {
                &[
                    field(b"number", Read::Number),
                    field(b"state", Read::Text),
                    optional(b"merged", Read::Boolean),
                    optional(b"merge_commit_sha", Read::Text),
                    optional(b"mergeable", Read::Boolean),
                    field(b"head", Read::Container(Role::Document(Node::PullRef { head: true }))),
                    field(b"base", Read::Container(Role::Document(Node::PullRef { head: false }))),
                    optional(b"requested_reviewers", Read::Container(Role::Document(Node::Reviewers))),
                ]
            }
        }
        Node::PullRef { .. } => const { &[field(b"ref", Read::Text), field(b"sha", Read::Text)] },
        Node::CombinedObject => {
            const {
                &[
                    field(b"state", Read::Text),
                    field(b"total_count", Read::Number),
                    optional(b"statuses", Read::Container(Role::Document(Node::Contexts))),
                ]
            }
        }
        Node::Context => {
            const {
                &[
                    optional(b"updated_at", Read::Text),
                    field(b"context", Read::Text),
                    field(b"state", Read::Text),
                    optional(b"description", Read::Text),
                    optional(b"target_url", Read::Text),
                ]
            }
        }
        Node::RunsObject => const { &[field(b"check_runs", Read::Container(Role::Document(Node::Runs)))] },
        Node::RunItem => {
            const {
                &[
                    field(b"status", Read::Text),
                    optional(b"conclusion", Read::Text),
                    optional(b"id", Read::Number),
                    optional(b"head_sha", Read::Text),
                    optional(b"completed_at", Read::Text),
                    optional(b"started_at", Read::Text),
                    optional(b"name", Read::Text),
                    optional(b"html_url", Read::Text),
                    optional(b"output", Read::Container(Role::Document(Node::RunOutput))),
                    optional(b"app", Read::Container(Role::Document(Node::RunApp))),
                ]
            }
        }
        Node::RunOutput => const { &[optional(b"title", Read::Text)] },
        Node::RunApp => const { &[optional(b"slug", Read::Text)] },
        Node::JobStep => {
            const { &[field(b"number", Read::Number), field(b"name", Read::Text), optional(b"conclusion", Read::Text)] }
        }
        Node::Job => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"run_id", Read::Number),
                    field(b"run_attempt", Read::Number),
                    optional(b"steps", Read::Container(Role::Document(Node::JobSteps))),
                    field(b"head_sha", Read::Text),
                ]
            }
        }
        Node::Issue => issue_fields(),
        Node::IdResult
        | Node::ReviewUser
        | Node::RemarkUser
        | Node::Reviewer
        | Node::IssueUser
        | Node::CommentUser
        | Node::BotUser => const { &[field(b"id", Read::Number)] },
        Node::Label => const { &[field(b"name", Read::Text)] },
        Node::BranchName => const { &[field(b"name", Read::LongText)] },
        Node::PullRequest => const { &[optional(b"merged_at", Read::Presence)] },
        Node::Comment => comment_fields(),
        Node::Installation => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"account", Read::Container(Role::Document(Node::InstallationAccount))),
                    field(b"created_at", Read::Text),
                    optional(b"suspended_at", Read::Presence),
                ]
            }
        }
        Node::InstallationAccount => {
            const { &[field(b"login", Read::Text), field(b"id", Read::Number), field(b"type", Read::Text)] }
        }
        Node::Minted => {
            const {
                &[
                    field(b"token", Read::Secret),
                    field(b"expires_at", Read::Text),
                    optional(b"permissions", Read::Container(Role::Document(Node::MintPermissions))),
                ]
            }
        }
        Node::MintPermissions => const { &[optional(b"contents", Read::Text), optional(b"pull_requests", Read::Text)] },
        Node::ReachableEnvelope => {
            const { &[field(b"repositories", Read::Container(Role::Document(Node::ReachableRows)))] }
        }
        Node::Reachable => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"owner", Read::Container(Role::Document(Node::AppOwner))),
                    field(b"name", Read::Text),
                    field(b"private", Read::Boolean),
                ]
            }
        }
        Node::AppOwner => const { &[field(b"login", Read::Text), field(b"id", Read::Number)] },
        Node::ViewerEnvelope => {
            const {
                &[
                    optional(b"data", Read::Container(Role::Document(Node::ViewerData))),
                    optional(b"errors", Read::Container(Role::Document(Node::GraphqlErrors))),
                ]
            }
        }
        Node::ViewerData => const { &[optional(b"viewer", Read::Container(Role::Document(Node::Viewer)))] },
        Node::Viewer => const { &[field(b"databaseId", Read::Number), field(b"login", Read::Text)] },
        Node::GraphqlError => const { &[field(b"type", Read::Text), field(b"message", Read::Text)] },
        Node::App => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"slug", Read::Text),
                    optional(b"permissions", Read::Container(Role::Document(Node::AppPermissions))),
                    optional(b"events", Read::Container(Role::Document(Node::AppEvents))),
                ]
            }
        }
        Node::Hook => const { &[field(b"url", Read::Text)] },
        Node::Conversion => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"slug", Read::Text),
                    field(b"owner", Read::Container(Role::Document(Node::AppOwner))),
                    field(b"pem", Read::Secret),
                    field(b"webhook_secret", Read::Secret),
                ]
            }
        }
        Node::Repository => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"owner", Read::Container(Role::Document(Node::Owner))),
                    field(b"name", Read::Text),
                    field(b"default_branch", Read::Text),
                    field(b"html_url", Read::Text),
                    field(b"allow_merge_commit", Read::Boolean),
                    field(b"allow_squash_merge", Read::Boolean),
                    field(b"allow_rebase_merge", Read::Boolean),
                ]
            }
        }
        Node::Owner => const { &[field(b"login", Read::Text), field(b"id", Read::Number), field(b"type", Read::Text)] },
        Node::Branch => {
            const { &[field(b"name", Read::Text), field(b"commit", Read::Container(Role::Document(Node::Commit)))] }
        }
        Node::CompareCommit | Node::MergeBase | Node::Commit => const { &[field(b"sha", Read::Text)] },
        Node::Error => {
            const { &[field(b"message", Read::Body), optional(b"errors", Read::Container(Role::Document(Node::Errors)))] }
        }
        Node::ErrorEntry => const { &[field(b"message", Read::Text)] },
        Node::WorkflowRows
        | Node::JobSteps
        | Node::LogRows
        | Node::AppPermissions
        | Node::AppEvents
        | Node::FileRows
        | Node::CompareCommits
        | Node::CompareFiles
        | Node::ReviewRows
        | Node::RemarkRows
        | Node::CollaboratorRows
        | Node::Done
        | Node::PullRows
        | Node::Reviewers
        | Node::Contexts
        | Node::Runs
        | Node::IssueRows
        | Node::CommentRows
        | Node::Labels
        | Node::InstallationRows
        | Node::ReachableRows
        | Node::GraphqlErrors
        | Node::BranchRows
        | Node::Errors => &[],
    }
}
#[expect(clippy::too_many_lines, reason = "one exhaustive table lists the element role of every document node")]
fn element(role: Role) -> Option<Role> {
    let node = match role {
        Role::Document(node) => node,
        Role::Payload(_) => return None,
    };
    match node {
        Node::WorkflowRows => Some(Role::Document(Node::Workflow)),
        Node::Protection(node) => protection::element(node),
        Node::JobSteps => Some(Role::Document(Node::JobStep)),
        Node::LogRows => Some(Role::Document(Node::LogRow)),
        Node::AppEvents => Some(Role::Document(Node::AppEvents)),
        Node::FileRows => Some(Role::Document(Node::FileRow)),
        Node::CompareCommits => Some(Role::Document(Node::CompareCommit)),
        Node::CompareFiles => Some(Role::Document(Node::CompareFile)),
        Node::ReviewRows => Some(Role::Document(Node::Review)),
        Node::RemarkRows => Some(Role::Document(Node::Remark)),
        Node::CollaboratorRows => Some(Role::Document(Node::Collaborator)),
        Node::PullRows => Some(Role::Document(Node::PullObject)),
        Node::Reviewers => Some(Role::Document(Node::Reviewer)),
        Node::Contexts => Some(Role::Document(Node::Context)),
        Node::Runs => Some(Role::Document(Node::RunItem)),
        Node::IssueRows => Some(Role::Document(Node::Issue)),
        Node::CommentRows => Some(Role::Document(Node::Comment)),
        Node::Labels => Some(Role::Document(Node::Label)),
        Node::InstallationRows => Some(Role::Document(Node::Installation)),
        Node::ReachableRows => Some(Role::Document(Node::Reachable)),
        Node::PullGraphql(PullNode::Nodes) => Some(Role::Document(Node::PullGraphql(PullNode::Removal))),
        Node::GraphqlErrors => Some(Role::Document(Node::GraphqlError)),
        Node::BranchRows => Some(Role::Document(Node::BranchName)),
        Node::Errors => Some(Role::Document(Node::ErrorEntry)),
        Node::ActionsPermissions
        | Node::Workflows
        | Node::Workflow
        | Node::PullGraphql(
            PullNode::EnqueueEnvelope
            | PullNode::EnqueueData
            | PullNode::Enqueue
            | PullNode::Envelope
            | PullNode::Data
            | PullNode::Repository
            | PullNode::Pull
            | PullNode::Commit
            | PullNode::Queue
            | PullNode::Timeline
            | PullNode::Removal,
        )
        | Node::LogRow
        | Node::LogDelivery
        | Node::LogRequest
        | Node::LogHeaders
        | Node::IdResult
        | Node::NumberResult
        | Node::Done
        | Node::PullObject
        | Node::PullRef { .. }
        | Node::Reviewer
        | Node::CombinedObject
        | Node::Context
        | Node::RunsObject
        | Node::RunItem
        | Node::RunOutput
        | Node::RunApp
        | Node::Job
        | Node::JobStep
        | Node::Issue
        | Node::IssueUser
        | Node::Label
        | Node::PullRequest
        | Node::Comment
        | Node::CommentUser
        | Node::Installation
        | Node::InstallationAccount
        | Node::Minted
        | Node::MintPermissions
        | Node::ReachableEnvelope
        | Node::Reachable
        | Node::AppOwner
        | Node::ViewerEnvelope
        | Node::ViewerData
        | Node::Viewer
        | Node::GraphqlError
        | Node::App
        | Node::AppPermissions
        | Node::Hook
        | Node::Conversion
        | Node::User
        | Node::Permission
        | Node::Compare
        | Node::MergeBaseOnly
        | Node::FileRow
        | Node::CompareCommit
        | Node::CompareFile
        | Node::MergeBase
        | Node::Review
        | Node::ReviewUser
        | Node::Remark
        | Node::RemarkUser
        | Node::Collaborator
        | Node::BotUser
        | Node::Repository
        | Node::Owner
        | Node::Branch
        | Node::Commit
        | Node::BranchName
        | Node::Error
        | Node::ErrorEntry => None,
    }
}
#[derive(Debug)]
struct Frame {
    node: Role,
    touched: u32,
    seen: u32,
}
#[derive(Debug)]
enum Want {
    Start(Role),
    Key,
    Value(Field, u32),
    Element,
    AppAccess,
    Unknown(u32),
    Done,
    Terminal,
}
/// One field or container completion, with the next tokenizer demand.
#[derive(Debug)]
pub(super) enum Move {
    PayloadStart(Token),
    Demand(Request),
    Scalar(Node, &'static [u8], Token),
    End(Node),
    PayloadScalar(Payload, &'static [u8], Token),
    PayloadEnd,
    BodyLong(Option<Node>, u64),
    Done,
}
/// A fixed-depth walker retaining field masks and no document tree.
#[derive(Debug)]
pub(super) struct Walk {
    open: Stack<Frame>,
    want: Want,
    limits: Limits,
    enqueue: bool,
}
impl Walk {
    pub(super) fn new(kind: document::Kind, limits: &Limits) -> Self {
        let node = match kind {
            document::Kind::ActionsPermissions => Node::ActionsPermissions,
            document::Kind::Workflows => Node::Workflows,
            document::Kind::RuleRows => Node::Protection(P::Rows),
            document::Kind::Ruleset => Node::Protection(P::Ruleset),
            document::Kind::ClassicProtection => Node::Protection(P::Classic),
            document::Kind::LogRows => Node::LogRows,
            document::Kind::LogDelivery => Node::LogDelivery,
            document::Kind::Merge => Node::Commit,
            document::Kind::Enqueue => Node::PullGraphql(PullNode::EnqueueEnvelope),
            document::Kind::Id => Node::IdResult,
            document::Kind::Number => Node::NumberResult,
            document::Kind::Done => Node::Done,
            document::Kind::Installation => Node::Installation,
            document::Kind::InstallationRows => Node::InstallationRows,
            document::Kind::Minted => Node::Minted,
            document::Kind::ReachableRows => Node::ReachableEnvelope,
            document::Kind::PullGraphql => Node::PullGraphql(PullNode::Envelope),
            document::Kind::Viewer => Node::ViewerEnvelope,
            document::Kind::App => Node::App,
            document::Kind::HookConfig => Node::Hook,
            document::Kind::Conversion => Node::Conversion,
            document::Kind::BotUser => Node::BotUser,
            document::Kind::User => Node::User,
            document::Kind::Permission => Node::Permission,
            document::Kind::FileRows => Node::FileRows,
            document::Kind::Compare => Node::Compare,
            document::Kind::ReviewRows => Node::ReviewRows,
            document::Kind::RemarkRows => Node::RemarkRows,
            document::Kind::CollaboratorRows => Node::CollaboratorRows,
            document::Kind::Repository => Node::Repository,
            document::Kind::Branch => Node::Branch,
            document::Kind::BranchRows => Node::BranchRows,
            document::Kind::IssueRows => Node::IssueRows,
            document::Kind::Issue => Node::Issue,
            document::Kind::CommentRows => Node::CommentRows,
            document::Kind::Pull => Node::PullObject,
            document::Kind::PullRows => Node::PullRows,
            document::Kind::CombinedStatus => Node::CombinedObject,
            document::Kind::CheckRuns => Node::RunsObject,
            document::Kind::Job => Node::Job,
            document::Kind::Error => Node::Error,
        };
        let mut walk = Self::root(Role::Document(node), limits);
        walk.enqueue = kind == document::Kind::Enqueue;
        walk
    }
    pub(super) fn merge_base(limits: &Limits) -> Self {
        Self::root(Role::Document(Node::MergeBaseOnly), limits)
    }
    /// A delivery has its own root but uses the same field masks and depth bound.
    pub(super) fn delivery(event: Hook, limits: &Limits) -> Self {
        Self::root(Role::Payload(Payload::Root(event)), limits)
    }
    fn root(role: Role, limits: &Limits) -> Self {
        Self { open: Stack::with_capacity(DEPTH), want: Want::Start(role), limits: *limits, enqueue: false }
    }
    pub(super) fn step(&mut self, event: Event) -> Result<Move, Malformed> {
        if let Event::Failed(error) = event {
            return Err(Malformed::Json(error));
        }
        let want = core::mem::replace(&mut self.want, Want::Terminal);
        match want {
            Want::Start(node) => self.start(node, &event),
            Want::Key => self.key(event),
            Want::Value(field, bit) => self.value(field, bit, event),
            Want::Element => {
                if event == Event::Token(Token::ArrayEnd) {
                    self.end()
                } else {
                    let frame = self.open.top_mut().expect("array");
                    if frame.node == Role::Document(Node::GraphqlErrors) && frame.seen != 0 && !self.enqueue {
                        return self.unknown(&event, 0);
                    }
                    if frame.node == Role::Document(Node::Protection(P::Contexts)) {
                        frame.seen = frame.seen.checked_add(1).ok_or(Malformed::Bound { field: b"contexts" })?;
                        if frame.seen > self.limits.inner {
                            return Err(Malformed::Bound { field: b"contexts" });
                        }
                        self.want = Want::Element;
                        return Ok(Move::Scalar(
                            Node::Protection(P::Contexts),
                            b"contexts",
                            self.app_text(event, b"contexts")?,
                        ));
                    }
                    if frame.node == Role::Document(Node::AppEvents) {
                        frame.seen = frame.seen.checked_add(1).ok_or(Malformed::Bound { field: b"events" })?;
                        if frame.seen > self.limits.inner {
                            return Err(Malformed::Bound { field: b"events" });
                        }
                        self.want = Want::Element;
                        return Ok(Move::Scalar(Node::AppEvents, b"events", self.app_text(event, b"events")?));
                    }
                    frame.seen = frame.seen.checked_add(1).ok_or(Malformed::Bound { field: b"errors" })?;
                    if (frame.node == Role::Document(Node::LogRows) || frame.node == Role::Document(Node::JobSteps))
                        && frame.seen > self.limits.inner
                    {
                        return Err(Malformed::Bound {
                            field: if frame.node == Role::Document(Node::JobSteps) { b"steps" } else { b"deliveries" },
                        });
                    }
                    if self.enqueue
                        && frame.node == Role::Document(Node::GraphqlErrors)
                        && frame.seen > self.limits.inner
                    {
                        return Err(Malformed::Bound { field: b"errors" });
                    }
                    if frame.node == Role::Document(Node::Errors)
                        && let Event::Token(Token::String(text)) = event
                    {
                        self.want = Want::Element;
                        return Ok(Move::Scalar(Node::Errors, b"message", Token::String(text)));
                    }
                    let node = element(frame.node).expect("array role");
                    self.start(node, &event)
                }
            }
            Want::AppAccess => {
                self.want = Want::Key;
                match event {
                    Event::Token(Token::String(text)) => {
                        Ok(Move::Scalar(Node::AppPermissions, b"access", Token::String(text)))
                    }
                    Event::Long(_) => Err(Malformed::Value { field: b"permissions" }),
                    Event::Token(
                        Token::ObjectStart
                        | Token::ObjectEnd
                        | Token::ArrayStart
                        | Token::ArrayEnd
                        | Token::Key(_)
                        | Token::Number(_)
                        | Token::True
                        | Token::False
                        | Token::Null,
                    )
                    | Event::Skipped(_)
                    | Event::Done
                    | Event::Failed(_)
                    | Event::Closed => Err(Malformed::Shape { field: b"permissions" }),
                }
            }
            Want::Unknown(depth) => self.unknown(&event, depth),
            Want::Done => {
                if event == Event::Done {
                    Ok(Move::Done)
                } else {
                    Err(Malformed::Shape { field: b"" })
                }
            }
            Want::Terminal => Err(Malformed::Shape { field: b"" }),
        }
    }
    pub(super) fn demand(&self) -> Request {
        match self.want {
            Want::AppAccess => Request::Text(5),
            Want::Key => {
                if self.open.top().expect("object").node == Role::Document(Node::AppPermissions) {
                    Request::Text(self.limits.string_bytes)
                } else {
                    Request::Next
                }
            }
            Want::Element => {
                if self.open.top().expect("array").node == Role::Document(Node::AppEvents)
                    || self.open.top().expect("array").node == Role::Document(Node::Protection(P::Contexts))
                {
                    Request::Text(self.limits.string_bytes)
                } else {
                    Request::Next
                }
            }
            Want::Start(_) | Want::Value(_, _) | Want::Unknown(_) | Want::Done | Want::Terminal => Request::Next,
        }
    }
    fn start(&mut self, node: Role, event: &Event) -> Result<Move, Malformed> {
        let array = element(node).is_some();
        if event != &Event::Token(if array { Token::ArrayStart } else { Token::ObjectStart }) {
            return Err(Malformed::Shape { field: b"" });
        }
        if self.open.push(Frame { node, touched: 0, seen: 0 }).is_err() {
            return Err(Malformed::Json(tokenizer::Error::TooDeep));
        }
        self.want = if array { Want::Element } else { Want::Key };
        Ok(Move::Demand(self.demand()))
    }
    fn key(&mut self, event: Event) -> Result<Move, Malformed> {
        match event {
            Event::Token(Token::ObjectEnd) => self.end(),
            Event::Long(_) if self.open.top().expect("object key").node == Role::Document(Node::AppPermissions) => {
                Err(Malformed::Bound { field: b"permissions" })
            }
            Event::Token(Token::Key(key)) => {
                let frame = self.open.top_mut().expect("object key");
                if frame.node == Role::Document(Node::AppPermissions) {
                    if key.len() > usize::try_from(self.limits.string_bytes).expect("u32 fits usize") {
                        return Err(Malformed::Bound { field: b"permissions" });
                    }
                    frame.seen = frame.seen.checked_add(1).ok_or(Malformed::Bound { field: b"permissions" })?;
                    if frame.seen > self.limits.inner {
                        return Err(Malformed::Bound { field: b"permissions" });
                    }
                    self.want = Want::AppAccess;
                    return Ok(Move::Scalar(Node::AppPermissions, b"name", Token::String(key)));
                }
                for (index, field) in fields(frame.node).iter().enumerate() {
                    if field.key == key.as_ref() {
                        let bit = 1u32.checked_shl(u32::try_from(index).expect("small table")).expect("field bit");
                        if frame.touched & bit != 0 {
                            return Err(Malformed::Shape { field: field.key });
                        }
                        frame.touched |= bit;
                        self.want = Want::Value(*field, bit);
                        return Ok(Move::Demand(match field.read {
                            Read::Login => Request::Text(super::super::delivery::OWNER_BYTES),
                            Read::Name => Request::Text(super::super::delivery::NAME_BYTES),
                            Read::Reason => Request::Text(self.limits.json.string),
                            Read::Text | Read::Secret | Read::LongText => Request::Text(self.limits.string_bytes),
                            Read::Body => Request::Text(self.limits.body_bytes.min(self.limits.string_bytes)),
                            Read::LongBody => Request::Text(self.limits.body_bytes),
                            Read::Presence => Request::Text(0),
                            Read::Payload | Read::Number | Read::Boolean | Read::Container(_) => Request::Next,
                        }));
                    }
                }
                self.want = Want::Unknown(0);
                Ok(Move::Demand(Request::Text(0)))
            }
            Event::Token(
                Token::ObjectStart
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            )
            | Event::Long(_)
            | Event::Skipped(_)
            | Event::Done
            | Event::Failed(_)
            | Event::Closed => Err(Malformed::Shape { field: b"" }),
        }
    }
    #[expect(clippy::too_many_lines, reason = "the field parser exhaustively handles each read kind and token")]
    fn value(&mut self, field: Field, bit: u32, event: Event) -> Result<Move, Malformed> {
        if event == Event::Token(Token::Null) {
            self.want = Want::Key;
            return Ok(Move::Demand(Request::Next));
        }
        let frame = self.open.top_mut().expect("field value");
        frame.seen |= bit;
        let node = frame.node;
        match field.read {
            Read::Payload => {
                self.want = Want::Key;
                return if event == Event::Token(Token::ObjectStart) {
                    Ok(Move::PayloadStart(Token::ObjectStart))
                } else {
                    Err(Malformed::Shape { field: field.key })
                };
            }
            Read::Container(child) => return self.start(child, &event),
            Read::Presence => {
                self.unknown(&event, 0)?;
                return Ok(scalar(node, field.key, Token::True));
            }
            Read::Text
            | Read::Login
            | Read::Name
            | Read::Secret
            | Read::Body
            | Read::LongBody
            | Read::LongText
            | Read::Reason
            | Read::Number
            | Read::Boolean => {}
        }
        let wrong = Malformed::Shape { field: field.key };
        let token = match event {
            Event::Token(token) => token,
            Event::Long(length) => {
                if field.read == Read::LongBody || field.read == Read::LongText {
                    self.want = Want::Key;
                    return Ok(Move::BodyLong(
                        match node {
                            Role::Document(node) => Some(node),
                            Role::Payload(_) => None,
                        },
                        length,
                    ));
                }
                return Err(if field.read == Read::Secret {
                    Malformed::Bound { field: field.key }
                } else {
                    Malformed::TooLong { field: field.key }
                });
            }
            Event::Skipped(_) | Event::Done | Event::Failed(_) | Event::Closed => return Err(wrong),
        };
        let valid = match (&field.read, &token) {
            (
                Read::Text
                | Read::Login
                | Read::Name
                | Read::Secret
                | Read::Body
                | Read::LongBody
                | Read::LongText
                | Read::Reason,
                Token::String(text),
            ) => {
                if field.read != Read::LongBody
                    && field.read != Read::Reason
                    && field.read != Read::Login
                    && field.read != Read::Name
                    && text.len() > usize::try_from(self.limits.string_bytes).expect("u32 fits usize")
                {
                    return Err(if field.read == Read::Secret {
                        Malformed::Bound { field: field.key }
                    } else {
                        Malformed::TooLong { field: field.key }
                    });
                }
                let bound = match field.read {
                    Read::Login => Some(super::super::delivery::OWNER_BYTES),
                    Read::Name => Some(super::super::delivery::NAME_BYTES),
                    Read::Text
                    | Read::Secret
                    | Read::Presence
                    | Read::Body
                    | Read::LongBody
                    | Read::LongText
                    | Read::Reason
                    | Read::Number
                    | Read::Boolean
                    | Read::Payload
                    | Read::Container(_) => None,
                };
                if let Some(bound) = bound
                    && text.len() > usize::try_from(bound).expect("path bound fits usize")
                {
                    return Err(Malformed::TooLong { field: field.key });
                }
                true
            }
            (Read::Number, Token::Number(text)) => {
                if crate::forgejo::id::unsigned(text).is_none() {
                    return Err(Malformed::Range { field: field.key });
                }
                true
            }
            (Read::Boolean, Token::True | Token::False) => true,
            (
                Read::Text
                | Read::Login
                | Read::Name
                | Read::Secret
                | Read::Presence
                | Read::Body
                | Read::LongBody
                | Read::LongText
                | Read::Reason
                | Read::Number
                | Read::Boolean
                | Read::Payload
                | Read::Container(_),
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
            ) => false,
        };
        if !valid {
            return Err(wrong);
        }
        self.want = Want::Key;
        Ok(scalar(node, field.key, token))
    }
    fn app_text(&self, event: Event, field: &'static [u8]) -> Result<Token, Malformed> {
        match event {
            Event::Token(Token::String(text)) => {
                if text.len() > usize::try_from(self.limits.string_bytes).expect("u32 fits usize") {
                    return Err(Malformed::Bound { field });
                }
                Ok(Token::String(text))
            }
            Event::Long(_) => Err(Malformed::Bound { field }),
            Event::Token(
                Token::ObjectStart
                | Token::ObjectEnd
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::Key(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            )
            | Event::Skipped(_)
            | Event::Done
            | Event::Failed(_)
            | Event::Closed => Err(Malformed::Shape { field }),
        }
    }
    fn unknown(&mut self, event: &Event, depth: u32) -> Result<Move, Malformed> {
        let depth = match event {
            Event::Token(Token::ObjectStart | Token::ArrayStart) => {
                depth.checked_add(1).ok_or(Malformed::Json(tokenizer::Error::TooDeep))?
            }
            Event::Token(Token::ObjectEnd | Token::ArrayEnd) => {
                depth.checked_sub(1).ok_or(Malformed::Shape { field: b"" })?
            }
            Event::Token(Token::Key(_)) => {
                self.want = Want::Unknown(depth);
                return Ok(Move::Demand(Request::Text(0)));
            }
            Event::Token(Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null)
            | Event::Long(_) => depth,
            Event::Skipped(_) | Event::Done | Event::Failed(_) | Event::Closed => {
                return Err(Malformed::Shape { field: b"" });
            }
        };
        if self.open.len().saturating_add(depth) > DEPTH {
            return Err(Malformed::Json(tokenizer::Error::TooDeep));
        }
        if depth == 0 {
            self.want = if element(self.open.top().expect("unknown parent").node).is_some() {
                Want::Element
            } else {
                Want::Key
            };
            Ok(Move::Demand(Request::Next))
        } else {
            self.want = Want::Unknown(depth);
            Ok(Move::Demand(Request::Text(0)))
        }
    }
    fn end(&mut self) -> Result<Move, Malformed> {
        let frame = self.open.pop().expect("container end");
        for (index, field) in fields(frame.node).iter().enumerate() {
            let bit = 1u32.checked_shl(u32::try_from(index).expect("small table")).expect("field bit");
            if field.required && (if field.nullable { frame.touched } else { frame.seen }) & bit == 0 {
                return Err(Malformed::Missing { field: field.key });
            }
        }
        self.want = match self.open.top() {
            None => Want::Done,
            Some(frame) => {
                if element(frame.node).is_some() {
                    Want::Element
                } else {
                    Want::Key
                }
            }
        };
        Ok(match frame.node {
            Role::Document(node) => Move::End(node),
            Role::Payload(_) => Move::PayloadEnd,
        })
    }
}

fn scalar(role: Role, key: &'static [u8], token: Token) -> Move {
    match role {
        Role::Document(node) => Move::Scalar(node, key, token),
        Role::Payload(node) => Move::PayloadScalar(node, key, token),
    }
}
fn fields(role: Role) -> &'static [Field] {
    match role {
        Role::Document(node) => document_fields(node),
        Role::Payload(node) => payload_fields(node),
    }
}
const fn child(key: &'static [u8], node: Payload) -> Field {
    field(key, Read::Container(Role::Payload(node)))
}
/// The common action/object/repository shape used by item and CI events.
const fn action_fields(key: &'static [u8], node: Payload) -> [Field; 3] {
    [field(b"action", Read::Text), child(key, node), child(b"repository", Payload::Repository)]
}
fn payload_fields(node: Payload) -> &'static [Field] {
    match node {
        Payload::Root(Hook::Issue) => const { &action_fields(b"issue", Payload::Item) },
        Payload::Root(Hook::Comment) => {
            const {
                &[
                    field(b"action", Read::Text),
                    child(b"issue", Payload::Number),
                    child(b"comment", Payload::Body),
                    child(b"repository", Payload::Repository),
                ]
            }
        }
        Payload::Root(Hook::Pull) => {
            const {
                &[
                    field(b"action", Read::Text),
                    field(b"number", Read::Number),
                    optional(b"reason", Read::Text),
                    child(b"repository", Payload::Repository),
                ]
            }
        }
        Payload::Root(Hook::Review) => {
            const {
                &[
                    field(b"action", Read::Text),
                    child(b"pull_request", Payload::Number),
                    child(b"review", Payload::Body),
                    child(b"repository", Payload::Repository),
                ]
            }
        }
        Payload::Root(Hook::Remark) => const { &action_fields(b"pull_request", Payload::Number) },
        Payload::Root(Hook::Push) => {
            const {
                &[
                    field(b"ref", Read::Text),
                    field(b"after", Read::Text),
                    field(b"deleted", Read::Boolean),
                    child(b"repository", Payload::Repository),
                    child(b"sender", Payload::Sender),
                ]
            }
        }
        Payload::Root(Hook::Ref(_)) => {
            const { &[field(b"ref", Read::Text), field(b"ref_type", Read::Text), child(b"repository", Payload::Repository)] }
        }
        Payload::Root(Hook::Ci(event)) => ci_fields(event),
        Payload::Root(Hook::Installation | Hook::Repositories) => {
            const { &[field(b"action", Read::Text), child(b"installation", Payload::Installation)] }
        }
        Payload::Root(Hook::Target) => {
            const {
                &[
                    field(b"action", Read::Text),
                    child(b"installation", Payload::InstallationId),
                    child(b"account", Payload::Account),
                ]
            }
        }
        Payload::Repository => {
            const {
                &[
                    field(b"id", Read::Number),
                    optional(b"name", Read::Name),
                    optional(b"owner", Read::Container(Role::Payload(Payload::RepositoryOwner))),
                ]
            }
        }
        Payload::RepositoryOwner => const { &[optional(b"login", Read::Login)] },
        Payload::Sender | Payload::InstallationId => const { &[field(b"id", Read::Number)] },
        Payload::Item => const { &[field(b"number", Read::Number), optional(b"body", Read::LongBody)] },
        Payload::Number => const { &[field(b"number", Read::Number)] },
        Payload::Body => const { &[optional(b"body", Read::LongBody)] },
        Payload::Head => const { &[field(b"head_sha", Read::Text)] },
        // Target puts the account beside the installation, the other two events inside it.
        Payload::Installation => const { &[field(b"id", Read::Number), child(b"account", Payload::Account)] },
        Payload::Account => const { &[field(b"id", Read::Number), field(b"login", Read::Text)] },
    }
}

fn ci_fields(event: Ci) -> &'static [Field] {
    match event {
        Ci::CheckRun => const { &action_fields(b"check_run", Payload::Head) },
        Ci::CheckSuite => const { &action_fields(b"check_suite", Payload::Head) },
        Ci::WorkflowJob => const { &action_fields(b"workflow_job", Payload::Head) },
        Ci::WorkflowRun => const { &action_fields(b"workflow_run", Payload::Head) },
        Ci::ChecksRequested | Ci::Destroyed => const { &action_fields(b"merge_group", Payload::Head) },
        Ci::Status => {
            const { &[field(b"sha", Read::Text), field(b"state", Read::Text), child(b"repository", Payload::Repository)] }
        }
    }
}
