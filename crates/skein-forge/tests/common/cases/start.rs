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
    document::{ApiSettings, Document, MergeStyle, Repository, Row, User},
    id::ObjectFormat,
    request::Request,
    server::Answer,
};
pub fn repository() -> Repository {
    Repository {
        id: 1,
        owner: User { id: 1, login: Box::from(*b"fixture"), is_admin: false, full_name: Box::from([]) },
        name: Box::from(*b"specimen"),
        object_format: ObjectFormat::Sha1,
        default_branch: Box::from(*b"main"),
        default_merge_style: MergeStyle::Merge,
        allow_merge_commits: true,
        allow_squash_merge: true,
        allow_rebase: true,
        html_url: Box::from(*b"http://127.0.0.1:3000/fixture/specimen"),
    }
}
pub fn cases() -> [(&'static str, Request, Answer); 6] {
    [
        (
            "user",
            Request::User,
            Answer::User(User { id: 2, login: Box::from(*b"writer"), is_admin: false, full_name: Box::from([]) }),
        ),
        (
            "user-fixture",
            Request::User,
            Answer::User(User { id: 1, login: Box::from(*b"fixture"), is_admin: true, full_name: Box::from([]) }),
        ),
        (
            "settings-api",
            Request::ApiSettings,
            Answer::ApiSettings(ApiSettings { max_response_items: 2, default_paging_num: 30 }),
        ),
        ("repository-by-id", Request::RepositoryById { id: 1 }, Answer::Repository(repository())),
        ("repository-by-path", Request::Repository { at: forgejo::specimen() }, Answer::Repository(repository())),
        (
            "users-search-uid",
            Request::UserSearch { uid: 1, limit: Some(1) },
            Answer::Search {
                ok: true,
                data: Box::from([User {
                    id: 1,
                    login: Box::from(*b"fixture"),
                    is_admin: false,
                    full_name: Box::from([]),
                }]),
            },
        ),
    ]
}
pub fn expected(answer: Answer) -> (Document, Vec<Row>) {
    match answer {
        Answer::User(value) => (Document::User(value), vec![]),
        Answer::ApiSettings(value) => (Document::ApiSettings(value), vec![]),
        Answer::Repository(value) => (Document::Repository(value), vec![]),
        Answer::Search { ok, data } => (Document::Search { ok }, data.into_vec().into_iter().map(Row::User).collect()),
        Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Token(_)
        | Answer::Tokens(_)
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
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Tree { .. }
        | Answer::Commits { .. }
        | Answer::Permission(_)
        | Answer::Pulls(_)
        | Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::Org(_)
        | Answer::CreatedRepository(_)
        | Answer::CreatedFile { .. }
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::CombinedStatus { .. } => panic!("a start answer"),
    }
}
