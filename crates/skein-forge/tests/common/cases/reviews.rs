#![expect(clippy::disallowed_macros, reason = "the document harness formats fixture values and literal expected rows")]
#![expect(
    clippy::disallowed_methods,
    reason = "the document harness transforms finite fixture rows with ordinary Rust iterators"
)]
#![expect(
    clippy::disallowed_types,
    reason = "the document test harness retains finite fixture rows and mutation buffers in ordinary Rust collections"
)]
#![expect(
    clippy::must_use_candidate,
    reason = "fixture-returning helpers are only consumed by assertions or other fixture construction"
)]
use super::super::forgejo;
use skein_forge::forgejo::{
    document::{
        Body, Branch, BranchProtection, CollaboratorPermission, CombinedStatus, Document, Permission, Review,
        ReviewComment, ReviewState, Row,
    },
    request::Request,
    server::Answer,
};
use skein_lib::Wall;
pub fn review(id: u64, state: ReviewState, official: bool, dismissed: bool, key: &[u8]) -> Review {
    Review {
        id,
        user: forgejo::user(2),
        state,
        commit_id: forgejo::sha(4),
        body: Body::Kept(Box::from(key)),
        submitted_at: Some(Wall::from_nanos(1_577_817_005_000_000_000)),
        updated_at: Some(Wall::from_nanos(1_577_817_005_000_000_000)),
        official,
        dismissed,
    }
}
pub fn branch(name: &[u8], sha: u64) -> Branch {
    Branch { name: Box::from(name), commit: forgejo::sha(sha), effective_branch_protection_name: None }
}
pub fn protection() -> BranchProtection {
    BranchProtection {
        rule_name: Box::from(*b"main"),
        enable_status_check: true,
        status_check_contexts: Box::from([Box::from(b"fixture/test".as_slice())]),
        required_approvals: 1,
        dismiss_stale_approvals: true,
        require_signed_commits: false,
    }
}
pub fn cases() -> Vec<(&'static str, Request, Option<Answer>)> {
    vec![
        (
            "reviews",
            Request::Reviews { at: forgejo::specimen(), number: 1, page: forgejo::page(1, 2) },
            Some(Answer::Reviews(Box::from([
                review(1, ReviewState::Approved, false, true, b"<!-- kit:key 01000003 -->\nreview text"),
                review(2, ReviewState::RequestChanges, true, false, b"<!-- kit:key 01000004 -->\nreview text"),
            ]))),
        ),
        (
            "statuses-page-2",
            Request::CombinedStatus { at: forgejo::specimen(), commit: forgejo::sha(4), page: forgejo::page(2, 2) },
            Some(Answer::CombinedStatus {
                state: CombinedStatus { state: None, total_count: 0 },
                sha: None,
                statuses: Box::from([]),
            }),
        ),
        (
            "review-comments",
            Request::ReviewComments { at: forgejo::specimen(), number: 1, review: 5 },
            Some(Answer::ReviewComments(Box::from([ReviewComment {
                id: 18,
                user: forgejo::user(1),
                path: Box::from(*b"file-0.txt"),
                position: 1,
                body: Body::Kept(Box::from(*b"remark text")),
            }]))),
        ),
        (
            "branch",
            Request::Branch { at: forgejo::specimen(), name: Box::from(*b"main") },
            Some(Answer::Branch(branch(b"main", 3))),
        ),
        (
            "branch-with-slash",
            Request::Branch { at: forgejo::specimen(), name: Box::from(*b"feature/slash") },
            Some(Answer::Branch(branch(b"feature/slash", 4))),
        ),
        (
            "branches-page-1",
            Request::Branches { at: forgejo::specimen(), prefix: None, page: forgejo::page(1, 2) },
            Some(Answer::Branches(Box::from([branch(b"landing", 7), branch(b"mergeable", 5)]))),
        ),
        (
            "branches-page-2",
            Request::Branches { at: forgejo::specimen(), prefix: None, page: forgejo::page(2, 2) },
            Some(Answer::Branches(Box::from([branch(b"writer-head", 9), branch(b"conflict", 10)]))),
        ),
        (
            "branch-protection",
            Request::Protection { at: forgejo::specimen(), name: Box::from(*b"main") },
            Some(Answer::Protection(protection())),
        ),
        (
            "branch-protections",
            Request::Protections { at: forgejo::specimen() },
            Some(Answer::Protections(Box::from([protection()]))),
        ),
        (
            "collaborators",
            Request::Collaborators { at: forgejo::specimen(), page: forgejo::page(1, 2) },
            Some(Answer::Collaborators(Box::from([forgejo::user(2)]))),
        ),
        (
            "collaborator-permission",
            Request::Permission { at: forgejo::specimen(), login: Box::from(*b"writer") },
            Some(Answer::Permission(CollaboratorPermission { permission: Permission::Write, user: forgejo::user(2) })),
        ),
        ("owner-permission", Request::Permission { at: forgejo::specimen(), login: Box::from(*b"fixture") }, None),
    ]
}
pub fn expected(answer: Answer) -> (Document, Vec<Row>) {
    match answer {
        Answer::Reviews(v) => (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::Review).collect()),
        Answer::ReviewComments(v) => {
            (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::ReviewComment).collect())
        }
        Answer::BranchRefs(v) | Answer::Branches(v) => {
            (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::Branch).collect())
        }
        Answer::Protections(v) => {
            (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::Protection).collect())
        }
        Answer::Collaborators(v) => {
            (Document::Listed { skipped: 0 }, v.into_vec().into_iter().map(Row::User).collect())
        }
        Answer::Branch(v) => (Document::Branch(v), vec![]),
        Answer::Protection(v) => (Document::Protection(v), vec![]),
        Answer::Permission(v) => (Document::Permission(v), vec![]),
        Answer::CombinedStatus { state, statuses, .. } => {
            (Document::CombinedStatus(state), statuses.into_vec().into_iter().map(Row::Status).collect())
        }
        Answer::Org(_)
        | Answer::CreatedRepository(_)
        | Answer::CreatedFile { .. }
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Files(_)
        | Answer::Compare { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Job { .. }
        | Answer::Jobs(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::Tree { .. }
        | Answer::Pulls(_) => panic!("review family answer"),
    }
}
