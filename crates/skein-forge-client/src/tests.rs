//! Step tests of admission, request accounting, bounded answers and recovery.
mod budget;
mod contracts;
mod fixtures;
use fixtures::{Harness, Sent, at, configured, listing_item, terminal};
mod keep;
mod locate;
mod outbox;
mod queue;
mod recovery;
mod settle;
mod writer;
use crate::{Domain, Event, Fact, Limits, Priority, Request, worst_case};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read, Repository, Write};
use skein_lib::{Duration, Time, Token};

const LIMITS: Limits = Limits {
    pending: 8,
    calls: 8,
    rate: 4,
    reserve: 2,
    window: Duration::from_secs(60),
    op_bytes: 256,
    answer_bytes: 4096,
    rows: 4,
    inbox: 16,
    read_attempts: 3,
    backoff: Duration::from_secs(1),
    backoff_max: Duration::from_secs(8),
    entries: 4,
    write_attempts: 3,
    lifetime: Duration::from_secs(10),
    clock_margin: Duration::from_secs(1),
    resources: 4,
    repositories: 2,
    poll: Duration::from_secs(5),
    poll_max: Duration::from_secs(60),
    hinted: Duration::from_secs(1),
    slow: Duration::from_secs(60),
    settle: Duration::from_secs(30),
    facts: 32,
};
const REPO: Repository = Repository { forge: 2, repository: 5 };

#[test]
fn a_fresh_read_meeting_no_connection_ends_once() {
    fresh_read_refusal_ends_once(Error::NotConnected);
}
#[test]
fn a_fresh_read_meeting_no_installation_ends_once() {
    fresh_read_refusal_ends_once(Error::NotInstalled);
}
fn fresh_read_refusal_ends_once(error: Error) {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    let sent = h.send().expect("fresh read");
    terminal(&h.answer(&sent, 0, Err(error)), 1, error);
    assert!(h.send().is_none());
}

#[test]
fn a_fresh_read_starts_immediately_without_global_lifetime_silence() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 1 }));
}
#[test]
fn priority_and_parallel_limit_follow_the_budget_cases() {
    let mut h = Harness::new(Limits { calls: 2, ..LIMITS });
    h.queued(1, Priority::Slow);
    h.queued(2, Priority::Keep);
    h.queued(3, Priority::Write);
    h.read(4);
    let first = h.one();
    let second = h.one();
    assert_eq!(first.op, Op::Read(Read::Pull { number: 4 }));
    assert_eq!(second.op, Op::Write(Write::Close { number: 3 }));
    assert!(h.send().is_none());
    terminal(&h.answer(&first, 1, Err(Error::Forbidden)), 4, Error::Forbidden);
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 2 }));
    assert!(h.facts().contains(&Fact::Sent { priority: Priority::Fresh }));
}
#[test]
fn reserve_keeps_a_share_for_keep_and_slow() {
    let mut h = Harness::new(LIMITS);
    h.queued(1, Priority::Slow);
    h.queued(2, Priority::Keep);
    h.queued(3, Priority::Write);
    h.read(4);
    h.read(5);
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 4 }));
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 5 }));
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 2 }));
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 1 }));
    assert!(h.send().is_none());
    assert_eq!(h.domain.next_deadline(), Some(at(160)));
    h.env.now = at(160);
    h.fire();
    assert_eq!(h.one().op, Op::Write(Write::Close { number: 3 }));
}
#[test]
fn reserve_is_available_to_fresh_reads_when_background_is_idle() {
    let mut h = Harness::new(LIMITS);
    for owner in 1..=5_u64 {
        h.read(owner);
    }
    for _ in 0..4_u32 {
        drop(h.one());
    }
    assert!(h.send().is_none());
    assert!(h.facts().contains(&Fact::Spent { until: at(160) }));
}
#[test]
fn actual_cost_settles_failed_calls_and_priority_share() {
    for priority in [Priority::Fresh, Priority::Write, Priority::Keep, Priority::Slow] {
        for cost in [0_u32, 1, 4] {
            let mut h = Harness::new(LIMITS);
            h.queued(1, priority);
            let sent = h.one();
            let first = u32::from(priority == Priority::Fresh || priority == Priority::Write);
            assert_eq!(h.domain.calls.remaining(), (3, first));
            terminal(&h.answer(&sent, cost, Err(Error::Forbidden)), 1, Error::Forbidden);
            assert_eq!(h.domain.calls.remaining(), (4_u32.saturating_sub(cost), if first == 1 { cost } else { 0 }));
            h.read(2);
            assert_eq!(h.send().is_none(), cost == 4);
        }
    }
}
#[test]
fn late_refund_cannot_credit_the_new_window_and_extra_cost_debits_it() {
    for cost in [0_u32, 4] {
        let mut h = Harness::new(LIMITS);
        h.read(1);
        let old = h.one();
        h.env.now = at(161);
        h.read(2);
        let current = h.one();
        assert_eq!(h.domain.calls.remaining(), (3, 1));
        drop(h.answer(&old, cost, Err(Error::Forbidden)));
        assert_eq!(h.domain.calls.remaining(), if cost == 0 { (3, 1) } else { (0, 4) });
        drop(h.answer(&current, 1, Err(Error::Forbidden)));
    }
}
#[test]
fn zero_cost_refund_reopens_the_last_reservation() {
    let mut h = Harness::new(Limits { rate: 1, reserve: 0, ..LIMITS });
    h.read(1);
    let sent = h.one();
    h.read(2);
    assert!(h.send().is_none());
    terminal(&h.answer(&sent, 0, Err(Error::Forbidden)), 1, Error::Forbidden);
    assert_eq!(h.domain.calls.remaining(), (1, 0));
    assert_eq!(h.one().op, Op::Read(Read::Pull { number: 2 }));
}
#[test]
fn rate_refusal_stops_every_class_and_retries_with_a_distinct_call_token() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    let first = h.one();
    h.queued(2, Priority::Write);
    h.queued(3, Priority::Keep);
    h.queued(4, Priority::Slow);
    assert!(h.answer(&first, 1, Err(Error::RateLimited { after: Duration::from_secs(40) })).is_empty());
    assert!(h.send().is_none());
    h.env.now = at(139);
    h.fire();
    assert!(h.send().is_none());
    h.env.now = at(140);
    h.fire();
    let retry = h.one();
    assert_eq!(retry.op, first.op);
    assert_ne!(retry.call, first.call);
    terminal(&h.answer(&retry, 1, Err(Error::Forbidden)), 1, Error::Forbidden);
    assert!(h.facts().contains(&Fact::Limited { reset: at(140) }));
}
#[test]
fn overlapping_rate_refusals_keep_the_later_reset() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    h.read(2);
    let first = h.one();
    let second = h.one();
    drop(h.answer(&first, 1, Err(Error::RateLimited { after: Duration::from_secs(40) })));
    drop(h.answer(&second, 1, Err(Error::RateLimited { after: Duration::from_secs(10) })));
    assert_eq!(h.domain.next_deadline(), Some(at(140)));
}
#[test]
fn full_and_oversized_admission_answer_once_without_an_api_call() {
    let mut h = Harness::new(Limits { pending: 1, calls: 1, ..LIMITS });
    h.read(1);
    terminal(&h.step(Event::Read { owner: Token::new(2), repository: REPO, read: Read::Settings }), 2, Error::Busy);
    let value = skein_lib::bytes::zeroed(257);
    terminal(
        &h.step(Event::Read { owner: Token::new(3), repository: REPO, read: Read::Branch { branch: value } }),
        3,
        Error::TooLarge,
    );
    let sent = h.one();
    assert!(h.send().is_none());
    drop(h.answer(&sent, 1, Err(Error::Forbidden)));
    h.read(4);
    drop(h.one());
}
#[test]
fn malformed_item_identity_order_or_kind_finishes_as_a_typed_failure() {
    for items in [
        Box::new([listing_item(1, 100, api::Kind::Issue), listing_item(1, 101, api::Kind::Issue)]),
        Box::new([listing_item(1, 101, api::Kind::Issue), listing_item(2, 100, api::Kind::Issue)]),
        Box::new([listing_item(2, 100, api::Kind::Issue), listing_item(1, 100, api::Kind::Issue)]),
        Box::new([listing_item(1, 100, api::Kind::Issue), listing_item(2, 100, api::Kind::Pull)]),
    ] {
        let mut h = Harness::new(LIMITS);
        drop(h.step(Event::Read {
            owner: Token::new(1),
            repository: REPO,
            read: Read::Items { since: at(100), page: 1, kind: Some(api::Kind::Issue) },
        }));
        let sent = h.one();
        terminal(&h.answer(&sent, 1, Ok(Answer::Items { items, more: false, now: at(101) })), 1, Error::InvalidAnswer);
        assert!(h.send().is_none());
    }
}
#[test]
fn arbitrary_item_since_tolerates_the_providers_coarse_inclusive_overlap() {
    let mut h = Harness::new(LIMITS);
    drop(h.step(Event::Read {
        owner: Token::new(1),
        repository: REPO,
        read: Read::Items { since: at(100).saturating_add(Duration::from_millis(900)), page: 1, kind: None },
    }));
    let sent = h.one();
    let answer = Answer::Items {
        items: Box::new([listing_item(1, 100, api::Kind::Issue), listing_item(2, 100, api::Kind::Issue)]),
        more: false,
        now: at(101),
    };
    assert_eq!(
        h.answer(&sent, 1, Ok(answer.clone())).as_ref(),
        &[Request::Read { owner: Token::new(1), result: Ok(answer) }],
    );
}
#[test]
fn input_rows_are_checked_before_scanning_or_forwarding_them() {
    let mut h = Harness::new(LIMITS);
    assert!(
        h.step(Event::Read {
            owner: Token::new(1),
            repository: REPO,
            read: Read::Compare { before: [1; 32], after: [2; 32] }
        })
        .is_empty()
    );
    let sent = h.one();
    let answer = Answer::Compare {
        before: [1; 32],
        after: [2; 32],
        contains_before: false,
        files: Box::new([]),
        commits: Box::new([[0; 32]; 5]),
    };
    terminal(&h.answer(&sent, 1, Ok(answer)), 1, Error::TooLarge);
}
#[test]
fn an_answer_with_the_wrong_shape_is_typed_failure() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    let sent = h.one();
    terminal(&h.answer(&sent, 1, Ok(Answer::Done)), 1, Error::InvalidAnswer);
}
#[test]
fn files_at_a_different_head_are_refused_and_deletions_survive() {
    let mut h = Harness::new(LIMITS);
    let read = Read::PullFiles { number: 1, head: [1; 32], page: 1 };
    drop(h.step(Event::Read { owner: Token::new(1), repository: REPO, read: read.clone() }));
    let sent = h.one();
    terminal(
        &h.answer(&sent, 1, Ok(Answer::PullFiles { head: [2; 32], files: Box::new([]), more: false })),
        1,
        Error::InvalidAnswer,
    );
    drop(h.step(Event::Read { owner: Token::new(2), repository: REPO, read }));
    let sent = h.one();
    let file = api::File { path: Box::from(&b"deleted"[..]), before: Some(Box::from(&b"contents"[..])), after: None };
    let answer = Answer::PullFiles { head: [1; 32], files: Box::new([file]), more: false };
    assert_eq!(
        h.answer(&sent, 1, Ok(answer.clone())).as_ref(),
        &[Request::Read { owner: Token::new(2), result: Ok(answer) }]
    );
}
#[test]
fn job_logs_are_bounded_and_pinned_to_the_failing_attempt() {
    let mut h = Harness::new(LIMITS);
    let attempt = api::JobAttempt { head: [1; 32], run: 7, job: 8, attempt: 2 };
    let read = Read::Job { attempt, max_bytes: 64 };
    drop(h.step(Event::Read { owner: Token::new(1), repository: REPO, read: read.clone() }));
    let sent = h.one();
    let answer = Answer::Job {
        attempt,
        step: Some(api::Step { number: 1, name: Box::from(&b"build"[..]) }),
        log: Box::from(&b"failed job log"[..]),
        truncated: false,
    };
    assert_eq!(
        h.answer(&sent, 1, Ok(answer.clone())).as_ref(),
        &[Request::Read { owner: Token::new(1), result: Ok(answer) }]
    );
    drop(h.step(Event::Read { owner: Token::new(2), repository: REPO, read: read.clone() }));
    let sent = h.one();
    terminal(
        &h.answer(
            &sent,
            1,
            Ok(Answer::Job {
                attempt: api::JobAttempt { attempt: 3, ..attempt },
                step: None,
                log: Box::new([]),
                truncated: false,
            }),
        ),
        2,
        Error::InvalidAnswer,
    );
    drop(h.step(Event::Read { owner: Token::new(3), repository: REPO, read: read.clone() }));
    let sent = h.one();
    terminal(
        &h.answer(&sent, 1, Ok(Answer::Job { attempt, step: None, log: Box::from([b'x'; 65]), truncated: true })),
        3,
        Error::InvalidAnswer,
    );
    drop(h.step(Event::Read { owner: Token::new(4), repository: REPO, read }));
    let sent = h.one();
    terminal(&h.answer(&sent, 1, Err(Error::MissingJob)), 4, Error::MissingJob);
    terminal(
        &h.step(Event::Read {
            owner: Token::new(5),
            repository: REPO,
            read: Read::Job { attempt, max_bytes: LIMITS.answer_bytes + 1 },
        }),
        5,
        Error::TooLarge,
    );
}
#[test]
fn diagnostics_can_overflow_without_changing_admission() {
    let mut h = Harness::new(Limits { facts: 0, ..LIMITS });
    h.read(1);
    let sent = h.one();
    terminal(&h.answer(&sent, 1, Err(Error::Forbidden)), 1, Error::Forbidden);
    assert_eq!(h.domain.facts_lost(), 2);
    assert!(h.domain.pop_fact().is_none());
}
#[test]
fn limits_reject_incoherent_budgets_and_account_for_capacity() {
    assert!(crate::valid(&LIMITS));
    assert!(worst_case(&Limits { pending: 9, ..LIMITS }).unwrap() > worst_case(&LIMITS).unwrap());
}

#[test]
fn fresh_reads_retry_transient_failures_and_finish_once_at_the_bound() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    let mut previous = None;
    for attempt in 1..=LIMITS.read_attempts {
        let sent = h.one();
        assert_ne!(Some(sent.call), previous);
        previous = Some(sent.call);
        let told = h.answer(&sent, 1, Err(Error::Unavailable));
        if attempt == LIMITS.read_attempts {
            terminal(&told, 1, Error::Unavailable);
        } else {
            assert!(told.is_empty());
            assert!(h.send().is_none());
            let due = h.domain.next_deadline().expect("retry armed");
            assert!(due > h.env.now);
            h.env.now = due;
            h.fire();
        }
    }
    assert!(h.send().is_none());
    assert!(h.domain.next_deadline().is_none());
}

#[test]
fn a_refused_read_is_terminal_and_never_retried() {
    let mut h = Harness::new(LIMITS);
    h.read(1);
    let sent = h.one();
    terminal(&h.answer(&sent, 1, Err(Error::Refused)), 1, Error::Refused);
    assert!(h.send().is_none());
    assert!(h.domain.next_deadline().is_none());
}

#[test]
fn a_rate_refusal_does_not_spend_a_fresh_reads_retry_attempt() {
    let mut h = Harness::new(Limits { read_attempts: 1, ..LIMITS });
    h.read(1);
    let sent = h.one();
    assert!(h.answer(&sent, 1, Err(Error::RateLimited { after: Duration::from_secs(1) })).is_empty());
    h.env.now = h.domain.next_deadline().expect("reset armed");
    h.fire();
    let retry = h.one();
    terminal(&h.answer(&retry, 1, Err(Error::Timeout)), 1, Error::Timeout);
    assert!(h.domain.next_deadline().is_none());
}

#[test]
fn confirmed_write_terminals_do_not_invent_update_commits_or_branch_success() {
    let update = Op::Write(Write::Update { number: 1, style: api::UpdateStyle::Merge });
    assert!(crate::api::accepts(&update, &Answer::Done));
    assert!(!crate::api::accepts(&update, &Answer::Merged([1; 32])));
    let branch = Op::Write(Write::CreateBranch { branch: Box::from(&b"topic"[..]), commit: [1; 32] });
    assert!(crate::api::accepts(&branch, &Answer::Branch(api::BranchCreation::Created)));
    assert!(crate::api::accepts(&branch, &Answer::Branch(api::BranchCreation::Exists)));
    assert!(!crate::api::accepts(&branch, &Answer::Done));
    let edit = Op::Write(Write::Edit { number: 1, title: None, body: Some(Box::from(&b"body"[..])) });
    assert!(crate::bounds::op(&edit, &Limits { op_bytes: 4, ..LIMITS }));
    assert!(!crate::bounds::op(&edit, &Limits { op_bytes: 3, ..LIMITS }));
}
#[test]
fn protection_nested_contexts_and_checks_provider_details_are_bounded() {
    let contexts = Box::new([Box::from(&b"ci"[..]), Box::from(&b"gate"[..])]);
    let protection = Answer::Protection(Some(api::Protection {
        queue: false,
        signatures: false,
        linear: false,
        code_owners: false,
        bypass: api::Bypass::Never,
        branch: Box::from(&b"main"[..]),
        contexts,
        approvals: 2,
        dismiss_stale: true,
    }));
    assert!(crate::bounds::answer(&protection, &LIMITS));
    assert!(!crate::bounds::answer(&protection, &Limits { rows: 1, ..LIMITS }));
    assert!(!crate::bounds::answer(&protection, &Limits { answer_bytes: 41, ..LIMITS }));
    let status = api::Status {
        at: Time::ZERO,
        context: Box::from(&b"ci"[..]),
        check: api::Check::Failed,
        description: Box::from(&b"failure"[..]),
        url: Box::from(&b"/job/1"[..]),
        job: None,
    };
    let checks = Answer::Checks(Box::new([status]));
    assert!(crate::api::accepts(&Op::Read(Read::Checks { commit: [1; 32] }), &checks));
    assert!(crate::bounds::answer(&checks, &LIMITS));
    assert!(!crate::bounds::answer(&checks, &Limits { rows: 0, ..LIMITS }));
}
#[test]
fn comparisons_must_return_the_requested_endpoints() {
    let mut h = Harness::new(LIMITS);
    drop(h.step(Event::Read {
        owner: Token::new(1),
        repository: REPO,
        read: Read::Compare { before: [1; 32], after: [2; 32] },
    }));
    let sent = h.one();
    terminal(
        &h.answer(
            &sent,
            1,
            Ok(Answer::Compare {
                before: [0; 32],
                after: [2; 32],
                contains_before: false,
                files: Box::new([]),
                commits: Box::new([]),
            }),
        ),
        1,
        Error::InvalidAnswer,
    );
}
#[test]
fn entry_refusal_is_observable_even_without_diagnostics() {
    let mut h = Harness::new(Limits { entries: 1, facts: 0, ..LIMITS });
    h.domain = configured(&h.env.limits);
    assert!(h.step(Event::Restored { clock: crate::RecoveryClock::Monotonic }).is_empty());
    for number in 1..=2_u64 {
        let out = h.step(Event::Make {
            entry: crate::Entry {
                number,
                task: 7,
                repository: REPO,
                effect: crate::Effect { write: Write::Close { number: 9 }, condition: crate::Condition::None },
                start: None,
                attempt: None,
                failures: 0,
                queued: false,
            },
        });
        if number == 2 {
            assert_eq!(
                out.as_ref(),
                &[Request::Outcome { entry: 2, task: 7, outcome: crate::Outcome::Failed(Error::Busy) }]
            );
        }
    }
    assert!(h.domain.pop_fact().is_none());
}
#[test]
fn an_answer_for_another_pull_cannot_satisfy_a_fresh_read() {
    let mut h = Harness::new(LIMITS);
    h.read(8);
    let call = h.one();
    let answer = api::Pull {
        queued: None,
        dequeued: None,
        reviewers: Box::new([]),
        number: 9,
        state: api::State::Open,
        head: Box::new([]),
        base: Box::new([]),
        commit: [1; 32],
        base_commit: None,
        merged: None,
        mergeable: true,
        ci: api::Ci::None,
    };
    let out =
        h.step(Event::Answered { call: call.call, cost: 1, free: 0, rate: None, result: Ok(Answer::Pull(answer)) });
    assert_eq!(out.as_ref(), &[Request::Read { owner: Token::new(8), result: Err(Error::InvalidAnswer) }]);
}

#[test]
fn deployment_config_and_decoded_key_frames_are_checked() {
    let d = configured(&LIMITS);
    assert!(crate::identity::historical(&d, b"\x01\x00\x03kiteffect"));
    for malformed in [
        &b"\x01\x00\x03kit"[..],
        &b"\x01\x00\x04kiteffect"[..],
        &b"\x01\x00\xffkiteffect"[..],
        &b"\x02\x00\x03kiteffect"[..],
        &b"kiteffect"[..],
    ] {
        assert!(!crate::identity::historical(&d, malformed));
    }
    let configs = [
        crate::Config { namespace: Box::new([]), writers: Box::new([crate::Writer { forge: REPO.forge, author: 1 }]) },
        crate::Config {
            namespace: Box::from(&b"kit"[..]),
            writers: Box::new([crate::Writer { forge: REPO.forge, author: 0 }]),
        },
        crate::Config {
            namespace: Box::from(&b"kit"[..]),
            writers: Box::new([
                crate::Writer { forge: REPO.forge, author: 1 },
                crate::Writer { forge: REPO.forge, author: 2 },
            ]),
        },
    ];
    for config in configs {
        assert_eq!(Domain::configured(&LIMITS, 0, config).expect_err("invalid config"), Error::TooLarge);
    }
    let config = crate::Config {
        namespace: Box::from(&b"kit"[..]),
        writers: Box::new([crate::Writer { forge: REPO.forge, author: 1 }]),
    };
    assert_eq!(
        Domain::configured(&Limits { op_bytes: 1, ..LIMITS }, 0, config).expect_err("oversized config"),
        Error::TooLarge
    );
}

#[test]
fn file_reads_are_pinned_bounded_and_charged_to_the_fresh_lane() {
    for (head, path, bytes, accepted) in [
        ([3; 32], b"file".as_slice(), b"abc".as_slice(), true),
        ([4; 32], b"file", b"abc", false),
        ([3; 32], b"other", b"abc", false),
        ([3; 32], b"file", b"abcd", false),
    ] {
        let mut h = Harness::new(LIMITS);
        h.step(Event::Read {
            owner: Token::new(7),
            repository: REPO,
            read: Read::File { head: [3; 32], path: Box::from(&b"file"[..]), max_bytes: 3 },
        });
        let sent = h.send().expect("fresh read admitted");
        let answer = Answer::File { head, path: Box::from(path), bytes: Box::from(bytes), truncated: true };
        let out = h.answer(&sent, 1, Ok(answer.clone()));
        let result = if accepted { Ok(answer) } else { Err(Error::InvalidAnswer) };
        assert_eq!(out.as_ref(), &[Request::Read { owner: Token::new(7), result }]);
        assert!(h.facts().contains(&Fact::Sent { priority: Priority::Fresh }));
    }
    let mut h = Harness::new(LIMITS);
    let refused = h.step(Event::Read {
        owner: Token::new(1),
        repository: REPO,
        read: Read::File { head: [3; 32], path: Box::from(&b"file"[..]), max_bytes: LIMITS.answer_bytes + 1 },
    });
    assert_eq!(refused.as_ref(), &[Request::Read { owner: Token::new(1), result: Err(Error::TooLarge) }]);
    assert!(h.send().is_none());
}

#[test]
fn a_job_answers_step_name_counts_against_answer_bytes() {
    let attempt = api::JobAttempt { head: [1; 32], run: 7, job: 8, attempt: 2 };
    for (length, accepted) in [(4095, true), (4096, false)] {
        let mut h = Harness::new(LIMITS);
        drop(h.step(Event::Read {
            owner: Token::new(1),
            repository: REPO,
            read: Read::Job { attempt, max_bytes: 4096 },
        }));
        let sent = h.one();
        let answer = Answer::Job {
            attempt,
            step: Some(api::Step { number: 1, name: Box::new([b'x']) }),
            log: Box::from(&[b'x'; 4096][..length]),
            truncated: false,
        };
        let expected = if accepted { Ok(answer.clone()) } else { Err(Error::TooLarge) };
        assert_eq!(
            h.answer(&sent, 1, Ok(answer)).as_ref(),
            &[Request::Read { owner: Token::new(1), result: expected }],
            "Length {length}"
        );
    }
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_a_zero_settle_window() {
    let limits = Limits { settle: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
fn vocabulary_queued_is_accepted_for_merge() {
    let merge = Op::Write(Write::Merge { number: 1, head: [1; 32], how: api::MergeHow::Style });
    assert!(crate::api::accepts(&merge, &Answer::Queued));
}

#[test]
fn a_directory_answer_is_pinned_and_charged_to_its_read() {
    // A one-byte entry name plus the fixed 32-byte entry charge.
    const ENTRY_BYTES: u32 = 33;
    for (head, path, name, accepted) in [
        ([3; 32], b"src".as_slice(), b"x".as_slice(), true),
        ([4; 32], b"src", b"x", false),
        ([3; 32], b"other", b"x", false),
        ([3; 32], b"src", b"xx", false),
    ] {
        let mut h = Harness::new(LIMITS);
        h.step(Event::Read {
            owner: Token::new(7),
            repository: REPO,
            read: Read::File { head: [3; 32], path: Box::from(*b"src"), max_bytes: ENTRY_BYTES },
        });
        let sent = h.send().expect("directory read admitted");
        let answer = Answer::Directory {
            head,
            path: Box::from(path),
            entries: Box::new([api::Entry { name: Box::from(name), kind: api::EntryKind::File }]),
            truncated: false,
        };
        let result = if accepted { Ok(answer.clone()) } else { Err(Error::InvalidAnswer) };
        assert_eq!(h.answer(&sent, 1, Ok(answer)).as_ref(), &[Request::Read { owner: Token::new(7), result }]);
    }
}

#[test]
fn an_empty_file_path_reads_the_root_directory() {
    let mut h = Harness::new(LIMITS);
    h.step(Event::Read {
        owner: Token::new(7),
        repository: REPO,
        read: Read::File { head: [3; 32], path: Box::from([]), max_bytes: 1 },
    });
    let sent = h.send().expect("root read admitted");
    let answer = Answer::Directory { head: [3; 32], path: Box::from([]), entries: Box::from([]), truncated: false };
    assert_eq!(
        h.answer(&sent, 1, Ok(answer.clone())).as_ref(),
        &[Request::Read { owner: Token::new(7), result: Ok(answer) }]
    );
}

#[test]
fn directory_bytes_do_not_use_the_row_limit() {
    // Two one-byte names with 32 bytes of fixed storage each.
    const PAIR_BYTES: u32 = 66;
    let mut h = Harness::new(Limits { rows: 1, ..LIMITS });
    h.step(Event::Read {
        owner: Token::new(7),
        repository: REPO,
        read: Read::File { head: [3; 32], path: Box::from([]), max_bytes: PAIR_BYTES },
    });
    let sent = h.send().expect("directory read admitted");
    let answer = Answer::Directory {
        head: [3; 32],
        path: Box::from([]),
        entries: Box::new([
            api::Entry { name: Box::from(*b"x"), kind: api::EntryKind::File },
            api::Entry { name: Box::from(*b"y"), kind: api::EntryKind::Directory },
        ]),
        truncated: false,
    };
    assert_eq!(
        h.answer(&sent, 1, Ok(answer.clone())).as_ref(),
        &[Request::Read { owner: Token::new(7), result: Ok(answer) }]
    );
}
mod turn;

#[test]
fn valid_accepts_test_limits() {
    assert!(crate::valid(&LIMITS));
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_pending() {
    let limits = Limits { pending: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_calls() {
    let limits = Limits { calls: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_calls_past_pending() {
    let limits = Limits { calls: 9, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_rate() {
    let limits = Limits { rate: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_reserve_past_rate() {
    let limits = Limits { reserve: 5, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_window() {
    let limits = Limits { window: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_read_attempts() {
    let limits = Limits { read_attempts: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_backoff() {
    let limits = Limits { backoff: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_backoff_past_its_ceiling() {
    let limits = Limits { backoff_max: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_entries() {
    let limits = Limits { entries: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_write_attempts() {
    let limits = Limits { write_attempts: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_lifetime() {
    let limits = Limits { lifetime: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_clock_margin() {
    let limits = Limits { clock_margin: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_rows() {
    let limits = Limits { rows: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_inbox() {
    let limits = Limits { inbox: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_resources() {
    let limits = Limits { resources: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_repositories() {
    let limits = Limits { repositories: 0, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_poll() {
    let limits = Limits { poll: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_poll_past_its_ceiling() {
    let limits = Limits { poll_max: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_hinted() {
    let limits = Limits { hinted: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_zero_slow() {
    let limits = Limits { slow: Duration::ZERO, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_alarm_capacity_past_u32() {
    let limits = Limits { pending: u32::MAX, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
#[should_panic(expected = "forge client limits are valid")]
fn valid_refuses_keep_capacity_past_u32() {
    let limits = Limits { resources: u32::MAX / 2, ..LIMITS };
    assert!(!crate::valid(&limits));
    let _domain = Domain::new(&limits);
}

#[test]
fn valid_ignores_heap_cached_operations() {
    let limits = Limits { resources: 1_000_000_000, op_bytes: u32::MAX, ..LIMITS };
    assert!(crate::valid(&limits));
    assert_eq!(worst_case(&limits), None);
}
