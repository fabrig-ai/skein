use crate::api::Answer;

#[test]
fn locate_bytes_counts_owner_and_name() {
    assert_eq!(crate::locate::bytes(b"ab", b"cde"), Some(5));
}

// The preserved positive locate fixture identifies repository seven.
const LOCATED_REPOSITORY: u32 = 7;

#[test]
fn locate_accepts_a_nonzero_repository() {
    assert!(crate::locate::accepts(&Answer::Located { repository: LOCATED_REPOSITORY }));
}

#[test]
fn locate_refuses_zero_and_answers_without_a_repository() {
    for answer in [Answer::Located { repository: 0 }, Answer::Done] {
        assert!(!crate::locate::accepts(&answer), "{answer:?}");
    }
}

use crate::api;
use crate::{
    forgejo::{plans::tables::Tables, tests::fixtures},
    marker,
};
use alloc::boxed::Box;
const WORD: &[u8] = b"kit";

#[test]
fn a_marker_round_trips_its_key_and_keeps_the_text() {
    let sixty_four = [0xab; 64];
    let rows: [(&str, &[u8], &[u8]); 4] = [
        ("empty key", b"", b"text"),
        ("one byte", &[0x01], b"text"),
        ("64 bytes", &sixty_four, b"text"),
        ("text holds a marker", &[0xff], b"<!-- kit:key 01 -->\ntext"),
    ];
    for (name, key, text) in rows {
        let encoded = marker::encode(WORD, key, text, 1024).expect("fixture fits");
        assert_eq!(marker::decode(WORD, &encoded), Some(Box::from(key)), "{name}");
        assert!(encoded.ends_with(text), "{name}: text kept whole");
        let size = u32::try_from(encoded.len()).expect("fixture length");
        assert_eq!(marker::encode(WORD, key, text, size), Some(encoded), "{name}: exact limit");
        assert_eq!(marker::encode(WORD, key, text, size.saturating_sub(1)), None, "{name}: one short");
    }
    let marked_body_bytes = marker::length(WORD, 2).expect("fixture length").checked_add(4).expect("fixture text");
    assert_eq!(
        marker::encode(WORD, &[0xab, 0x01], b"body", marked_body_bytes),
        Some(Box::from(&b"<!-- kit:key ab01 -->\nbody"[..]))
    );
    assert_eq!(marker::length(WORD, u32::MAX), None);
}

#[test]
fn a_body_without_a_well_formed_marker_has_no_key() {
    let rows: [(&str, &[u8]); 5] = [
        ("uppercase", b"<!-- kit:key AB -->\n"),
        ("odd", b"<!-- kit:key a -->\n"),
        ("no line feed", b"<!-- kit:key ab -->"),
        ("leading space", b" <!-- kit:key ab -->\n"),
        ("after text", b"text<!-- kit:key ab -->\n"),
    ];
    for (name, body) in rows {
        assert_eq!(marker::decode(WORD, body), None, "{name}");
    }
}

#[test]
fn a_full_table_drops_the_entry_used_longest_ago() {
    let mut tables = Tables::new(2, 2);
    let rows = [
        ("repositories stay separate by forge", api::Repository { forge: 2, repository: 1 }),
        ("repositories stay separate by id", api::Repository { forge: 1, repository: 2 }),
    ];
    for (name, second) in rows {
        tables.keep_repository(fixtures::REPO, fixtures::specimen());
        tables.keep_repository(second, fixtures::specimen());
        assert_eq!(tables.repository(fixtures::REPO), Some(&fixtures::specimen()), "{name}: use first");
        tables.keep_repository(api::Repository { forge: 3, repository: 3 }, fixtures::specimen());
        assert_eq!(tables.repository(second), None, "{name}: oldest evicted");
        assert_eq!(tables.repository(fixtures::REPO), Some(&fixtures::specimen()), "{name}: recent kept");
    }
}

#[test]
fn a_full_user_table_keeps_the_most_recent_login_and_replaces_in_place() {
    let mut tables = Tables::new(2, 2);
    tables.keep_user(1, 1, Box::from(&b"first"[..]));
    tables.keep_user(2, 1, Box::from(&b"other-forge"[..]));
    assert_eq!(tables.user(1, 1), Some(&b"first"[..]), "first touched");
    tables.keep_user(1, 2, Box::from(&b"third"[..]));
    assert_eq!(tables.user(2, 1), None, "oldest evicted");
    tables.keep_user(1, 1, Box::from(&b"renamed"[..]));
    assert_eq!(tables.user(1, 1), Some(&b"renamed"[..]), "existing login refreshed");
    assert_eq!(tables.user(1, 2), Some(&b"third"[..]), "replacement evicts nothing");
}

fn measured(answer: &Answer, bytes: u64) {
    assert_eq!(crate::measure::answer(answer), Some(bytes));
    assert_eq!(crate::forgejo::plans::reads::size::answer(answer, u32::MAX), Ok(bytes));
}

#[test]
fn measure_answer_summary() {
    let item = api::Summary {
        labels: Box::new([]),
        number: 1,
        kind: api::Kind::Issue,
        state: api::State::Open,
        author: 1,
        key: None,
        title: Box::from(WORD),
        body: Box::new([]),
        updated: skein_lib::Time::ZERO,
    };
    measured(&Answer::Items { items: Box::new([item.clone()]), more: false, now: skein_lib::Time::ZERO }, 131);
    measured(&Answer::Item { item, comments: Box::new([]), more: false }, 3);
}

#[test]
fn measure_answer_review() {
    let review = api::Review {
        id: 1,
        provenance: api::Provenance::Original,
        revision: 1,
        author: 1,
        verdict: api::Verdict::Comment,
        commit: [1; 32],
        key: None,
        body: Box::from(WORD),
        at: skein_lib::Time::ZERO,
        official: false,
    };
    measured(&Answer::Reviews { reviews: Box::new([review]), more: false }, 131);
}

#[test]
fn measure_answer_status() {
    let status = api::Status {
        at: skein_lib::Time::ZERO,
        context: Box::from(WORD),
        check: api::Check::Passed,
        description: Box::from(&b"ab"[..]),
        url: Box::from(&b"x"[..]),
        job: None,
    };
    for answer in [
        Answer::Statuses { ci: api::Ci::Passed, statuses: Box::new([status.clone()]), more: false },
        Answer::Checks(Box::new([status])),
    ] {
        measured(&answer, 166);
    }
}

#[test]
fn measure_answer_comment() {
    let item = api::Summary {
        labels: Box::new([]),
        number: 1,
        kind: api::Kind::Issue,
        state: api::State::Open,
        author: 1,
        key: None,
        title: Box::new([]),
        body: Box::new([]),
        updated: skein_lib::Time::ZERO,
    };
    let comment = api::Comment {
        id: 1,
        provenance: api::Provenance::Original,
        author: 1,
        created: skein_lib::Time::ZERO,
        revision: 1,
        key: Some(Box::from(&b"a"[..])),
        body: Box::from(WORD),
    };
    measured(&Answer::Item { item, comments: Box::new([comment]), more: false }, 100);
}

#[test]
fn measure_answer_remark() {
    let remark = api::Remark { id: 1, author: 1, path: Box::from(WORD), line: 1, body: Box::from(&b"ab"[..]) };
    measured(&Answer::Remarks { remarks: Box::new([remark]), more: false }, 69);
}

#[test]
fn measure_answer_file() {
    let file =
        api::File { path: Box::from(WORD), before: Some(Box::from(&b"a"[..])), after: Some(Box::from(&b"bc"[..])) };
    for answer in [
        Answer::PullFiles { head: [1; 32], files: Box::new([file.clone()]), more: false },
        Answer::Compare {
            before: [1; 32],
            after: [2; 32],
            contains_before: true,
            files: Box::new([file]),
            commits: Box::new([]),
        },
    ] {
        measured(&answer, 70);
    }
}

#[test]
fn measure_answer_commit() {
    measured(
        &Answer::Compare {
            before: [1; 32],
            after: [2; 32],
            contains_before: true,
            files: Box::new([]),
            commits: Box::new([[1; 32]]),
        },
        32,
    );
    measured(&Answer::Commit([1; 32]), 0);
}

#[test]
fn measure_answer_collaborator() {
    measured(
        &Answer::Collaborators {
            collaborators: Box::new([api::Collaborator { user: 1, permission: api::Permission::Write }]),
            more: false,
        },
        16,
    );
}

#[test]
fn measure_answer_string() {
    let item = api::Summary {
        labels: Box::new([Box::from(WORD)]),
        number: 1,
        kind: api::Kind::Issue,
        state: api::State::Open,
        author: 1,
        key: None,
        title: Box::new([]),
        body: Box::new([]),
        updated: skein_lib::Time::ZERO,
    };
    measured(&Answer::Branches(Box::new([Box::from(WORD)])), 19);
    measured(&Answer::Item { item, comments: Box::new([]), more: false }, 19);
    measured(
        &Answer::Protection(Some(api::Protection {
            queue: false,
            signatures: false,
            linear: false,
            code_owners: false,
            bypass: api::Bypass::Never,
            branch: Box::from(WORD),
            contexts: Box::new([Box::from(&b"x"[..])]),
            approvals: 0,
            dismiss_stale: false,
        })),
        20,
    );
}

#[test]
fn measure_answer_reviewer() {
    measured(
        &Answer::Pull(api::Pull {
            queued: None,
            dequeued: None,
            reviewers: Box::new([7]),
            number: 1,
            state: api::State::Open,
            head: Box::from(WORD),
            base: Box::from(WORD),
            commit: [1; 32],
            base_commit: None,
            merged: None,
            mergeable: false,
            ci: api::Ci::None,
        }),
        14,
    );
}

#[test]
fn measure_op_reviewers() {
    const TWO_IDS_BYTES: u64 = 16;
    let op = api::Op::Write(api::Write::SetReviewers { number: 1, reviewers: Box::new([7, 8]) });
    assert_eq!(crate::measure::op(&op), Some(TWO_IDS_BYTES));
}

#[test]
fn measure_op_strings() {
    for (op, expected) in [
        (api::Op::Read(api::Read::Locate { owner: Box::from(&b"ab"[..]), name: Box::from(WORD) }), 5),
        (
            api::Op::Write(api::Write::OpenPull {
                title: Box::new([]),
                body: Box::new([]),
                head: Box::from(WORD),
                base: Box::from(WORD),
            }),
            6,
        ),
    ] {
        assert_eq!(crate::measure::op(&op), Some(expected), "{op:?}");
    }
}

use core::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[test]
fn host_names_include_only_nondefault_ports_and_bracket_ipv6() {
    type HostRow = (&'static str, SocketAddr, Option<&'static [u8]>, &'static [u8]);
    let rows: [HostRow; 5] = [
        ("plain default", SocketAddr::from((Ipv4Addr::LOCALHOST, 80)), None, b"127.0.0.1"),
        ("plain custom", SocketAddr::from((Ipv4Addr::LOCALHOST, 3000)), None, b"127.0.0.1:3000"),
        ("TLS default", SocketAddr::from((Ipv4Addr::LOCALHOST, 443)), Some(b"forge.example"), b"forge.example"),
        ("TLS custom", SocketAddr::from((Ipv4Addr::LOCALHOST, 3000)), Some(b"forge.example"), b"forge.example:3000"),
        ("IPv6", SocketAddr::from((Ipv6Addr::LOCALHOST, 80)), None, b"[0:0:0:0:0:0:0:1]"),
    ];
    for (name, address, server, expected) in rows {
        assert_eq!(crate::host::host(address, server), Some(Box::from(expected)), "{name}");
    }
}

#[test]
fn host_uses_names_and_omits_both_default_ports() {
    type HostRow = (core::net::IpAddr, Option<&'static [u8]>, &'static [u8]);
    let rows: [HostRow; 4] = [
        (Ipv4Addr::LOCALHOST.into(), Some(b"forge.example"), b"forge.example"),
        (Ipv6Addr::LOCALHOST.into(), Some(b"forge.example"), b"forge.example"),
        (Ipv4Addr::LOCALHOST.into(), None, b"127.0.0.1"),
        (Ipv6Addr::LOCALHOST.into(), None, b"[0:0:0:0:0:0:0:1]"),
    ];
    for (ip, name, expected) in rows {
        // Both transports use the same name argument, including TLS on 80
        // and named plaintext on 443 (forge.md, section 5.1; decision 25).
        for port in [80, 443] {
            assert_eq!(
                crate::host::host(SocketAddr::new(ip, port), name),
                Some(Box::from(expected)),
                "{ip}:{port} {name:?}"
            );
        }
    }
    assert_eq!(
        crate::host::host(SocketAddr::from((Ipv6Addr::LOCALHOST, 3000)), None),
        Some(Box::from(&b"[0:0:0:0:0:0:0:1]:3000"[..]))
    );
}

#[test]
fn vocabulary_validator_accepts_up_to_128_printable_bytes() {
    for length in 1..=api::VALIDATOR_BYTES {
        let bytes = [b'~'; api::VALIDATOR_BYTES];
        let value = bytes.get(..length).expect("within validator bound");
        let validator = api::Validator::new(value).expect("printable bytes fit");
        assert_eq!(validator.as_bytes(), value);
    }
    for byte in 0x20..=0x7e {
        assert_eq!(api::Validator::new(&[byte]).expect("printable ASCII").as_bytes(), &[byte]);
    }
}

#[test]
fn vocabulary_validator_rejects_129_bytes_empty_and_control_bytes() {
    assert_eq!(api::VALIDATOR_BYTES, 128);
    assert_eq!(api::Validator::new(&[b'a'; 129]), None);
    assert_eq!(api::Validator::new(b""), None);
    for byte in (0..=0x1f).chain(0x7f..=0xff) {
        assert_eq!(api::Validator::new(&[b'a', byte, b'z']), None, "byte {byte}");
    }
}

#[test]
fn vocabulary_rate_carries_limit_remaining_and_reset() {
    let reset = skein_lib::Wall::from_nanos(3_600_000_000_000);
    let rate = api::Rate { limit: 5000, remaining: 4999, reset_at: reset };
    assert_eq!((rate.limit, rate.remaining, rate.reset_at), (5000, 4999, reset));
}

#[test]
fn vocabulary_queued_is_not_a_merged_commit() {
    assert_ne!(Answer::Queued, Answer::Merged([1; 32]));
}

fn vocabulary_forgejo_pull() -> api::Pull {
    use crate::forgejo::plans::plan::Step;
    let mut run = fixtures::Run::known(api::Op::Read(api::Read::Pull { number: 1 }));
    run.begin();
    run.answer(200, fixtures::PULL);
    let Step::Done(Ok(Answer::Pull(pull))) = run.answer(200, fixtures::EMPTY_STATUS) else {
        panic!("Forgejo pull read completes");
    };
    pull
}

#[test]
fn vocabulary_pull_queued_and_dequeued_default_to_none_in_forgejo() {
    let pull = vocabulary_forgejo_pull();
    assert_eq!((pull.queued, pull.dequeued), (None, None));
}

#[test]
fn vocabulary_dequeued_bound_is_256_bytes() {
    assert_eq!(api::DEQUEUED_BYTES, 256);
    assert_eq!(api::DEQUEUED_BYTES.cmp(&api::VALIDATOR_BYTES), core::cmp::Ordering::Greater);
}

#[test]
fn vocabulary_measure_counts_dequeued_bytes_and_not_queued() {
    let mut pull = vocabulary_forgejo_pull();
    let baseline = crate::measure::pull(&pull).expect("fixture fits");
    pull.queued = Some(1);
    assert_eq!(crate::measure::pull(&pull), Some(baseline));
    let reason = b"The head moved.";
    pull.dequeued = Some(Box::from(reason.as_slice()));
    let expected = baseline.checked_add(u64::try_from(reason.len()).expect("short reason")).expect("fixture fits");
    assert_eq!(crate::measure::pull(&pull), Some(expected));
    assert_eq!(crate::measure::answer(&Answer::Pull(pull)), Some(expected));
}

#[test]
fn vocabulary_measure_of_queued_is_zero() {
    assert_eq!(crate::measure::answer(&Answer::Queued), Some(0));
}

fn vocabulary_app_credential() -> crate::credential::Credential {
    crate::credential::Credential::App { id: 1, key: skein_lib::Secret::new(Box::new([])) }
}

#[test]
fn vocabulary_app_credential_gives_no_forgejo_authorization() {
    assert_eq!(crate::forgejo::credential::authorization(&vocabulary_app_credential()), None);
}

#[test]
fn vocabulary_app_credential_gives_no_github_authorization() {
    assert_eq!(crate::github::credential::authorization(&vocabulary_app_credential()), None);
}

#[test]
fn turn_measures_count_nested_strings_and_list_widths() {
    use crate::api::{Conclusion, Entry, EntryKind, Event, Input, Run, RunJob, RunState};
    use skein_lib::Time;
    let run = Run {
        id: 1,
        number: 1,
        workflow: Box::from(*b"ci"),
        commit: [0; 32],
        event: Event::Dispatch,
        actor: 1,
        state: RunState::Completed,
        conclusion: Some(Conclusion::Failure),
        url: Box::from(*b"url"),
        created: Time::ZERO,
        jobs: Box::new([RunJob {
            id: 2,
            name: Box::from(*b"job"),
            attempt: 1,
            state: RunState::Completed,
            conclusion: Some(Conclusion::Failure),
            step: Some(api::Step { number: 1, name: Box::from(*b"step") }),
        }]),
    };
    // Each run holds five string bytes, one 64-byte job and seven job/step bytes.
    assert_eq!(crate::measure::answer(&Answer::Runs { runs: Box::new([run.clone(), run]), more: false }), Some(408));
    let entry = Entry { name: Box::from(*b"abc"), mode: 0o100_644, kind: EntryKind::Blob, object: [0; 32] };
    assert_eq!(
        crate::measure::answer(&Answer::Tree { entries: Box::new([entry.clone(), entry.clone(), entry]), more: false }),
        Some(201)
    );
    let input = Input { name: Box::from(*b"key"), value: Box::from(*b"receipt") };
    assert_eq!(
        crate::measure::op(&api::Op::Write(api::Write::Dispatch {
            workflow: Box::from(*b"ci"),
            branch: Box::from(*b"main"),
            inputs: Box::new([input.clone(), input])
        })),
        Some(90)
    );
    assert!(size_of::<Run>() <= crate::measure::RUN_BYTES);
    assert_eq!(crate::measure::RUN_BYTES, 128);
}
