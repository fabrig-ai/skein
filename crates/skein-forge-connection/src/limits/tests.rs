use super::{Listing, Shared, Unusable, check, largest_row, page_size};
use crate::tests::fixtures;
use alloc::boxed::Box;
use core::mem::size_of;
use skein_forge::api;
use skein_lib::{List, Time};

#[test]
fn each_listing_uses_its_largest_answer_row() {
    let limits = fixtures::limits();
    for (listing, row) in [
        (Listing::Items, fixtures::ROW_ITEMS),
        (Listing::Reviews, fixtures::ROW_REVIEWS),
        (Listing::Statuses, fixtures::ROW_STATUSES),
        (Listing::Remarks, fixtures::ROW_REMARKS),
        (Listing::Branches, fixtures::ROW_BRANCHES),
        (Listing::Collaborators, fixtures::ROW_COLLABORATORS),
        (Listing::PullFiles, fixtures::ROW_PULLFILES),
        (Listing::Compare, fixtures::ROW_COMPARE),
    ] {
        assert_eq!(
            skein_forge::measure::answer(&maximal_answer(listing)),
            Some(u64::from(row)),
            "{listing:?}: independent oracle"
        );
        assert_eq!(largest_row(listing, &limits), Some(u64::from(row)), "{listing:?}: production bound");
        for (answer_bytes, rows, cap, expected) in [
            (row - 1, fixtures::PAGE_ROWS, fixtures::PAGE_ROWS, fixtures::NO_PAGE_ROWS),
            (row, fixtures::PAGE_ROWS, fixtures::PAGE_ROWS, fixtures::ONE_PAGE_ROW),
            (fixtures::PAGE_ROWS * row, fixtures::PAGE_ROWS, fixtures::PAGE_ROWS, fixtures::PAGE_ROWS),
            (fixtures::PAGE_ROWS * row, fixtures::PAGE_ROWS, fixtures::ONE_PAGE_ROW, fixtures::ONE_PAGE_ROW),
            (fixtures::PAGE_ROWS * row, fixtures::ONE_PAGE_ROW, fixtures::PAGE_ROWS, fixtures::ONE_PAGE_ROW),
        ] {
            let shared = Shared { rows, answer_bytes, ..fixtures::shared() };
            assert_eq!(page_size(listing, cap, &limits, &shared), expected, "{listing:?}/{answer_bytes}/{rows}/{cap}");
        }
    }
}
#[test]
fn limits_check_admits_one_largest_row_of_every_listing() {
    let limits = fixtures::limits();
    let settings = fixtures::settings();
    for (answer_bytes, expected) in
        [(fixtures::MAX_ROW - 1, Err(Unusable::PageSize { listing: Listing::Items })), (fixtures::MAX_ROW, Ok(()))]
    {
        let shared = Shared { answer_bytes, ..fixtures::shared() };
        assert_eq!(check(&limits, &shared, &settings), expected);
    }
}
fn maximal_answer(listing: Listing) -> api::Answer {
    let limits = fixtures::limits();
    let string = usize::try_from(limits.string_bytes).expect("fixture string");
    let body = usize::try_from(limits.body_bytes).expect("fixture body");
    match listing {
        Listing::Items => {
            let mut labels = List::with_capacity(limits.inner);
            for _ in 0..limits.inner {
                labels.push(fixtures::filled(string, b'l')).expect("labels");
            }
            api::Answer::Items {
                items: Box::new([api::Summary {
                    labels: labels.into_boxed(),
                    number: 1,
                    kind: api::Kind::Issue,
                    state: api::State::Open,
                    author: fixtures::WRITER,
                    key: Some(fixtures::filled(body / 2, b'k')),
                    title: fixtures::filled(string, b't'),
                    body: fixtures::filled(body, b'b'),
                    updated: Time::ZERO,
                }]),
                more: false,
                now: Time::ZERO,
            }
        }
        Listing::Reviews => api::Answer::Reviews {
            reviews: Box::new([api::Review {
                id: 1,
                provenance: api::Provenance::Original,
                revision: 0,
                author: fixtures::WRITER,
                verdict: api::Verdict::Approve,
                commit: fixtures::BRANCH_HEAD,
                key: Some(fixtures::filled(body / 2, b'k')),
                body: fixtures::filled(body, b'b'),
                at: Time::ZERO,
                official: false,
            }]),
            more: false,
        },
        Listing::Statuses => api::Answer::Statuses {
            ci: api::Ci::Passed,
            statuses: Box::new([api::Status {
                at: Time::ZERO,
                context: fixtures::filled(string, b'c'),
                check: api::Check::Passed,
                description: fixtures::filled(string, b'd'),
                url: fixtures::filled(string, b'u'),
                job: None,
            }]),
            more: false,
        },
        Listing::Remarks => api::Answer::Remarks {
            remarks: Box::new([api::Remark {
                id: 1,
                author: fixtures::WRITER,
                path: fixtures::filled(string, b'p'),
                line: 1,
                body: fixtures::filled(body, b'b'),
            }]),
            more: false,
        },
        Listing::Branches => api::Answer::Branches(Box::new([fixtures::filled(string, b'p')])),
        Listing::Collaborators => api::Answer::Collaborators {
            collaborators: Box::new([api::Collaborator { user: fixtures::WRITER, permission: api::Permission::Write }]),
            more: false,
        },
        Listing::Compare => api::Answer::Compare {
            before: [0; 32],
            after: fixtures::BRANCH_HEAD,
            contains_before: true,
            commits: Box::new([fixtures::BRANCH_HEAD]),
            files: Box::new([]),
        },
        Listing::PullFiles => api::Answer::PullFiles {
            head: fixtures::BRANCH_HEAD,
            files: Box::new([api::File {
                path: fixtures::filled(string, b'p'),
                before: Some(fixtures::filled(body, b'b')),
                after: Some(fixtures::filled(body, b'a')),
            }]),
            more: false,
        },
    }
}

#[test]
fn connect_checks_name_the_refused_value() {
    use crate::{Access, Value};
    use skein_forge::credential::Credential;
    use skein_lib::Secret;
    let token = Credential::Token(Secret::new(Box::from(fixtures::TOKEN)));
    for (label, credential, user, listener, secret, expected) in [
        (
            "App",
            Credential::App { id: 1, key: Secret::new(Box::new([])) },
            fixtures::WRITER,
            true,
            None,
            Err(Value::Credential),
        ),
        ("empty token", Credential::Token(Secret::new(Box::new([]))), fixtures::WRITER, true, None, Err(Value::Token)),
        (
            "token bound",
            Credential::Token(Secret::new(fixtures::filled(fixtures::TOKEN_BOUND, b'a'))),
            fixtures::WRITER,
            true,
            None,
            Ok(()),
        ),
        (
            "token past bound",
            Credential::Token(Secret::new(fixtures::filled(fixtures::TOKEN_BOUND + 1, b'a'))),
            fixtures::WRITER,
            true,
            None,
            Err(Value::Token),
        ),
        (
            "token control",
            Credential::Token(Secret::new(Box::from(b"a\n".as_slice()))),
            fixtures::WRITER,
            true,
            None,
            Err(Value::Token),
        ),
        ("zero user", token.clone(), 0, true, None, Err(Value::User)),
        (
            "no listener",
            token.clone(),
            fixtures::WRITER,
            false,
            Some(Secret::new(Box::from(fixtures::HOOK_SECRET))),
            Err(Value::Secret),
        ),
        ("empty secret", token.clone(), fixtures::WRITER, true, Some(Secret::new(Box::new([]))), Err(Value::Secret)),
        (
            "secret bound",
            token.clone(),
            fixtures::WRITER,
            true,
            Some(Secret::new(fixtures::filled(fixtures::SECRET_BOUND, b'a'))),
            Ok(()),
        ),
        (
            "secret past bound",
            token,
            fixtures::WRITER,
            true,
            Some(Secret::new(fixtures::filled(fixtures::SECRET_BOUND + 1, b'a'))),
            Err(Value::Secret),
        ),
    ] {
        let access = Access { credential, user, webhook_secret: secret };
        assert_eq!(
            super::access(&fixtures::limits(), crate::settings::Kind::Forgejo, listener, &access),
            expected,
            "{label}"
        );
    }
}

#[test]
fn a_github_app_access_is_accepted() {
    let access = fixtures::app_access(skein_fake_forge_github::keys::KEYS.first().expect("test key"), 7, 9);
    assert_eq!(super::access(&fixtures::limits(), crate::settings::Kind::GitHub, false, &access), Ok(()));
}

#[test]
fn a_github_access_names_the_refused_value() {
    use crate::{Access, Value, settings::Kind};
    use skein_forge::credential::Credential;
    use skein_lib::Secret;
    let l = fixtures::limits();
    let key = *skein_fake_forge_github::keys::KEYS.first().expect("test key");
    let overlong =
        fixtures::filled(usize::try_from(l.key_bytes).expect("key bound").checked_add(1).expect("one past"), 0);
    let mut secret = fixtures::app_access(key, 7, 9);
    secret.webhook_secret = Some(Secret::new(Box::from(fixtures::HOOK_SECRET)));
    for (label, access, listener, expected) in [
        ("key past bound", fixtures::app_access(&overlong, 7, 9), false, Value::Key),
        ("key will not sign", fixtures::app_access(&[0; 64], 7, 9), false, Value::Key),
        (
            "token",
            Access {
                credential: Credential::Token(Secret::new(Box::from(fixtures::TOKEN))),
                user: 9,
                webhook_secret: None,
            },
            false,
            Value::Credential,
        ),
        ("zero user", fixtures::app_access(key, 7, 0), false, Value::User),
        ("secret with listener", secret, true, Value::Secret),
        ("key before user", fixtures::app_access(&[0; 64], 7, 0), false, Value::Key),
    ] {
        assert_eq!(super::access(&l, Kind::GitHub, listener, &access), Err(expected), "{label}");
    }
}

#[test]
fn an_app_credential_on_a_forgejo_forge_is_the_other_dialects() {
    for key in [*skein_fake_forge_github::keys::KEYS.first().expect("test key"), &[0; 64]] {
        let access = fixtures::app_access(key, 7, 9);
        assert_eq!(
            super::access(&fixtures::limits(), crate::settings::Kind::Forgejo, false, &access),
            Err(crate::Value::Credential)
        );
    }
}

#[test]
fn a_forges_key_and_jwt_are_in_the_worst_case() {
    let l = fixtures::limits();
    let shared = fixtures::shared();
    let forges = super::Forges { forgejo: 0, github: 2 };
    let larger = super::Limits { key_bytes: l.key_bytes.checked_add(100).expect("key bound"), ..l };
    let before = super::worst_case(&l, &shared, forges, 0).expect("fixture bound");
    let after = super::worst_case(&larger, &shared, forges, 0).expect("larger bound");
    assert_eq!(
        after.checked_sub(before).expect("larger footprint"),
        u64::from(forges.github).checked_mul(100).expect("key growth")
    );
}

#[test]
fn github_document_depth_needs_ten_only_for_github() {
    for (depth, expected) in
        [(9, Err(Unusable::GitHubDocuments(skein_forge::github::limits::Problem::Depth))), (10, Ok(()))]
    {
        let limits = super::Limits { depth, ..fixtures::limits() };
        assert_eq!(check(&limits, &fixtures::shared(), &fixtures::github_settings()), expected);
        assert_eq!(check(&limits, &fixtures::shared(), &fixtures::settings()), Ok(()));
    }
}

#[test]
fn installation_pages_use_their_own_full_page_budget() {
    let l = super::Limits { installations: 1, ..fixtures::limits() };
    let s = Shared { rows: 1, answer_bytes: fixtures::MAX_ROW, name_bytes: 4, ..fixtures::shared() };
    assert_eq!(
        super::installation_bounds(&l, &s),
        Some(skein_forge::github::plans::plan::Bounds {
            inner: l.inner,
            page_size: 100,
            rows: 100,
            answer_bytes: 2000,
            body_bytes: fixtures::BODY_ROOM,
        })
    );
    assert_eq!(check(&l, &s, &fixtures::github_settings()), Ok(()));
}
#[test]
fn installation_page_budget_refuses_checked_overflow() {
    const CAP: u32 = skein_forge::github::pin::PAGE_CAP;
    const OVERHEAD: u32 = 16; // measure::STRING_BYTES, the dialect's per-login overhead.
    const NAME_BOUND: u32 = u32::MAX / CAP - OVERHEAD;
    let l = fixtures::limits();
    let at = Shared { name_bytes: NAME_BOUND, ..fixtures::shared() };
    let past = Shared { name_bytes: NAME_BOUND + 1, ..at };
    assert_eq!(super::installation_bounds(&l, &at).expect("at derived bound").answer_bytes, 4_294_967_200);
    assert_eq!(super::installation_bounds(&l, &past), None);
    assert_eq!(super::installation_bounds(&l, &Shared { name_bytes: u32::MAX, ..at }), None);
    assert_eq!(check(&l, &past, &fixtures::github_settings()), Err(Unusable::Zero));
    assert_eq!(
        check(&l, &past, &fixtures::settings()),
        Err(Unusable::StringBytes { string_bytes: l.string_bytes, name_bytes: past.name_bytes })
    );
}
#[test]
fn installation_heap_covers_simultaneous_tables_and_page_payloads() {
    const ROWS: u32 = 3;
    const NAME: u32 = 128;
    const TOKEN: u32 = 64;
    const PEAK_PAYLOADS: u64 = 3 * (6 * 128 + 64) + 2 * 100 * 128 + 3 * 128 + 7 * 64;
    let l = super::Limits { installations: ROWS, token_bytes: TOKEN, ..fixtures::limits() };
    let s = Shared { name_bytes: NAME, ..fixtures::shared() };
    let empty = super::installation_heap(&super::Limits { token_bytes: 0, ..l }, &Shared { name_bytes: 0, ..s })
        .expect("scalar storage");
    let peak = super::installation_heap(&l, &s).expect("payload storage");
    // Old and staged metadata plus the live row, accepted/missing accounts,
    // a full page and its handoff, cursor/event accounts and mint handoffs coexist.
    assert_eq!(peak - empty, PEAK_PAYLOADS);
    assert!(
        peak >= PEAK_PAYLOADS
            + List::<skein_forge::github::document::Installation>::worst_case(100).expect("page storage") * 2
    );
}
#[test]
fn installation_staging_and_page_accounting_refuse_overflow() {
    let l = super::Limits { installations: u32::MAX, token_bytes: u32::MAX, ..fixtures::limits() };
    let s = Shared { name_bytes: u32::MAX, ..fixtures::shared() };
    assert_eq!(super::installation_heap(&l, &s), None);
    assert_eq!(super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, 0), None);
}

#[test]
fn ready_candidate_and_held_token_fit_at_simultaneous_peak() {
    const ROWS: u32 = 3;
    const TOKEN: u32 = 64;
    const NAME: u32 = 128;
    // Live Full tokens, decoder/authorization handoffs, retry candidate and held Contents.
    const TOKEN_COPIES: u64 = 3 + 7;
    // Six bounded table accounts per row, two page buffers and three event accounts.
    const NAME_COPIES: u64 = 3 * 6 + 2 * 100 + 3;
    let l = super::Limits { installations: ROWS, token_bytes: TOKEN, ..fixtures::limits() };
    let s = Shared { name_bytes: NAME, ..fixtures::shared() };
    let scalar = super::installation_heap(&super::Limits { token_bytes: 0, ..l }, &Shared { name_bytes: 0, ..s })
        .expect("scalar storage");
    let peak = super::installation_heap(&l, &s).expect("simultaneous payloads");
    assert_eq!(peak - scalar, TOKEN_COPIES * 64 + NAME_COPIES * 128);
    assert!(
        scalar >= skein_lib::Map::<u64, crate::installations::Row>::worst_case(ROWS).expect("states and revisions")
    );
}

#[test]
fn renewal_clocks_and_handoff_fit_at_peak() {
    use crate::installations::{Clock, Row};
    const ROWS: u32 = 3;
    // One retained Full per row and seven handoff/candidate/held copies per forge.
    const COPIES: u64 = 10;
    let l = super::Limits { installations: ROWS, token_bytes: 0, ..fixtures::limits() };
    let s = fixtures::shared();
    let scalar = super::installation_heap(&l, &s).expect("scalar storage");
    let payload = super::installation_heap(&super::Limits { token_bytes: 64, ..l }, &s).expect("handoff storage");
    assert_eq!(payload - scalar, COPIES * 64);
    assert!(size_of::<Row>() >= size_of::<Clock>() * 2 + 16);
    assert!(scalar >= skein_lib::Map::<u64, Row>::worst_case(ROWS).expect("row clocks and revisions"));
}
#[test]
fn renewal_handoff_checked_add_refuses_overflow() {
    let l = fixtures::limits();
    let s = fixtures::shared();
    let base = super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, 0).expect("bounded renewal storage");
    assert_eq!(super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, u64::MAX - base), Some(u64::MAX));
    assert_eq!(super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, u64::MAX - base + 1), None);
}

#[test]
fn github_tables_and_discovery_plans_are_priced_at_simultaneous_peak() {
    let l = fixtures::limits();
    let s = fixtures::shared();
    let fixed = super::github_tables_heap(&l, l.repositories, l.users).expect("full tables");
    let staged = super::github_tables_heap(&l, 1, 1).expect("discovery staging");
    assert!(
        fixed
            >= u64::from(l.repositories)
                * u64::try_from(size_of::<skein_forge::github::plans::tables::Repository>()).expect("row size")
    );
    let base = super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, 0).expect("complete bound");
    assert!(base >= fixed + u64::from(s.calls) * (staged + u64::from(s.answer_bytes) * 4));
    let more = super::Limits { repositories: l.repositories + 1, users: l.users + 1, ..l };
    assert!(super::worst_case(&more, &s, super::Forges { forgejo: 0, github: 1 }, 0).expect("larger tables") > base);
    assert_eq!(super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, u64::MAX - base + 1), None);
}

#[test]
fn discovery_candidates_fit_the_installation_bound_for_every_call() {
    use crate::installations::Selected;
    const ROWS: u32 = 3;
    const CALLS: u32 = 2;
    let l = super::Limits { installations: ROWS, ..fixtures::limits() };
    let s = Shared { calls: CALLS, ..fixtures::shared() };
    let empty = super::Limits { installations: 0, ..l };
    let before = super::github_call_heap(&empty, &s).expect("no candidate storage");
    let after = super::github_call_heap(&l, &s).expect("candidate storage for a call");
    assert_eq!(after - before, List::<Selected>::worst_case(ROWS).expect("candidate list"));
    let total = super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, 0).expect("all calls");
    assert!(total >= u64::from(CALLS) * after);
}
#[test]
fn discovery_handoff_prices_two_live_read_plans_and_the_retained_operation() {
    let l = fixtures::limits();
    let s = fixtures::shared();
    let reads = u64::try_from(size_of::<skein_forge::github::plans::reads::Plan>()).expect("read size");
    let staged = super::github_tables_heap(&l, 1, 1).expect("staged row");
    let candidates = List::<crate::installations::Selected>::worst_case(l.installations).expect("candidate storage");
    let heap = super::github_call_heap(&l, &s).expect("discovery and handoff");
    assert!(heap >= reads * 2 + staged + candidates + u64::from(s.answer_bytes) * 3);
    let base = super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, 0).expect("all calls");
    assert!(base >= heap * u64::from(s.calls) + u64::from(s.op_bytes) * 3 * u64::from(s.calls));
}
#[test]
fn discovery_candidate_storage_refuses_checked_overflow() {
    let l = super::Limits { installations: u32::MAX, ..fixtures::limits() };
    let s = Shared { calls: u32::MAX, ..fixtures::shared() };
    assert_eq!(super::worst_case(&l, &s, super::Forges { forgejo: 0, github: 1 }, 0), None);
}
