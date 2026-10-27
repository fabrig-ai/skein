use crate::github::{
    decode,
    document::Kind,
    request::{Request, Scope},
};
use alloc::boxed::Box;
use skein_lib::Wall;

fn owner() -> Box<[u8]> {
    Box::from(*b"acme")
}
fn name() -> Box<[u8]> {
    Box::from(*b"widgets")
}

#[test]
fn github_decode_kind_names_each_documented_answer() {
    let commit = [7; 32];
    let method = crate::github::plans::tables::Method::Merge;
    for (request, kind) in [
        (Request::Installations { page: 1 }, Kind::InstallationRows),
        (Request::Installation { id: 3 }, Kind::Installation),
        (Request::Mint { installation: 3, scope: Scope::Contents }, Kind::Minted),
        (Request::Reachable { page: 1, per_page: 100 }, Kind::ReachableRows),
        (
            Request::CreateIssue { owner: owner(), name: name(), title: Box::from([]), body: Box::from([]) },
            Kind::Number,
        ),
        (Request::Post { owner: owner(), name: name(), number: 9, body: Box::from([]) }, Kind::Id),
        (
            Request::Review {
                owner: owner(),
                name: name(),
                number: 9,
                verdict: crate::api::Verdict::Approve,
                body: Box::from([]),
            },
            Kind::Id,
        ),
        (Request::Reviewers { owner: owner(), name: name(), number: 9, add: true, logins: Box::from([]) }, Kind::Done),
        (Request::Reviewers { owner: owner(), name: name(), number: 9, add: false, logins: Box::from([]) }, Kind::Done),
        (Request::Update { owner: owner(), name: name(), number: 9, head: commit }, Kind::Done),
        (Request::Merge { owner: owner(), name: name(), number: 9, head: commit, method }, Kind::Merge),
        (Request::Enqueue { id: Box::from(*b"opaque"), head: commit }, Kind::Enqueue),
        (Request::Viewer, Kind::Viewer),
        (Request::PullGraphql { owner: owner(), name: name(), number: 5 }, Kind::PullGraphql),
        (Request::App, Kind::App),
        (
            Request::OpenPull {
                owner: owner(),
                name: name(),
                title: Box::from([]),
                body: Box::from([]),
                head: Box::from(*b"alice"),
                base: Box::from(*b"main"),
            },
            Kind::Number,
        ),
        (Request::CreateBranch { owner: owner(), name: name(), branch: Box::from(*b"alice"), commit }, Kind::Done),
        (status(commit), Kind::Done),
        (Request::Edit { owner: owner(), name: name(), number: 9, title: None, body: None }, Kind::Done),
        (Request::Reopen { owner: owner(), name: name(), number: 9 }, Kind::Done),
        (Request::Close { owner: owner(), name: name(), number: 9 }, Kind::Done),
        (Request::HookConfig, Kind::HookConfig),
        (Request::SetHook { url: Box::from(*b"https://example.test/hook") }, Kind::HookConfig),
        (Request::Conversion { code: Box::from(*b"code") }, Kind::Conversion),
        (Request::BotUser { slug: Box::from(*b"example-app") }, Kind::BotUser),
        (Request::Collaborators { owner: owner(), name: name(), page: 1, per_page: 100 }, Kind::CollaboratorRows),
        (
            Request::ReviewComments { owner: owner(), name: name(), number: 9, review: 2, page: 1, per_page: 100 },
            Kind::RemarkRows,
        ),
        (Request::Reviews { owner: owner(), name: name(), number: 9, page: 1, per_page: 100 }, Kind::ReviewRows),
        (Request::PullFiles { owner: owner(), name: name(), number: 9, page: 1, per_page: 100 }, Kind::FileRows),
        (
            Request::Compare { owner: owner(), name: name(), before: commit, after: [8; 32], per_page: 100 },
            Kind::Compare,
        ),
        (Request::User { id: 6 }, Kind::User),
        (Request::Permission { owner: owner(), name: name(), login: Box::from(*b"alice") }, Kind::Permission),
        (Request::Repository { id: 5 }, Kind::Repository),
        (Request::Locate { owner: owner(), name: name() }, Kind::Repository),
        (Request::Issues { owner: owner(), name: name(), since: Wall::EPOCH, page: 1, per_page: 100 }, Kind::IssueRows),
        (Request::Issue { owner: owner(), name: name(), number: 9 }, Kind::Issue),
        (Request::Comments { owner: owner(), name: name(), number: 9, page: 1, per_page: 100 }, Kind::CommentRows),
        (Request::Pull { owner: owner(), name: name(), number: 9 }, Kind::Pull),
        (
            Request::PullsFor {
                owner: owner(),
                name: name(),
                head: Box::from(*b"topic"),
                base: Box::from(*b"main"),
                open: true,
            },
            Kind::PullRows,
        ),
        (
            Request::CombinedStatus { owner: owner(), name: name(), commit, page: 1, per_page: 100 },
            Kind::CombinedStatus,
        ),
        (Request::Job { owner: owner(), name: name(), id: 11 }, Kind::Job),
        (Request::CheckRuns { owner: owner(), name: name(), commit, page: 1, per_page: 100 }, Kind::CheckRuns),
        (Request::Branch { owner: owner(), name: name(), branch: Box::from(*b"main") }, Kind::Branch),
        (Request::Branches { owner: owner(), name: name(), page: 1, per_page: 100 }, Kind::BranchRows),
    ] {
        assert_eq!(decode::kind(&request), Some(kind), "{request:?}");
    }
}

#[test]
fn github_decode_kind_is_none_for_raw_and_empty_answers() {
    for request in [
        Request::Raw { owner: owner(), name: name(), path: Box::from(*b"README.md"), commit: [7; 32] },
        Request::JobLog { owner: owner(), name: name(), id: 11 },
        Request::Uninstall { id: 3 },
        Request::DeleteBranch { owner: owner(), name: name(), branch: Box::from(*b"alice") },
    ] {
        assert_eq!(decode::kind(&request), None, "{request:?}");
    }
}

#[test]
fn github_protection_requests_select_their_decoders() {
    for (request, kind) in [
        (
            Request::RulesForBranch { owner: owner(), name: name(), branch: Box::from(*b"main"), page: 1 },
            Kind::RuleRows,
        ),
        (Request::Ruleset { owner: owner(), name: name(), id: 4 }, Kind::Ruleset),
        (
            Request::ClassicProtection { owner: owner(), name: name(), branch: Box::from(*b"main") },
            Kind::ClassicProtection,
        ),
    ] {
        assert_eq!(decode::kind(&request), Some(kind));
    }
}

fn status(commit: crate::api::Commit) -> Request {
    Request::Status {
        owner: owner(),
        name: name(),
        commit,
        context: Box::from(*b"alice"),
        check: crate::api::Check::Pending,
        description: Box::new([]),
        link: Box::new([]),
    }
}
