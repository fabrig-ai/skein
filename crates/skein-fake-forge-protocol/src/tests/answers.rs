use super::fixtures::{at, boxed, config, limits};
use crate::{answers, routes};
use skein_fake_forge_domain::api as a;
use skein_forge::forgejo::{
    document as d,
    request::{Page, Request},
    server::{self, Answer},
};
use skein_lib::{Time, Wall, bytes};

#[test]
fn set_base_admits_the_constructors_origins_and_preserves_refusals() {
    const BOUND: &[u8] = b"http://127.0.0.1:65535";
    let mut limits = limits();
    // The longest bound IPv4 origin is admitted at target_bytes exactly.
    limits.documents.target_bytes = u32::try_from(BOUND.len()).expect("literal origin");
    let mut service = crate::Service::new(config(), &limits, 3).expect("fixture config");
    let settings = a::Settings { default: boxed(b"main"), merge: true, rebase: true, squash: true };
    for (name, base, expected, html) in [
        ("bound ip:port at cap", BOUND, Ok(()), b"http://127.0.0.1:65535/org/repo".as_slice()),
        ("TLS at cap", b"https://127.0.0.1:6553", Ok(()), b"https://127.0.0.1:6553/org/repo"),
        ("TLS one past cap", b"https://127.0.0.1:65535", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("TLS empty authority", b"https://", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("TLS slash", b"https://127.0.0.1/", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("no scheme", b"127.0.0.1:65535", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("empty authority", b"http://", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("slash", b"http://127.0.0.1/", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("below printable", b"http://127.0.0.1 ", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("above printable", b"http://127.0.0.1\x7f", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
        ("one past cap", b"http://127.0.0.1:655350", Err(crate::Error::Limits), b"http://127.0.0.1:3000/org/repo"),
    ] {
        service.set_base(config().base, &limits).expect("reset origin");
        let input = crate::Config { base: boxed(base), ..config() };
        let admitted = match crate::Service::new(input, &limits, 3) {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        };
        assert_eq!(admitted, expected, "new: {name}");
        assert_eq!(service.set_base(boxed(base), &limits), expected, "set_base: {name}");
        let Answer::Repository(repository) =
            answers::repository_document(&service.config, 2, b"org/repo", &settings).expect("repository")
        else {
            panic!("repository answer")
        };
        assert_eq!(repository.html_url.as_ref(), html, "{name}");
    }
}

#[test]
fn repositories_keep_the_scenarios_id_path_and_domain_settings() {
    let input =
        a::Answer::Settings(a::Settings { default: boxed(b"trunk"), merge: false, rebase: false, squash: true });
    for request in [Request::RepositoryById { id: 2 }, Request::Repository { at: at() }] {
        let repository = answers::read(&config(), &crate::setup::Store::new(&limits()), &request, &input);
        assert_eq!(
            repository,
            Ok(Answer::Repository(d::Repository {
                id: 2,
                owner: d::User { id: 0, login: boxed(b"org"), is_admin: false, full_name: Box::from([]) },
                name: boxed(b"repo"),
                object_format: skein_forge::forgejo::id::ObjectFormat::Sha1,
                default_branch: boxed(b"trunk"),
                default_merge_style: d::MergeStyle::Squash,
                allow_merge_commits: false,
                allow_squash_merge: true,
                allow_rebase: false,
                html_url: boxed(b"http://127.0.0.1:3000/org/repo")
            })),
            "{request:?}"
        );
    }
}

#[test]
fn issue_documents_keep_the_domain_values() {
    let summary = a::Summary {
        number: 7,
        kind: a::Kind::Issue,
        state: a::State::Closed,
        merged: false,
        title: boxed(b"title"),
        body: boxed(b"body"),
        author: 2,
        labels: Box::new([boxed(b"label")]),
        created: Time::from_nanos(1_000_000_000),
        updated: Time::from_nanos(2_000_000_000),
    };
    assert_eq!(
        answers::issue(&config(), &crate::setup::Store::new(&limits()), &summary),
        d::Issue {
            number: 7,
            pull: false,
            state: d::IssueState::Closed,
            user: d::User { id: 2, login: boxed(b"person"), is_admin: false, full_name: Box::from([]) },
            labels: Box::new([boxed(b"label")]),
            updated_at: Wall::from_nanos(2_000_000_000),
            title: boxed(b"title"),
            body: d::Body::Kept(boxed(b"body"))
        }
    );
}
#[test]
fn comment_documents_keep_the_domain_values() {
    for (edited, expected) in
        [(None, None), (Some(Time::from_nanos(2_000_000_000)), Some(Wall::from_nanos(2_000_000_000)))]
    {
        let comment =
            a::Comment { id: 8, author: 17, body: boxed(b"comment"), created: Time::from_nanos(1_000_000_000), edited };
        assert_eq!(
            answers::comment(&config(), &crate::setup::Store::new(&limits()), &comment),
            d::Comment {
                id: 8,
                user: d::User { id: 17, login: boxed(b"u17"), is_admin: false, full_name: Box::from([]) },
                created_at: Some(Wall::from_nanos(1_000_000_000)),
                updated_at: expected,
                body: d::Body::Kept(boxed(b"comment"))
            },
            "edited {edited:?}"
        );
    }
}

#[test]
fn review_documents_keep_the_head_verdict_and_official_flag() {
    for (verdict, expected) in [
        (a::Verdict::Approve, d::ReviewState::Approved),
        (a::Verdict::RequestChanges, d::ReviewState::RequestChanges),
        (a::Verdict::Comment, d::ReviewState::Comment),
    ] {
        let input = a::Review {
            id: 8,
            author: 2,
            verdict,
            commit: 7,
            body: boxed(b"review"),
            at: Time::from_nanos(1_000_000_000),
            official: true,
        };
        assert_eq!(
            answers::review(&config(), &crate::setup::Store::new(&limits()), &input),
            d::Review {
                id: 8,
                user: d::User { id: 2, login: boxed(b"person"), is_admin: false, full_name: Box::from([]) },
                state: expected,
                commit_id: routes::object(7),
                body: d::Body::Kept(boxed(b"review")),
                submitted_at: Some(Wall::from_nanos(1_000_000_000)),
                updated_at: None,
                official: true,
                dismissed: false
            },
            "{verdict:?}"
        );
    }
}

#[test]
fn changed_files_keep_the_path_and_each_kind_of_change() {
    for (before, after, expected) in [
        (None, Some(boxed(b"a")), d::FileStatus::Added),
        (Some(boxed(b"b")), None, d::FileStatus::Deleted),
        (Some(boxed(b"b")), Some(boxed(b"a")), d::FileStatus::Modified),
    ] {
        let request = Request::PullFiles { at: at(), number: 1, page: Page { page: Some(1), limit: Some(2) } };
        let input = a::Answer::PullFiles {
            head: 3,
            files: Box::new([a::ChangedFile { path: boxed(b"src"), before, after }]),
            more: true,
        };
        assert_eq!(
            answers::read(&config(), &crate::setup::Store::new(&limits()), &request, &input),
            Ok(Answer::Files(Box::new([d::ChangedFile {
                filename: boxed(b"src"),
                previous_filename: None,
                status: expected
            }]))),
            "{expected:?}"
        );
    }
}

#[test]
fn a_ci_status_has_no_creator_and_a_link_relative_to_the_origin() {
    let input = a::Answer::Checks(Box::new([a::CheckSummary {
        status: a::Status { context: boxed(b"ci"), state: a::Check::Failed, author: 3, at: Time::ZERO },
        description: boxed(b"failed"),
        link: boxed(b"/org/repo/actions/runs/1/jobs/0"),
        job: Some(a::JobRef { run: 1, job: 1, attempt: 1 }),
    }]));
    let request =
        Request::CombinedStatus { at: at(), commit: routes::object(7), page: Page { page: None, limit: None } };
    let answer = answers::read(&config(), &crate::setup::Store::new(&limits()), &request, &input).expect("statuses");
    let encoded = server::encode(&answer, &limits().documents).expect("JSON");
    for expected in [
        b"\"creator\":null".as_slice(),
        b"\"target_url\":\"/org/repo/actions/runs/1/jobs/0\"",
        b"\"total_count\":1",
        b"\"state\":\"failure\"",
    ] {
        assert!(bytes::find(&encoded, expected).is_some(), "{expected:?}");
    }
}

#[test]
fn statuses_combine_failed_before_pending_before_passed() {
    for (checks, expected) in [
        ([a::Check::Passed, a::Check::Passed], Some(d::StatusState::Success)),
        ([a::Check::Passed, a::Check::Pending], Some(d::StatusState::Pending)),
        ([a::Check::Failed, a::Check::Pending], Some(d::StatusState::Failure)),
        ([a::Check::Pending, a::Check::Failed], Some(d::StatusState::Failure)),
    ] {
        let mut rows = skein_lib::List::with_capacity(2);
        for state in checks {
            rows.push(a::CheckSummary {
                status: a::Status { context: boxed(b"ci"), state, author: 2, at: Time::ZERO },
                description: boxed(b""),
                link: boxed(b""),
                job: None,
            })
            .expect("two");
        }
        let input = a::Answer::Checks(rows.into_boxed());
        let request =
            Request::CombinedStatus { at: at(), commit: routes::object(7), page: Page { page: None, limit: None } };
        let answer =
            answers::read(&config(), &crate::setup::Store::new(&limits()), &request, &input).expect("statuses");
        let Answer::CombinedStatus { state, statuses, .. } = answer else {
            panic!("statuses");
        };
        assert_eq!(state, d::CombinedStatus { state: expected, total_count: 2 }, "{checks:?}");
        for row in &statuses {
            assert_eq!(
                row.creator,
                Some(d::User { id: 2, login: boxed(b"person"), is_admin: false, full_name: Box::from([]) }),
                "{checks:?}"
            );
        }
    }
}

#[test]
fn a_files_false_comparison_keeps_commits_and_lists_no_files() {
    let input = a::Answer::Comparison {
        merge_base: Some(1),
        base: 8,
        head: 9,
        contains_base: true,
        files: Box::new([a::ChangedFile { path: boxed(b"src"), before: None, after: Some(boxed(b"a")) }]),
        commits: Box::new([9]),
    };
    for (files, listed) in [(true, 1), (false, 0)] {
        let request = Request::Compare { at: at(), before: routes::object(8), after: routes::object(9), files };
        let Ok(Answer::Compare { compare, commits, files: changed }) =
            answers::read(&config(), &crate::setup::Store::new(&limits()), &request, &input)
        else {
            panic!("A comparison");
        };
        assert_eq!((compare.total_commits, commits.as_ref()), (1, [routes::object(9)].as_slice()), "{files}");
        assert_eq!(changed.len(), listed, "{files}");
    }
}

#[test]
fn a_jobs_steps_keep_their_numbers_names_and_statuses() {
    let input = a::Answer::Job {
        job: a::Job { id: 8, run: 7, index: 0, attempt: 2 },
        steps: Box::new([
            a::Step { number: 0, name: boxed(b"Setup"), status: a::StepStatus::Success, log: boxed(b"setup\n") },
            a::Step { number: 1, name: boxed(b"Work"), status: a::StepStatus::Cancelled, log: boxed(b"work\n") },
            a::Step { number: 2, name: boxed(b"Finish"), status: a::StepStatus::Failure, log: boxed(b"finish\n") },
        ]),
    };
    assert_eq!(
        answers::read(&config(), &crate::setup::Store::new(&limits()), &Request::Job { at: at(), job: 8 }, &input),
        Ok(Answer::Job {
            job: d::Job { id: 8, run_id: 7, attempt: 2 },
            steps: Box::new([
                d::Step { number: 0, name: boxed(b"Setup"), status: d::StepStatus::Success },
                d::Step { number: 1, name: boxed(b"Work"), status: d::StepStatus::Cancelled },
                d::Step { number: 2, name: boxed(b"Finish"), status: d::StepStatus::Failure },
            ]),
        })
    );
}
