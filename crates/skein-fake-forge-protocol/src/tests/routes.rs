use super::fixtures::{at, boxed, config};
use crate::{
    errors, route, routes,
    tally::{Route, Tally},
};
use skein_fake_forge_domain::api::{self as a, Op, Read, Write};
use skein_forge::forgejo::{
    document as d,
    failure::Refusal,
    request::{self as r, Request},
};
use skein_lib::{Duration, Time, Wall};

fn read_rows() -> [(Request, Route, Op); 26] {
    let p = r::Page { page: Some(2), limit: Some(3) };
    let oid = routes::object(9);
    [
        (Request::RepositoryById { id: 2 }, Route::RepositoryById, Op::Read(Read::Settings)),
        (Request::Repository { at: at() }, Route::Repository, Op::Read(Read::Settings)),
        (
            Request::Issues {
                at: at(),
                state: Some(r::StateFilter::Open),
                kind: Some(r::IssueKind::Issues),
                since: Some(Wall::from_nanos(7)),
                sort: Some(r::Sort::LeastUpdate),
                page: p,
            },
            Route::Issues,
            Op::Read(Read::Items {
                state: Some(a::State::Open),
                kind: Some(a::Kind::Issue),
                since: Time::from_nanos(7),
                order: a::Order::LeastUpdated,
                labels: Box::from([]),
                author: None,
                page: 2,
                limit: 2,
            }),
        ),
        (Request::Issue { at: at(), number: 3 }, Route::Issue, Op::Read(Read::Item { number: 3, after: 0 })),
        (Request::Comments { at: at(), number: 3 }, Route::Comments, Op::Read(Read::Comments { number: 3 })),
        (Request::Pull { at: at(), number: 3 }, Route::Pull, Op::Read(Read::Pull { number: 3 })),
        (
            Request::PullFor { at: at(), head: boxed(b"work"), base: boxed(b"main") },
            Route::PullFor,
            Op::Read(Read::FirstPull { head: boxed(b"work"), base: boxed(b"main") }),
        ),
        (
            Request::CombinedStatus { at: at(), commit: oid, page: p },
            Route::CombinedStatus,
            Op::Read(Read::Checks { commit: 9 }),
        ),
        (Request::Reviews { at: at(), number: 3, page: p }, Route::Reviews, Op::Read(Read::Pull { number: 3 })),
        (
            Request::ReviewComments { at: at(), number: 3, review: 2 },
            Route::ReviewComments,
            Op::Read(Read::Pull { number: 3 }),
        ),
        (
            Request::Branch { at: at(), name: boxed(b"main") },
            Route::Branch,
            Op::Read(Read::Branch { branch: boxed(b"main") }),
        ),
        (Request::Branches { at: at(), prefix: None, page: p }, Route::Branches, Op::Git(a::Git::Clone)),
        (
            Request::Protection { at: at(), name: boxed(b"main") },
            Route::Protection,
            Op::Read(Read::Protection { branch: boxed(b"main") }),
        ),
        (Request::Protections { at: at() }, Route::Protections, Op::Read(Read::Protection { branch: boxed(b"") })),
        (Request::Collaborators { at: at(), page: p }, Route::Collaborators, Op::Read(Read::Collaborators)),
        (
            Request::Permission { at: at(), login: boxed(b"person") },
            Route::Permission,
            Op::Read(Read::Permission { user: 2 }),
        ),
        (Request::Pulls { at: at(), page: p }, Route::Pulls, Op::Read(Read::OpenPulls { page: 2, limit: 2 })),
        (
            Request::PullFiles { at: at(), number: 3, page: p },
            Route::PullFiles,
            Op::Read(Read::PullFiles { number: 3, page: 2, limit: 2 }),
        ),
        (
            Request::Compare { at: at(), before: routes::object(8), after: oid, files: true },
            Route::Compare,
            Op::Read(Read::Compare { base: 8, head: 9, page: 1, limit: 2 }),
        ),
        (
            Request::Raw { at: at(), reference: oid, path: boxed(b"src") },
            Route::Raw,
            Op::Read(Read::File { commit: 9, path: boxed(b"src") }),
        ),
        (
            Request::Runs { at: at(), head_sha: oid, run_number: 7, page: p },
            Route::Runs,
            Op::Read(Read::Runs { workflow: None, commit: Some(9), number: Some(7), page: 1, limit: 1 }),
        ),
        (Request::Run { at: at(), run: 7 }, Route::Run, Op::Read(Read::Run { run: 7 })),
        (Request::Jobs { at: at(), run: 7 }, Route::Jobs, Op::Read(Read::Jobs { run: 7 })),
        (Request::Job { at: at(), job: 8 }, Route::Job, Op::Read(Read::Job { job: 8 })),
        (
            Request::JobLog { at: at(), job: 8, attempt: 2, step: Some(1) },
            Route::JobLog,
            Op::Read(Read::Log { job: 8, attempt: 2, step: Some(1) }),
        ),
        (
            Request::JobLog { at: at(), job: 8, attempt: 2, step: None },
            Route::JobLog,
            Op::Read(Read::Log { job: 8, attempt: 2, step: None }),
        ),
    ]
}
#[test]
fn each_read_route_names_one_domain_read() {
    for (input, name, expected) in read_rows() {
        assert_eq!(route(&input), name, "{name:?}");
        assert_eq!(routes::repository(&config(), &input).as_deref(), Some(&b"org/repo"[..]), "{name:?}");
        assert_eq!(routes::op(&config(), &input), Ok(expected), "{name:?}");
    }
}

fn write_rows() -> [(Request, Route, Write); 14] {
    [
        (
            Request::CreateIssue { at: at(), title: boxed(b"t"), body: boxed(b"b") },
            Route::CreateIssue,
            Write::CreateIssue { title: boxed(b"t"), body: boxed(b"b"), labels: Box::from([]) },
        ),
        (
            Request::OpenPull {
                at: at(),
                head: boxed(b"work"),
                base: boxed(b"main"),
                title: boxed(b"t"),
                body: boxed(b"b"),
            },
            Route::OpenPull,
            Write::OpenPull {
                head: boxed(b"work"),
                base: boxed(b"main"),
                title: boxed(b"t"),
                body: boxed(b"b"),
                opens_empty: true,
            },
        ),
        (
            Request::PostComment { at: at(), number: 3, body: boxed(b"b") },
            Route::PostComment,
            Write::Comment { number: 3, body: boxed(b"b") },
        ),
        (
            Request::PostReview { at: at(), number: 3, event: r::ReviewEvent::Approved, body: boxed(b"b") },
            Route::PostReview,
            Write::Review { number: 3, verdict: Some(a::Verdict::Approve), body: boxed(b"b") },
        ),
        (
            Request::EditIssue { at: at(), number: 3, title: None, body: None, state: Some(d::IssueState::Closed) },
            Route::EditIssue,
            Write::Close { number: 3 },
        ),
        (
            Request::EditIssue { at: at(), number: 3, title: None, body: None, state: Some(d::IssueState::Open) },
            Route::EditIssue,
            Write::Reopen { number: 3 },
        ),
        (
            Request::EditIssue { at: at(), number: 3, title: Some(boxed(b"t")), body: Some(boxed(b"b")), state: None },
            Route::EditIssue,
            Write::EditItem { number: 3, title: Some(boxed(b"t")), body: Some(boxed(b"b")) },
        ),
        (
            Request::RequestReviewers { at: at(), number: 3, reviewers: Box::new([boxed(b"person")]) },
            Route::RequestReviewers,
            Write::RequestReviewers { number: 3, reviewers: Box::new([2]) },
        ),
        (
            Request::RemoveReviewers { at: at(), number: 3, reviewers: Box::new([boxed(b"person")]) },
            Route::RemoveReviewers,
            Write::RemoveReviewers { number: 3, reviewers: Box::new([2]) },
        ),
        (
            Request::Merge { at: at(), number: 3, style: d::MergeStyle::Squash, head: routes::object(9) },
            Route::Merge,
            Write::Merge { number: 3, head: 9, how: a::MergeHow::Style },
        ),
        (
            Request::Update { at: at(), number: 3, style: None },
            Route::Update,
            Write::Update { number: 3, style: a::UpdateStyle::Merge },
        ),
        (
            Request::PostStatus {
                at: at(),
                commit: routes::object(9),
                context: boxed(b"ci"),
                state: d::StatusState::Failure,
                description: Box::new([]),
                link: Box::new([]),
            },
            Route::PostStatus,
            Write::Status {
                commit: 9,
                context: boxed(b"ci"),
                state: a::Check::Failed,
                description: Box::new([]),
                link: Box::new([]),
            },
        ),
        (
            Request::CreateBranch { at: at(), branch: boxed(b"work"), from: routes::object(9) },
            Route::CreateBranch,
            Write::CreateBranch { branch: boxed(b"work"), commit: 9 },
        ),
        (
            Request::DeleteBranch { at: at(), branch: boxed(b"work") },
            Route::DeleteBranch,
            Write::DeleteBranch { branch: boxed(b"work") },
        ),
    ]
}
#[test]
fn each_write_route_names_one_domain_write() {
    for (input, name, expected) in write_rows() {
        assert_eq!(route(&input), name, "{name:?}");
        assert_eq!(routes::op(&config(), &input), Ok(Op::Write(expected)), "{name:?}");
    }
}

#[test]
fn commit_counts_round_trip_in_the_shared_sha1_spelling() {
    for (count, hex) in [
        (0, b"e220a8397b1dcdaf000000000000000000000000"),
        (1, b"910a2dec89025cc1000000000000000100000000"),
        (2, b"975835de1c9756ce000000000000000200000000"),
        (u64::MAX, b"e4d971771b652c20ffffffffffffffff00000000"),
    ] {
        let object = routes::object(count);
        assert_eq!(&*skein_forge::forgejo::id::write(&object), hex, "{count}");
        assert_eq!(object.bytes.get(16..), Some([0; 16].as_slice()), "{count}");
        assert_eq!(routes::commit(&object), Some(count), "{count}");
    }
}

#[test]
fn count_zero_has_a_nonzero_id() {
    assert_ne!(routes::object(0).bytes, [0; 32]);
}

#[test]
fn the_first_256_counts_have_distinct_nonzero_short_forms() {
    let mut short_forms = alloc::collections::BTreeSet::new();
    for count in 0..=255 {
        let hex = skein_forge::forgejo::id::write(&routes::object(count));
        let short = hex.get(..7).expect("SHA-1 short form");
        assert_ne!(short, b"0000000", "{count}");
        assert!(short_forms.insert(skein_lib::bytes::copy_of(short)), "duplicate short form for {count}");
    }
    assert_eq!(short_forms.len(), 256);
}

#[test]
fn a_changed_mix_count_or_tail_names_no_commit() {
    for byte in 0..32 {
        let mut object = routes::object(1);
        *object.bytes.get_mut(byte).expect("object byte") ^= 1;
        assert_eq!(routes::commit(&object), None, "byte {byte}");
    }
}

#[test]
fn sha256_names_no_commit() {
    let mut object = routes::object(1);
    object.format = skein_forge::forgejo::id::ObjectFormat::Sha256;
    assert_eq!(routes::commit(&object), None, "sha256");
}

#[test]
fn a_limit_without_a_page_takes_the_default_paging_num() {
    let mut c = config();
    c.max_response_items = 50;
    for (input, expected) in [
        (r::Page { page: None, limit: Some(1) }, (1, 30)),
        (r::Page { page: Some(1), limit: Some(1) }, (1, 1)),
        (r::Page { page: Some(0), limit: Some(0) }, (1, 1)),
        (r::Page { page: Some(2), limit: Some(99) }, (2, 50)),
    ] {
        assert_eq!(routes::page(&c, input), expected, "{input:?}");
    }
}

#[test]
fn requests_are_counted_by_route_user_and_total() {
    let mut tally = Tally::new(2);
    tally.count(Some(Route::User), Some(1));
    tally.count(Some(Route::Issues), Some(2));
    tally.count(Some(Route::Issues), Some(1));
    tally.count(None, None);
    assert_eq!(tally.total, 4);
    for (route, expected) in [(Route::User, Some(1)), (Route::Issues, Some(2)), (Route::Merge, None)] {
        assert_eq!(tally.routes.get(&route).copied(), expected, "{route:?}");
    }
    for (user, expected) in [(1, Some(2)), (2, Some(1)), (3, None)] {
        assert_eq!(tally.users.get(&user).copied(), expected, "user {user}");
    }
    assert_eq!((tally.unrouted, tally.unauthorized), (1, 1));
}

#[test]
fn domain_failures_have_forgejos_status_and_exact_message() {
    use a::Error as E;
    let now = Time::ZERO;
    let rows = [
        (E::Forbidden, Route::Issue, 403, b"".as_slice(), None),
        (E::Missing(a::What::Branch), Route::DeleteBranch, 500, b"", None),
        (E::Missing(a::What::Item), Route::Issue, 404, b"", None),
        (
            E::RateLimited { reset: now.saturating_add(Duration::from_nanos(1_000_000_001)) },
            Route::Issue,
            429,
            b"",
            Some(2),
        ),
        (E::Exists, Route::OpenPull, 409, b"", None),
        (E::Stale, Route::Merge, 409, b"head out of date", None),
        (E::Conflict, Route::Merge, 405, b"Please try again later", None),
        (E::Conflict, Route::Update, 409, b"merge failed because of conflict", None),
        (E::Closed, Route::Merge, 405, b"", None),
        (E::Protected, Route::Merge, 405, b"not allowed to merge [reason: Does not have enough approvals]", None),
        (E::Protected, Route::DeleteBranch, 403, b"", None),
        (E::NothingToMerge, Route::Merge, 422, b"", None),
        (E::Empty, Route::CreateIssue, 422, b"[Title]: Required", None),
        (E::Full, Route::CreateIssue, 413, b"", None),
        (E::TooLarge, Route::CreateIssue, 413, b"", None),
        (E::Refused, Route::CreateIssue, 422, b"", None),
        (E::Circular, Route::CreateIssue, 422, b"", None),
        (E::Unavailable, Route::Issue, 503, b"", None),
        (E::Unreachable, Route::Issue, 503, b"", None),
        (E::Timeout, Route::Issue, 500, b"", None),
    ];
    for (input, route, status, message, retry_after) in rows {
        assert_eq!(
            errors::answer(input, route, now),
            errors::Failure { status, message, retry_after },
            "{input:?} on {route:?}"
        );
    }
    assert_eq!(errors::refusal(Refusal::MergeStyle).status, 405);
}

#[test]
fn turn_routes_name_the_domain_calls_and_keep_their_fields() {
    let config = config();
    let page = r::Page { page: Some(2), limit: Some(1) };
    let tree = routes::tree_object([17; 20]);
    for (input, expected) in [
        (
            Request::Tree { at: at(), object: routes::object(9), page },
            Op::Read(Read::GitTree { object: a::TreeOf::Commit(9), page: 2, limit: 1 }),
        ),
        (
            Request::Tree { at: at(), object: tree, page },
            Op::Read(Read::GitTree { object: a::TreeOf::Tree([17; 20]), page: 2, limit: 1 }),
        ),
        (
            Request::WorkflowRuns { at: at(), workflow: boxed(b"gate.yml"), head_sha: routes::object(9), page },
            Op::Read(Read::Runs { workflow: None, commit: Some(9), number: None, page: 2, limit: 1 }),
        ),
        (
            Request::Dispatch {
                at: at(),
                workflow: boxed(b"gate.yml"),
                branch: boxed(b"main"),
                inputs: Box::new([skein_forge::api::Input { name: boxed(b"select"), value: boxed(b"all") }]),
            },
            Op::Write(Write::Dispatch {
                workflow: boxed(b"gate.yml"),
                branch: boxed(b"main"),
                inputs: Box::new([(boxed(b"select"), boxed(b"all"))]),
            }),
        ),
        (Request::Rerun { at: at(), job: 7 }, Op::Write(Write::Rerun { job: 7 })),
        (
            Request::Merge { at: at(), number: 3, head: routes::object(9), style: d::MergeStyle::FastForwardOnly },
            Op::Write(Write::Merge { number: 3, head: 9, how: a::MergeHow::FastForwardOnly }),
        ),
        (
            Request::Update { at: at(), number: 3, style: Some(r::UpdateStyle::Rebase) },
            Op::Write(Write::Update { number: 3, style: a::UpdateStyle::Rebase }),
        ),
        (
            Request::PostStatus {
                at: at(),
                commit: routes::object(9),
                context: boxed(b"gate"),
                state: d::StatusState::Success,
                description: boxed(b"proved"),
                link: boxed(b"https://ci/gate"),
            },
            Op::Write(Write::Status {
                commit: 9,
                context: boxed(b"gate"),
                state: a::Check::Passed,
                description: boxed(b"proved"),
                link: boxed(b"https://ci/gate"),
            }),
        ),
    ] {
        assert_eq!(routes::op(&config, &input), Ok(expected), "{input:?}");
    }
    assert_eq!(&tree.bytes[..20], &[17; 20]);
    assert_eq!(skein_forge::forgejo::id::write(&tree).as_ref(), b"1111111111111111111111111111111111111111");
    assert_eq!(errors::answer(a::Error::Refused, Route::Rerun, Time::ZERO).status, 400);
    assert_eq!(errors::answer(a::Error::Missing(a::What::File), Route::Dispatch, Time::ZERO).status, 404);
    assert_eq!(errors::answer(a::Error::Missing(a::What::Branch), Route::Dispatch, Time::ZERO).status, 400);
    assert_eq!(errors::answer(a::Error::Conflict, Route::Update, Time::ZERO).status, 409);
}
