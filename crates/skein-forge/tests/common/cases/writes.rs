#![expect(clippy::disallowed_macros, reason = "the document harness formats fixture values and literal expected rows")]
#![expect(
    clippy::disallowed_types,
    reason = "the document test harness retains finite fixture rows and mutation buffers in ordinary Rust collections"
)]
#![expect(
    clippy::must_use_candidate,
    reason = "fixture-returning helpers are only consumed by assertions or other fixture construction"
)]
use super::super::forgejo;
use skein_forge::forgejo::failure::Refusal;
use skein_forge::forgejo::{
    document::{
        Body, Branch, Comment, CommitStatus, Document, Issue, IssueState, PullBranch, PullRequest, Review, ReviewState,
        StatusState,
    },
    server::Answer,
};
use skein_lib::Wall;
pub fn answers() -> Vec<(&'static str, Answer)> {
    let mut rows = vec![];
    for (case, title, state, at) in [
        ("create-issue", b"fixture issue".as_slice(), IssueState::Open, Wall::from_nanos(1_577_817_004_000_000_000)),
        ("edit-issue-title", b"edited title", IssueState::Open, Wall::from_nanos(1_577_817_005_000_000_000)),
        ("close-issue", b"edited title", IssueState::Closed, Wall::from_nanos(1_577_817_005_000_000_000)),
        ("reopen-issue", b"edited title", IssueState::Open, Wall::from_nanos(1_577_817_005_000_000_000)),
    ] {
        rows.push((
            case,
            Answer::Issue(Issue {
                number: 3,
                pull: false,
                state,
                user: forgejo::user(2),
                labels: Box::from([]),
                updated_at: at,
                title: Box::from(title),
                body: Body::Kept(Box::from(*b"<!-- kit:key 01000001 -->\nissue text")),
            }),
        ));
    }
    for (case, number, head, head_sha, base, base_sha) in
        [("open-pull", 4, "writer-head", 2, "landing", 1), ("open-pull-no-changes", 6, "no-changes", 3, "main", 3)]
    {
        rows.push((
            case,
            Answer::Pull(PullRequest {
                number,
                state: IssueState::Open,
                head: PullBranch { name: Box::from(head.as_bytes()), sha: Some(forgejo::sha(head_sha)), repo_id: 1 },
                base: PullBranch { name: Box::from(base.as_bytes()), sha: Some(forgejo::sha(base_sha)), repo_id: 1 },
                merged: false,
                merge_commit_sha: None,
                merge_base: Some(forgejo::sha(3)),
                mergeable: true,
                requested_reviewers: Box::from([]),
            }),
        ));
    }
    rows.extend(posted_answers());
    rows
}
pub fn posted_answers() -> Vec<(&'static str, Answer)> {
    let at = Wall::from_nanos(1_577_817_005_000_000_000);
    let mut rows = vec![];
    for (case, id, body) in
        [("post-comment", 4, "<!-- kit:key 01000002 -->\ncomment text"), ("post-comment-plain", 5, "plain comment")]
    {
        rows.push((
            case,
            Answer::Comment(Comment {
                id,
                user: forgejo::user(2),
                created_at: Some(at),
                updated_at: Some(at),
                body: Body::Kept(Box::from(body.as_bytes())),
            }),
        ));
    }
    for (case, id, state, official, body) in [
        ("post-review-approved", 1, ReviewState::Approved, true, "<!-- kit:key 01000003 -->\nreview text"),
        ("post-review-request-changes", 2, ReviewState::RequestChanges, true, "<!-- kit:key 01000004 -->\nreview text"),
        ("post-review-comment", 3, ReviewState::Comment, false, "<!-- kit:key 01000005 -->\nreview text"),
    ] {
        rows.push((
            case,
            Answer::Review(Review {
                id,
                user: forgejo::user(2),
                state,
                commit_id: forgejo::sha(4),
                body: Body::Kept(Box::from(body.as_bytes())),
                submitted_at: Some(at),
                updated_at: Some(at),
                official,
                dismissed: false,
            }),
        ));
    }
    for (case, context, status) in [
        ("post-status", b"fixture/test".as_slice(), StatusState::Success),
        ("post-status-pending", b"fixture/other", StatusState::Pending),
    ] {
        rows.push((
            case,
            Answer::Status(CommitStatus {
                creator: Some(forgejo::user(2)),
                updated_at: Wall::from_nanos(1_577_817_005_000_000_000),
                context: Box::from(context),
                status,
                description: Box::from([]),
                target_url: Box::from([]),
            }),
        ));
    }
    rows.push((
        "create-branch",
        Answer::Branch(Branch {
            name: Box::from(*b"recorded"),
            commit: forgejo::sha(4),
            effective_branch_protection_name: None,
        }),
    ));
    rows
}
pub fn expected(answer: Answer) -> Document {
    match answer {
        Answer::Issue(value) => Document::Issue(value),
        Answer::Pull(value) => Document::Pull(value),
        Answer::Comment(value) => Document::Comment(value),
        Answer::Review(value) => Document::Review(value),
        Answer::Status(value) => Document::Status(value),
        Answer::Branch(value) => Document::Branch(value),
        Answer::Error(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Comments(_)
        | Answer::Files(_)
        | Answer::Compare { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Job { .. }
        | Answer::Jobs(_)
        | Answer::Pulls(_)
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Tree { .. }
        | Answer::Commits { .. }
        | Answer::Permission(_)
        | Answer::Org(_)
        | Answer::CreatedRepository(_)
        | Answer::CreatedFile { .. }
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::CombinedStatus { .. } => panic!("unexpected write answer"),
    }
}
pub fn failure_cases() -> Vec<(&'static str, u16, &'static [u8], Option<Refusal>)> {
    vec![
        ("merge-stale-head", 409, b"head out of date", Some(Refusal::HeadOutOfDate)),
        ("update-conflict", 409, b"merge failed because of conflict", Some(Refusal::UpdateConflict)),
        ("merge-closed", 405, b"", Some(Refusal::MergeClosed)),
        ("merge-conflict", 405, b"Please try again later", Some(Refusal::MergeConflict)),
        ("merge-protected", 405, b"not allowed to merge [reason: Does not have enough approvals]", Some(Refusal::MergeProtected)),
        ("merge-style-refused", 405, b"squash is not an allowed merge style for this repository", Some(Refusal::MergeStyle)),
        ("create-issue-empty-title", 422, b"[Title]: Required", Some(Refusal::Empty)),
        ("create-branch-exists", 409, b"The branch already exists.", None),
        ("no-token", 401, b"token is required", None),
        ("wrong-token", 401, b"token is malformed: token contains an invalid number of segments\naccess token does not exist [sha: 000000000000000000000000000000000000000d]\ntoken is malformed: token contains an invalid number of segments\ntask with token \"000000000000000000000000000000000000000d\": resource does not exist\nauthorized integration: parse JWT error: token is malformed: token contains an invalid number of segments", None),
        ("owner-permission", 403, b"Only admins can query all permissions, repo admins can query all repo permissions, collaborators can query only their own", None),
        ("writer-branch-protection", 403, b"user should be an owner or a collaborator with admin write of a repository", None),
        ("writer-branch-protections", 403, b"user should be an owner or a collaborator with admin write of a repository", None),
        ("delete-protected-branch", 403, b"can not delete default branch", None),
        ("archived-write", 423, b"<Repository 3:fixture/archived> is archived", None),
        ("missing-issue", 404, b"IsErrIssueNotExist", None),
        ("actions-job-log-past-last-step", 404, b"step index out of range", None),
        ("raw-file-missing", 404, b"The target couldn't be found.", None),
        ("stranger-repository-by-path", 404, b"The target couldn't be found.", None),
        ("stranger-repository-by-id", 404, b"The target couldn't be found.", None),
        ("delete-missing-branch", 500, b"", None),
        ("moved-repository", 301, b"", None),
        ("open-pull-exists", 409, b"pull request already exists for these targets [id: 1, issue_id: 1, head_repo_id: 1, base_repo_id: 1, head_branch: feature/slash, base_branch: main]", None),
    ]
}
