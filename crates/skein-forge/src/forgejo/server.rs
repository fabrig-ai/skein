//! The fake forge's side: origin targets and JSON answers.
use crate::forgejo::{
    document::{
        ApiSettings, Branch, BranchProtection, CollaboratorPermission, CombinedStatus, Comment, CommitStatus, Issue,
        PullRequest, Repository, Review, ReviewComment, User,
    },
    limits::Limits,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_json::writer;
mod actions;
mod branches;
pub mod delivery;
mod files;
mod hooks;
mod items;
mod orgs;
mod reviews;
mod setup;
mod start;
pub(crate) mod writes;
use crate::forgejo::decode::Malformed;
use skein_json::tokenizer;
pub use writes::{BodyDecoder, body, body_first};
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum BodyStep {
    Demand(tokenizer::Request),
    Decoded(Request),
    Malformed(Malformed),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Unrouted {
    NotFound,
    Method,
    Malformed,
}
#[derive(Debug)]
pub enum Route {
    Ready(Request),
    Body(BodyDecoder),
}
pub fn route(method: Method, target: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    match hooks::repository_route(method, target, limits) {
        Err(Unrouted::NotFound) => {}
        other => return other,
    }
    match writes::route(method, target, limits) {
        Err(Unrouted::NotFound) => match read_route(method, target, limits) {
            Err(Unrouted::NotFound) => match setup::route(method, target, limits) {
                Err(Unrouted::NotFound) => match orgs::route(method, target, limits) {
                    Err(Unrouted::NotFound) => hooks::route(method, target, limits),
                    other => other,
                },
                other => other,
            },
            other => other,
        },
        other => other,
    }
}
fn read_route(method: Method, target: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    match start::route(method, target, limits) {
        Err(Unrouted::NotFound) => match reviews::route(method, target, limits) {
            Err(Unrouted::NotFound) => match branches::route(method, target, limits) {
                Err(Unrouted::NotFound) => match files::route(method, target, limits) {
                    Err(Unrouted::NotFound) => match actions::route(method, target, limits) {
                        Err(Unrouted::NotFound) => items::route(method, target, limits),
                        other => other,
                    },
                    other => other,
                },
                other => other,
            },
            other => other,
        },
        other => other,
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Answer {
    Artifacts(Box<[crate::forgejo::document::Artifact]>),
    /// A comparison with full commit metadata, newest first as Forgejo sends it.
    Commits {
        commits: Box<[crate::forgejo::document::CommitMessage]>,
    },
    /// Encodes a page without filtering rows whose push permission is false.
    Reachable(Box<[crate::forgejo::document::Reachable]>),
    Hook(crate::forgejo::document::Hook),
    Hooks(Box<[crate::forgejo::document::Hook]>),
    Token(crate::forgejo::document::AccessToken),
    Tokens(Box<[crate::forgejo::document::AccessToken]>),
    Org(crate::forgejo::document::Org),
    Team(crate::forgejo::document::Team),
    Teams(Box<[crate::forgejo::document::Team]>),
    Members(Box<[User]>),
    User(User),
    ApiSettings(ApiSettings),
    Repository(Repository),
    CreatedRepository(crate::forgejo::document::CreatedRepository),
    CreatedFile {
        commit: crate::forgejo::id::ObjectId,
    },
    /// Encodes an admin search page, whose rows are all writable by the admin.
    RepoSearch {
        ok: bool,
        data: Box<[crate::forgejo::document::Reachable]>,
    },
    Search {
        ok: bool,
        data: Box<[User]>,
    },
    Issues(Box<[Issue]>),
    Issue(Issue),
    Comments(Box<[Comment]>),
    Comment(Comment),
    Status(CommitStatus),
    Review(Review),
    Error(crate::forgejo::document::ErrorBody),
    Pull(PullRequest),
    Files(Box<[crate::forgejo::document::ChangedFile]>),
    Compare {
        compare: crate::forgejo::document::Compare,
        commits: Box<[crate::forgejo::id::ObjectId]>,
        files: Box<[crate::forgejo::document::ChangedFile]>,
    },
    Tree {
        entries: Box<[crate::forgejo::document::Entry]>,
        truncated: bool,
    },
    Runs {
        total_count: u64,
        runs: Box<[crate::forgejo::document::Run]>,
    },
    Run(crate::forgejo::document::Run),
    Job {
        job: crate::forgejo::document::Job,
        steps: Box<[crate::forgejo::document::Step]>,
    },
    Jobs(Box<[crate::forgejo::document::Job]>),
    Pulls(Box<[PullRequest]>),
    Reviews(Box<[Review]>),
    ReviewComments(Box<[ReviewComment]>),
    Branch(Branch),
    Branches(Box<[Branch]>),
    BranchRefs(Box<[Branch]>),
    Protection(BranchProtection),
    Protections(Box<[BranchProtection]>),
    Collaborators(Box<[User]>),
    Permission(CollaboratorPermission),
    CombinedStatus {
        state: CombinedStatus,
        sha: Option<crate::forgejo::id::ObjectId>,
        statuses: Box<[CommitStatus]>,
    },
}
pub fn encode(answer: &Answer, limits: &Limits) -> Result<Box<[u8]>, writer::Refusal> {
    let mut measure = writer::Encoder::measure(&limits.writer);
    write(&mut measure, answer)?;
    let length = measure.measured()?;
    let mut write = writer::Encoder::write(length, &limits.writer);
    self::write(&mut write, answer)?;
    Ok(write.finish())
}
fn write(json: &mut writer::Encoder, answer: &Answer) -> Result<(), writer::Refusal> {
    match answer {
        Answer::CreatedFile { commit } => {
            json.object_start();
            json.key(b"commit");
            json.object_start();
            json.key(b"sha");
            json.string(&crate::forgejo::id::write(commit));
            json.object_end();
            json.object_end();
            Ok(())
        }
        Answer::CreatedRepository(value) => {
            crate::forgejo::client::repositories::write(json, value);
            Ok(())
        }
        Answer::Token(value) => {
            setup::token(json, value);
            Ok(())
        }
        Answer::Tokens(tokens) => {
            json.array_start();
            for token in tokens {
                setup::token(json, token);
            }
            json.array_end();
            Ok(())
        }
        Answer::Hook(_) | Answer::Hooks(_) => {
            hooks::write(json, answer);
            Ok(())
        }
        Answer::Org(_) | Answer::Team(_) | Answer::Teams(_) | Answer::Members(_) => {
            orgs::write(json, answer);
            Ok(())
        }
        Answer::Error(value) => {
            json.object_start();
            json.key(b"message");
            match &value.message {
                crate::forgejo::document::Body::Kept(message) => json.string(message),
                crate::forgejo::document::Body::Long(_) => return Err(writer::Refusal::TooLong),
            }
            json.object_end();
            Ok(())
        }
        Answer::Comment(value) => items::comment(json, value),
        Answer::Status(value) => {
            items::commit_status(json, value);
            Ok(())
        }
        Answer::Review(value) => reviews::review(json, value),
        Answer::Commits { .. } | Answer::Tree { .. } | Answer::Files(_) | Answer::Compare { .. } => {
            files::write(json, answer);
            Ok(())
        }
        Answer::Artifacts(_) | Answer::Runs { .. } | Answer::Run(_) | Answer::Job { .. } | Answer::Jobs(_) => {
            actions::write(json, answer);
            Ok(())
        }
        Answer::Pulls(_) | Answer::Reviews(_) | Answer::ReviewComments(_) => reviews::write(json, answer),
        Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Permission(_) => {
            branches::write(json, answer);
            Ok(())
        }
        Answer::Reachable(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. } => {
            start::write(json, answer);
            Ok(())
        }
        Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::CombinedStatus { .. } => items::write(json, answer),
    }
}
