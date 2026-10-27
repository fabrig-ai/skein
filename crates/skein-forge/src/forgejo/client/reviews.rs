use super::{Encoded, Refusal, items::Target};
use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Field, Kind, Move, Read, Value, Walk},
    },
    document::{self, Body, Document, Review, ReviewComment, ReviewState, Row, User},
    limits::Limits,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_lib::Writer;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReviewKind {
    Review,
    Reviews,
    Comment,
    Comments,
}
const fn field(key: &'static [u8], read: Read, required: bool) -> Field {
    Field { key, read, required }
}
pub(crate) fn fields(kind: ReviewKind) -> &'static [Field] {
    match kind {
        ReviewKind::Review => {
            const {
                &[
                    field(b"id", Read::Number, true),
                    field(b"user", Read::Object(Kind::User), true),
                    field(b"state", Read::Text, true),
                    field(b"commit_id", Read::Id { empty: false }, true),
                    field(b"body", Read::Body, true),
                    field(b"submitted_at", Read::Time, false),
                    field(b"updated_at", Read::Time, false),
                    field(b"official", Read::Boolean, true),
                    field(b"dismissed", Read::Boolean, true),
                ]
            }
        }
        ReviewKind::Comment => {
            const {
                &[
                    field(b"id", Read::Number, true),
                    field(b"user", Read::Object(Kind::User), true),
                    field(b"path", Read::Text, true),
                    field(b"position", Read::Number, true),
                    field(b"body", Read::Body, true),
                ]
            }
        }
        ReviewKind::Reviews | ReviewKind::Comments => &[],
    }
}
pub(crate) fn element(kind: ReviewKind) -> Option<Kind> {
    match kind {
        ReviewKind::Reviews => Some(Kind::Review(ReviewKind::Review)),
        ReviewKind::Comments => Some(Kind::Review(ReviewKind::Comment)),
        ReviewKind::Review | ReviewKind::Comment => None,
    }
}
pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let mut measure = Target { length: 0, writer: None };
    target(request, &mut measure);
    if measure.length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let mut write = Target { length: 0, writer: Some(Writer::new(measure.length)) };
    target(request, &mut write);
    Ok(Encoded { method: Method::Get, target: write.writer.expect("writing pass").finish(), body: None })
}
#[expect(
    clippy::too_many_lines,
    reason = "exhaustive request selection lists every setup variant beside its owning group"
)]
fn target(request: &Request, out: &mut Target) {
    match request {
        Request::Reviews { at, number, page } => {
            out.numbered(at, b"/pulls/", *number);
            out.put(b"/reviews");
            out.page(*page, false);
        }
        Request::ReviewComments { at, number, review } => {
            out.numbered(at, b"/pulls/", *number);
            out.put(b"/reviews/");
            out.number(*review);
            out.put(b"/comments");
        }
        Request::Branch { at, name } => {
            out.repo(at);
            out.put(b"/branches/");
            out.segment(name);
        }
        Request::Protection { at, name } => {
            out.repo(at);
            out.put(b"/branch_protections/");
            out.segment(name);
        }
        Request::Branches { at, prefix: Some(prefix), .. } => {
            out.repo(at);
            out.put(b"/git/refs/heads/");
            out.segment(prefix);
        }
        Request::Branches { at, prefix: None, page } => {
            out.repo(at);
            out.put(b"/branches");
            out.page(*page, false);
        }
        Request::Collaborators { at, page } => {
            out.repo(at);
            out.put(b"/collaborators");
            out.page(*page, false);
        }
        Request::Protections { at } => {
            out.repo(at);
            out.put(b"/branch_protections");
        }
        Request::Permission { at, login } => {
            out.repo(at);
            out.put(b"/collaborators/");
            out.segment(login);
            out.put(b"/permission");
        }
        Request::Pulls { at, page } => {
            out.repo(at);
            out.put(b"/pulls?state=open&sort=oldest");
            out.page(*page, true);
        }
        Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
        | Request::Repository { .. }
        | Request::UserRepos { .. }
        | Request::RepoSearch { .. }
        | Request::UserSearch { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::CombinedStatus { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::Org { .. }
        | Request::CreateOrg { .. }
        | Request::CreateFile { .. }
        | Request::CreateRepository { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::AddMember { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::CreateUser { .. }
        | Request::DeleteBranch { .. } => unreachable!("review encoder selection"),
    }
}
pub(crate) fn state(text: &[u8]) -> Option<ReviewState> {
    match text {
        b"APPROVED" => Some(ReviewState::Approved),
        b"REQUEST_CHANGES" => Some(ReviewState::RequestChanges),
        b"COMMENT" => Some(ReviewState::Comment),
        b"PENDING" => Some(ReviewState::Pending),
        b"REQUEST_REVIEW" => Some(ReviewState::RequestReview),
        _ => None,
    }
}
fn review() -> Review {
    Review {
        id: 0,
        user: document::user(),
        state: ReviewState::Pending,
        commit_id: crate::forgejo::id::ObjectId { format: crate::forgejo::id::ObjectFormat::Sha1, bytes: [0; 32] },
        body: Body::Kept(Box::from([])),
        submitted_at: None,
        updated_at: None,
        official: false,
        dismissed: false,
    }
}
fn comment() -> ReviewComment {
    ReviewComment { id: 0, user: document::user(), path: Box::from([]), position: 0, body: Body::Kept(Box::from([])) }
}
#[derive(Debug)]
pub(super) struct Reviews {
    single: bool,
    user: User,
    review: Review,
    comment: ReviewComment,
}
impl Reviews {
    pub(super) fn new(kind: ReviewKind) -> Self {
        Self { single: kind == ReviewKind::Review, user: document::user(), review: review(), comment: comment() }
    }
    pub(super) fn step(&mut self, movement: Move, walk: &Walk) -> Step {
        let result = match movement {
            Move::Demand(demand) => return Step::Demand(demand),
            Move::Scalar { kind, key, value } => self.scalar(kind, key, value),
            Move::End(Kind::User) => {
                let user = core::mem::replace(&mut self.user, document::user());
                if walk.parent() == Some(Kind::Review(ReviewKind::Review)) {
                    self.review.user = user;
                } else {
                    self.comment.user = user;
                }
                Ok(())
            }
            Move::End(Kind::Review(kind)) => match kind {
                ReviewKind::Review => {
                    if walk.parent().is_none() {
                        return Step::Demand(skein_json::tokenizer::Request::Next);
                    }
                    return Step::Row(
                        Row::Review(core::mem::replace(&mut self.review, review())),
                        skein_json::tokenizer::Request::Next,
                    );
                }
                ReviewKind::Comment => {
                    return Step::Row(
                        Row::ReviewComment(core::mem::replace(&mut self.comment, comment())),
                        skein_json::tokenizer::Request::Next,
                    );
                }
                ReviewKind::Reviews | ReviewKind::Comments => Ok(()),
            },
            Move::End(
                Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_)
                | Kind::Delivery(_)
                | Kind::Branch(_),
            ) => unreachable!("review tables"),
            Move::Done => {
                return Step::Done(if self.single {
                    Document::Review(core::mem::replace(&mut self.review, review()))
                } else {
                    Document::Listed { skipped: walk.skipped() }
                });
            }
        };
        match result {
            Ok(()) => Step::Demand(skein_json::tokenizer::Request::Next),
            Err(error) => Step::Malformed(error),
        }
    }
    fn scalar(&mut self, kind: Kind, key: &'static [u8], value: Value) -> Result<(), Malformed> {
        match value {
            Value::Input(_) => unreachable!("dispatch input table"),
            Value::Number(n) => match key {
                b"id" => {
                    if kind == Kind::User {
                        self.user.id = n;
                    } else if kind == Kind::Review(ReviewKind::Review) {
                        self.review.id = n;
                    } else {
                        self.comment.id = n;
                    }
                }
                b"position" => {
                    self.comment.position = match u32::try_from(n) {
                        Ok(n) => n,
                        Err(_) => return Err(Malformed::Range { field: key }),
                    }
                }
                _ => unreachable!("review number table"),
            },
            Value::Text(text) => match key {
                b"login" => self.user.login = text,
                b"path" => self.comment.path = text,
                b"state" => self.review.state = state(&text).ok_or(Malformed::Value { field: key })?,
                _ => unreachable!("review text table"),
            },
            Value::Body(body) => {
                if kind == Kind::Review(ReviewKind::Review) {
                    self.review.body = body;
                } else {
                    self.comment.body = body;
                }
            }
            Value::Time(time) => {
                if key == b"submitted_at" {
                    self.review.submitted_at = Some(time);
                } else {
                    self.review.updated_at = Some(time);
                }
            }
            Value::Id(id) => self.review.commit_id = id.expect("required commit id"),
            Value::Boolean(value) => {
                if key == b"official" {
                    self.review.official = value;
                } else {
                    self.review.dismissed = value;
                }
            }
        }
        Ok(())
    }
}
