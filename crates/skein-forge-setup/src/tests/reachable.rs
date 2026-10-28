use super::fixtures::{self, ADMIN, reached, search, secret, user_repo, user_repos};
use crate::call::Auth;
use crate::{
    given::{Place, Refused},
    item::Item,
    reachable::{self, Reachable, Reached},
    reconcile::{Event, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::forgejo::{
    document::{Document, Hook, Row},
    request::{Page, Request},
};
use skein_lib::Queue;

const READING: usize = 1;
const ADMIN_CHECK: usize = 2;
const SETTINGS: usize = 3;
const SEARCH: usize = 4;
fn step(machine: &mut Reachable, event: Event) -> Need {
    let mut out = Queue::uncapped();
    reachable::step(machine, event, &mut out);
    assert_eq!(out.len(), 1);
    out.pop().expect("one need")
}
fn at(phase: usize) -> Reachable {
    let mut machine = fixtures::reachable(1);
    for event in [
        Event::Begin,
        Event::Read(Some(secret(ADMIN))),
        fixtures::user_event(fixtures::admin()),
        fixtures::settings_event(50),
    ]
    .into_iter()
    .take(phase)
    {
        step(&mut machine, event);
    }
    machine
}
fn send(request: Request) -> Need {
    Need::Send { request: request.into(), auth: Auth::Token(secret(ADMIN)) }
}
fn page(page: u32, limit: u32) -> Need {
    send(Request::RepoSearch { page: Page { page: Some(page), limit: Some(limit) } })
}
fn failure(stage: Stage, why: Why) -> Need {
    fixtures::named_failure(stage, b"kit-admin", why)
}
fn rows(count: u32) -> Box<[Row]> {
    let mut rows = skein_lib::List::with_capacity(count);
    for id in 1..=count {
        rows.push(reached(u64::from(id), 7, b"ada", b"work", false)).expect("fixture rows");
    }
    rows.into_boxed()
}
fn outcome(count: u32, more: bool) -> Need {
    let mut rows = skein_lib::List::with_capacity(count);
    for id in 1..=count {
        rows.push(Reached {
            id: u64::from(id),
            owner_id: 7,
            owner: b"ada".as_slice().into(),
            name: b"work".as_slice().into(),
            private: false,
        })
        .expect("fixture rows");
    }
    Need::Done(Outcome::Reachable { rows: rows.into_boxed(), more })
}
#[test]
fn reachable_checks_the_admins_token_then_reads_the_cap_then_one_page() {
    let mut machine = fixtures::reachable(1);
    assert_eq!(step(&mut machine, Event::Begin), Need::Read(Item::AdminToken));
    assert_eq!(step(&mut machine, Event::Read(Some(secret(ADMIN)))), send(Request::User));
    assert_eq!(step(&mut machine, fixtures::user_event(fixtures::admin())), send(Request::ApiSettings));
    assert_eq!(step(&mut machine, fixtures::settings_event(50)), page(1, 4));
}
#[test]
fn the_page_size_is_the_least_of_the_limit_and_the_cap() {
    for (cap, limit) in [(1, 1), (3, 3), (4, 4), (50, 4)] {
        assert_eq!(step(&mut at(SETTINGS), fixtures::settings_event(cap)), page(1, limit));
    }
}
#[test]
fn the_page_asked_for_is_the_page_sent() {
    for number in [3, u32::MAX] {
        let mut machine = fixtures::reachable(number);
        step(&mut machine, Event::Begin);
        step(&mut machine, Event::Read(Some(secret(ADMIN))));
        step(&mut machine, fixtures::user_event(fixtures::admin()));
        assert_eq!(step(&mut machine, fixtures::settings_event(50)), page(number, 4));
    }
}
#[test]
fn a_full_page_says_there_is_more() {
    assert_eq!(step(&mut at(SEARCH), search(true, rows(4))), outcome(4, true));
    let mut capped = at(SETTINGS);
    step(&mut capped, fixtures::settings_event(3));
    assert_eq!(step(&mut capped, search(true, rows(3))), outcome(3, true));
}
#[test]
fn a_short_last_page_says_there_is_no_more() {
    assert_eq!(step(&mut at(SEARCH), search(true, rows(3))), outcome(3, false));
}
#[test]
fn an_empty_page_says_there_is_no_more_and_holds_no_row() {
    assert_eq!(step(&mut at(SEARCH), search(true, rows(0))), outcome(0, false));
}
#[test]
fn each_row_carries_its_id_owner_name_and_privacy() {
    assert_eq!(
        step(
            &mut at(SEARCH),
            search(
                true,
                Box::new([reached(9, 13, b"grace", b"private", true), reached(2, 7, b"ada", b"public", false),])
            )
        ),
        Need::Done(Outcome::Reachable {
            rows: Box::new([
                Reached {
                    id: 9,
                    owner_id: 13,
                    owner: b"grace".as_slice().into(),
                    name: b"private".as_slice().into(),
                    private: true
                },
                Reached {
                    id: 2,
                    owner_id: 7,
                    owner: b"ada".as_slice().into(),
                    name: b"public".as_slice().into(),
                    private: false
                },
            ]),
            more: false
        })
    );
}
#[test]
fn a_non_admin_is_refused_naming_the_admin() {
    assert_eq!(
        step(&mut at(ADMIN_CHECK), fixtures::user_event(fixtures::person(b"person", b"Person"))),
        failure(Stage::Admin, Why::NotAdmin)
    );
}
#[test]
fn neither_token_stops_missing_naming_api() {
    for admin in [None, Some(secret(b""))] {
        for api in [None, Some(secret(b""))] {
            let mut machine = at(READING);
            step(&mut machine, Event::Read(admin.clone()));
            assert_eq!(step(&mut machine, Event::Read(api)), connected_failure(Why::Missing));
        }
    }
}
#[test]
fn an_installation_given_to_a_forgejo_still_stops_before_either_token_is_read() {
    for (id, subject) in [(7, b"7".as_slice()), (0, b"0"), (u64::MAX, b"18446744073709551615")] {
        let mut machine = Reachable::new(&fixtures::given(), Some(id), 1).expect("given admitted");
        assert_eq!(
            step(&mut machine, Event::Begin),
            fixtures::named_failure(Stage::Installation, subject, Why::Unexpected)
        );
        let mut out = Queue::uncapped();
        reachable::step(&mut machine, Event::Read(Some(secret(ADMIN))), &mut out);
        assert!(out.is_empty());
    }
}
#[test]
fn an_answer_whose_ok_is_not_true_is_unexpected() {
    assert_eq!(step(&mut at(SEARCH), search(false, rows(1))), failure(Stage::Token, Why::Unexpected));
}
#[test]
fn a_row_that_is_not_a_found_row_is_unexpected() {
    let hook = Row::Hook(Hook { id: 1, url: Box::new([]), events: Box::new([]), active: true });
    assert_eq!(
        step(&mut at(SEARCH), search(true, Box::new([reached(1, 7, b"ada", b"work", false), hook]))),
        failure(Stage::Token, Why::Unexpected)
    );
}
#[test]
fn a_zero_page_cap_is_unexpected() {
    assert_eq!(step(&mut at(SETTINGS), fixtures::settings_event(0)), failure(Stage::Token, Why::Unexpected));
}
#[test]
fn a_page_of_more_rows_than_asked_is_unexpected() {
    assert_eq!(step(&mut at(SEARCH), search(true, rows(5))), failure(Stage::Token, Why::Unexpected));
    let mut capped = at(SETTINGS);
    step(&mut capped, fixtures::settings_event(3));
    assert_eq!(step(&mut capped, search(true, rows(4))), failure(Stage::Token, Why::Unexpected));
}
#[test]
fn a_refused_request_names_the_admin_at_each_of_its_three_requests() {
    for (phase, stage) in [(ADMIN_CHECK, Stage::Admin), (SETTINGS, Stage::Token), (SEARCH, Stage::Token)] {
        assert_eq!(step(&mut at(phase), Event::Reply(Reply::Status(403))), failure(stage, Why::Refused(403)));
    }
}
#[test]
fn a_forge_that_stops_answering_stops_the_plan_at_each_request() {
    for (phase, stage) in [(ADMIN_CHECK, Stage::Admin), (SETTINGS, Stage::Token), (SEARCH, Stage::Token)] {
        assert_eq!(step(&mut at(phase), Event::Reply(Reply::Down)), failure(stage, Why::Down));
    }
}
#[test]
fn a_reply_of_another_kind_is_unexpected_at_each_request() {
    for (phase, stage) in [(ADMIN_CHECK, Stage::Admin), (SETTINGS, Stage::Token), (SEARCH, Stage::Token)] {
        for answer in [
            Reply::Empty,
            Reply::Document(Document::RepoSearch { ok: true }),
            Reply::Document(Document::RepoSearch { ok: false }),
        ] {
            assert_eq!(step(&mut at(phase), Event::Reply(answer)), failure(stage, Why::Unexpected));
        }
    }
    for (phase, event, stage) in [
        (ADMIN_CHECK, fixtures::settings_event(50), Stage::Admin),
        (ADMIN_CHECK, search(true, rows(0)), Stage::Admin),
        (SETTINGS, fixtures::user_event(fixtures::admin()), Stage::Token),
        (SETTINGS, search(true, rows(0)), Stage::Token),
        (SEARCH, fixtures::settings_event(50), Stage::Token),
        (SEARCH, fixtures::user_event(fixtures::admin()), Stage::Token),
    ] {
        assert_eq!(step(&mut at(phase), event), failure(stage, Why::Unexpected));
    }
}
/// Begin, the owner read, and the three requests, in transition order.
pub(super) fn phase_cases() -> [(Reachable, Stage, &'static [u8]); 5] {
    [
        (at(0), Stage::Admin, b"kit-admin"),
        (at(READING), Stage::Admin, b"kit-admin"),
        (at(ADMIN_CHECK), Stage::Admin, b"kit-admin"),
        (at(SETTINGS), Stage::Token, b"kit-admin"),
        (at(SEARCH), Stage::Token, b"kit-admin"),
    ]
}
#[test]
fn an_out_of_order_event_stops_each_phase() {
    for (phase, (_, stage, subject)) in phase_cases().into_iter().enumerate() {
        for event in [Event::Begin, Event::Read(None), Event::Written, fixtures::drawn(), Event::Reply(Reply::Empty)] {
            if (phase == 0 && event == Event::Begin)
                || (phase == READING && event == Event::Read(None))
                || (phase >= ADMIN_CHECK && event == Event::Reply(Reply::Empty))
            {
                continue;
            }
            assert_eq!(
                step(&mut at(phase), event),
                fixtures::named_failure(stage, subject, Why::Unexpected),
                "phase {phase}"
            );
        }
    }
}
#[test]
fn an_ended_reachable_ignores_every_event() {
    let mut done = at(SEARCH);
    step(&mut done, search(true, rows(0)));
    let mut stopped = at(ADMIN_CHECK);
    step(&mut stopped, Event::Reply(Reply::Down));
    for mut machine in [done, stopped] {
        for event in [
            Event::Begin,
            Event::Read(None),
            Event::Read(Some(secret(ADMIN))),
            Event::Reply(Reply::Down),
            Event::Written,
            fixtures::drawn(),
        ] {
            let mut out = Queue::uncapped();
            reachable::step(&mut machine, event, &mut out);
            assert!(out.is_empty());
        }
    }
}
#[test]
fn reachable_refuses_page_zero_naming_its_place() {
    assert_eq!(
        Reachable::new(&fixtures::given(), None, 0).expect_err("zero page"),
        Refused { place: Place::Page, other: None }
    );
}
#[test]
fn reachable_refuses_a_given_that_is_refused() {
    let mut given = fixtures::given();
    given.admin_name = Box::new([]);
    for page in [0, 1] {
        assert_eq!(
            Reachable::new(&given, None, page).expect_err("empty admin name"),
            Refused { place: Place::AdminName, other: None }
        );
    }
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the redaction test inspects failure and machine Debug renderings")]
fn no_token_is_in_a_reachables_failure_debug() {
    for phase in 0..=SEARCH {
        let mut machine = at(phase);
        let event = if phase == 0 { Event::Written } else { Event::Begin };
        let failure = step(&mut machine, event);
        assert!(skein_lib::bytes::find(format!("{failure:?}").as_bytes(), ADMIN).is_none());
        let debug = format!("{machine:?}");
        assert!(skein_lib::bytes::find(debug.as_bytes(), ADMIN).is_none());
        assert!(debug.contains("token: None"));
    }
    let mut machine = at(SEARCH);
    step(&mut machine, search(true, rows(0)));
    assert!(format!("{machine:?}").contains("token: None"));
}

const API: &[u8] = b"picker-api-token";
const API_READ: usize = 2;
const CONNECTED_SETTINGS: usize = 3;
const CONNECTED_LISTING: usize = 4;
fn connected_at_page(phase: usize, page: u32) -> Reachable {
    let mut machine = fixtures::reachable(page);
    for event in [Event::Begin, Event::Read(None), Event::Read(Some(secret(API))), fixtures::settings_event(50)]
        .into_iter()
        .take(phase)
    {
        step(&mut machine, event);
    }
    machine
}
fn connected_at(phase: usize) -> Reachable {
    connected_at_page(phase, 1)
}
fn connected_send(request: Request) -> Need {
    Need::Send { request: request.into(), auth: Auth::Token(secret(API)) }
}
fn connected_page(page: u32, limit: u32) -> Need {
    connected_send(Request::UserRepos { page: Page { page: Some(page), limit: Some(limit) } })
}
fn connected_failure(why: Why) -> Need {
    fixtures::named_failure(Stage::Token, b"api", why)
}
fn user_rows(count: u32, push: bool) -> Box<[Row]> {
    let mut rows = skein_lib::List::with_capacity(count);
    for id in 1..=count {
        rows.push(user_repo(u64::from(id), 7, b"ada", b"work", false, push)).expect("fixture rows");
    }
    rows.into_boxed()
}
fn kept(ids: &[u64], more: bool) -> Need {
    let mut rows = skein_lib::List::with_capacity(u32::try_from(ids.len()).expect("fixture count"));
    for id in ids {
        rows.push(Reached {
            id: *id,
            owner_id: 7,
            owner: b"ada".as_slice().into(),
            name: b"work".as_slice().into(),
            private: false,
        })
        .expect("fixture rows");
    }
    Need::Done(Outcome::Reachable { rows: rows.into_boxed(), more })
}
#[test]
fn with_no_admin_token_the_api_token_reads_the_cap_then_one_page() {
    let mut machine = fixtures::reachable(1);
    assert_eq!(step(&mut machine, Event::Begin), Need::Read(Item::AdminToken));
    assert_eq!(step(&mut machine, Event::Read(None)), Need::Read(Item::ApiToken));
    assert_eq!(step(&mut machine, Event::Read(Some(secret(API)))), connected_send(Request::ApiSettings));
    assert_eq!(step(&mut machine, fixtures::settings_event(50)), connected_page(1, 4));
}
#[test]
fn the_connected_page_size_is_the_least_of_the_limit_and_the_cap() {
    for (cap, limit) in [(1, 1), (3, 3), (4, 4), (50, 4)] {
        assert_eq!(
            step(&mut connected_at(CONNECTED_SETTINGS), fixtures::settings_event(cap)),
            connected_page(1, limit)
        );
    }
}
#[test]
fn the_connected_page_asked_for_is_the_page_sent() {
    for page in [3, u32::MAX] {
        assert_eq!(
            step(&mut connected_at_page(CONNECTED_SETTINGS, page), fixtures::settings_event(50)),
            connected_page(page, 4)
        );
    }
}
#[test]
fn a_kept_admin_token_is_used_and_the_api_token_is_never_read() {
    let mut machine = at(READING);
    assert_eq!(step(&mut machine, Event::Read(Some(secret(ADMIN)))), send(Request::User));
    assert_eq!(step(&mut machine, fixtures::user_event(fixtures::admin())), send(Request::ApiSettings));
    assert_eq!(step(&mut machine, fixtures::settings_event(50)), page(1, 4));
    assert_eq!(step(&mut machine, search(true, rows(1))), outcome(1, false));
}
#[test]
fn a_row_without_push_is_dropped_and_the_rest_keep_their_order() {
    assert_eq!(
        step(
            &mut connected_at(CONNECTED_LISTING),
            user_repos(Box::new([
                user_repo(9, 7, b"ada", b"work", false, true),
                user_repo(3, 7, b"ada", b"work", false, false),
                user_repo(2, 7, b"ada", b"work", false, true),
            ]))
        ),
        kept(&[9, 2], false)
    );
}
#[test]
fn more_is_read_before_the_rows_are_dropped() {
    assert_eq!(step(&mut connected_at(CONNECTED_LISTING), user_repos(user_rows(4, false))), kept(&[], true));
}
#[test]
fn a_short_page_has_no_more_though_rows_are_dropped() {
    assert_eq!(
        step(
            &mut connected_at(CONNECTED_LISTING),
            user_repos(Box::new([
                user_repo(1, 7, b"ada", b"work", false, true),
                user_repo(2, 7, b"ada", b"work", false, false),
                user_repo(3, 7, b"ada", b"work", false, true),
            ]))
        ),
        kept(&[1, 3], false)
    );
}
#[test]
fn each_connected_row_carries_its_id_owner_name_and_privacy() {
    assert_eq!(
        step(
            &mut connected_at(CONNECTED_LISTING),
            user_repos(Box::new([
                user_repo(9, 13, b"grace", b"private", true, true),
                user_repo(2, 7, b"ada", b"public", false, true),
            ]))
        ),
        Need::Done(Outcome::Reachable {
            rows: Box::new([
                Reached {
                    id: 9,
                    owner_id: 13,
                    owner: b"grace".as_slice().into(),
                    name: b"private".as_slice().into(),
                    private: true
                },
                Reached {
                    id: 2,
                    owner_id: 7,
                    owner: b"ada".as_slice().into(),
                    name: b"public".as_slice().into(),
                    private: false
                },
            ]),
            more: false
        })
    );
}
#[test]
fn a_connected_listing_of_more_rows_than_asked_is_unexpected() {
    assert_eq!(
        step(&mut connected_at(CONNECTED_LISTING), user_repos(user_rows(5, false))),
        connected_failure(Why::Unexpected)
    );
    let mut capped = connected_at(CONNECTED_SETTINGS);
    step(&mut capped, fixtures::settings_event(3));
    assert_eq!(step(&mut capped, user_repos(user_rows(4, false))), connected_failure(Why::Unexpected));
}
#[test]
fn a_zero_page_cap_is_unexpected_naming_api() {
    assert_eq!(
        step(&mut connected_at(CONNECTED_SETTINGS), fixtures::settings_event(0)),
        connected_failure(Why::Unexpected)
    );
}
#[test]
fn a_refused_request_names_api_at_both_of_its_requests() {
    for phase in [CONNECTED_SETTINGS, CONNECTED_LISTING] {
        assert_eq!(
            step(&mut connected_at(phase), Event::Reply(Reply::Status(401))),
            connected_failure(Why::Refused(401))
        );
    }
}
#[test]
fn a_forge_that_stops_answering_stops_the_connected_plan_at_both() {
    for phase in [CONNECTED_SETTINGS, CONNECTED_LISTING] {
        assert_eq!(step(&mut connected_at(phase), Event::Reply(Reply::Down)), connected_failure(Why::Down));
    }
}
#[test]
fn a_reply_of_another_kind_is_unexpected_at_both() {
    for phase in [CONNECTED_SETTINGS, CONNECTED_LISTING] {
        for event in [
            Event::Reply(Reply::Empty),
            fixtures::user_event(fixtures::admin()),
            search(true, rows(1)),
            Event::Reply(Reply::Document(Document::RepoSearch { ok: false })),
        ] {
            assert_eq!(step(&mut connected_at(phase), event), connected_failure(Why::Unexpected));
        }
    }
    assert_eq!(
        step(&mut connected_at(CONNECTED_SETTINGS), user_repos(user_rows(0, true))),
        connected_failure(Why::Unexpected)
    );
    assert_eq!(
        step(&mut connected_at(CONNECTED_LISTING), fixtures::settings_event(50)),
        connected_failure(Why::Unexpected)
    );
}
#[test]
fn an_out_of_order_event_stops_each_connected_phase() {
    for phase in [API_READ, CONNECTED_SETTINGS, CONNECTED_LISTING] {
        for event in [Event::Begin, Event::Written, fixtures::drawn()] {
            assert_eq!(step(&mut connected_at(phase), event), connected_failure(Why::Unexpected));
        }
    }
    assert_eq!(step(&mut connected_at(API_READ), Event::Reply(Reply::Empty)), connected_failure(Why::Unexpected));
    for phase in [CONNECTED_SETTINGS, CONNECTED_LISTING] {
        assert_eq!(step(&mut connected_at(phase), Event::Read(None)), connected_failure(Why::Unexpected));
    }
}
#[test]
fn an_ended_connected_reachable_ignores_every_event() {
    let mut done = connected_at(CONNECTED_LISTING);
    step(&mut done, user_repos(user_rows(0, false)));
    let mut stopped = connected_at(CONNECTED_SETTINGS);
    step(&mut stopped, Event::Reply(Reply::Down));
    for mut machine in [done, stopped] {
        for event in [
            Event::Begin,
            Event::Read(None),
            Event::Read(Some(secret(API))),
            Event::Reply(Reply::Down),
            Event::Written,
            fixtures::drawn(),
        ] {
            let mut out = Queue::uncapped();
            reachable::step(&mut machine, event, &mut out);
            assert!(out.is_empty());
        }
    }
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the redaction test inspects failure and machine Debug renderings")]
fn no_token_is_in_a_connected_failure_debug() {
    for phase in API_READ..=CONNECTED_LISTING {
        let mut machine = connected_at(phase);
        assert!(skein_lib::bytes::find(format!("{machine:?}").as_bytes(), API).is_none());
        let failure = step(&mut machine, Event::Begin);
        assert!(skein_lib::bytes::find(format!("{failure:?}").as_bytes(), API).is_none());
        assert!(format!("{machine:?}").contains("token: None"));
    }
    let mut machine = connected_at(CONNECTED_LISTING);
    step(&mut machine, user_repos(user_rows(0, true)));
    assert!(format!("{machine:?}").contains("token: None"));
}
