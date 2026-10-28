use super::fixtures::{self, TAKEN, TAKEN_SUFFIX, TakeOverRun, secret, take_over_admin, take_over_row};
use crate::call::Auth;
use crate::{
    item::Item,
    reconcile::{Event, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::forgejo::{document::Row, request::Request};

// Number of accepted events before each outstanding need.
const CHECKING: usize = 1;
const WRITTEN: usize = 2;
const SETTINGS: usize = 3;
const LISTING: usize = 4;
const DELETING: usize = 5;
fn at(phase: usize) -> TakeOverRun {
    let mut run = TakeOverRun::new();
    for event in [
        Event::Begin,
        take_over_admin(),
        Event::Written,
        fixtures::settings_event(fixtures::PAGE_CAP),
        fixtures::rows(Box::new([take_over_row(7, b"oldtoken")])),
    ]
    .into_iter()
    .take(phase)
    {
        run.step(event);
    }
    run
}
fn send(request: Request) -> Need {
    Need::Send { request: request.into(), auth: Auth::Token(secret(TAKEN)) }
}
fn listing(page: u32) -> Need {
    send(Request::Tokens { login: b"root".as_slice().into(), page: fixtures::page(page) })
}
fn failure(stage: Stage, why: Why) -> Need {
    fixtures::named_failure(stage, b"kit-admin", why)
}
fn done() -> Need {
    Need::Done(Outcome::Nothing)
}

#[test]
fn the_taken_token_checks_as_the_admin_and_is_written_then_listed() {
    let mut run = TakeOverRun::new();
    assert_eq!(run.step(Event::Begin), send(Request::User));
    assert_eq!(run.step(take_over_admin()), Need::Write { item: Item::AdminToken, bytes: secret(TAKEN) });
    assert_eq!(run.step(Event::Written), send(Request::ApiSettings));
    assert_eq!(run.step(fixtures::settings_event(fixtures::PAGE_CAP)), listing(1));
}

#[test]
fn an_empty_token_stops_before_any_request() {
    assert_eq!(TakeOverRun::with_token(b"").step(Event::Begin), failure(Stage::Admin, Why::Missing));
}

#[test]
fn a_token_shorter_than_eight_bytes_is_checked_like_any_other() {
    let mut run = TakeOverRun::with_token(b"abc");
    assert_eq!(run.step(Event::Begin), Need::Send { request: Request::User.into(), auth: Auth::Token(secret(b"abc")) });
    run.step(take_over_admin());
    run.step(Event::Written);
    run.step(fixtures::settings_event(fixtures::PAGE_CAP));
    assert_eq!(run.step(fixtures::rows(Box::new([take_over_row(1, b"abc")]))), done());
}

#[test]
fn a_token_of_a_plain_user_is_not_admin() {
    assert_eq!(
        at(CHECKING).step(fixtures::user_event(fixtures::user(9, b"person", false))),
        failure(Stage::Admin, Why::NotAdmin)
    );
}

#[test]
fn a_refused_check_names_the_admin() {
    for (reply, why) in [(Reply::Status(401), Why::Refused(401)), (Reply::Down, Why::Down)] {
        assert_eq!(at(CHECKING).step(Event::Reply(reply)), failure(Stage::Admin, why));
    }
}

#[test]
fn a_zero_page_cap_is_unexpected() {
    assert_eq!(at(SETTINGS).step(fixtures::settings_event(0)), failure(Stage::Token, Why::Unexpected));
}

#[test]
fn a_token_that_is_not_the_taken_one_is_deleted_by_id() {
    assert_eq!(
        at(LISTING).step(fixtures::rows(Box::new([take_over_row(7, b"oldtoken"), take_over_row(8, TAKEN_SUFFIX)]))),
        send(Request::DeleteToken { login: b"root".as_slice().into(), id: 7 })
    );
}

#[test]
fn the_taken_tokens_own_row_is_never_deleted() {
    assert_eq!(at(LISTING).step(fixtures::rows(Box::new([take_over_row(8, TAKEN_SUFFIX)]))), done());
}

#[test]
fn a_deleted_token_lists_the_same_page_again() {
    assert_eq!(at(DELETING).step(Event::Reply(Reply::Empty)), listing(1));
}

#[test]
fn a_delete_answered_not_found_counts_as_deleted() {
    assert_eq!(at(DELETING).step(Event::Reply(Reply::Status(404))), listing(1));
}

#[test]
fn a_full_page_with_nothing_to_delete_reads_the_next_page() {
    let mut run = at(LISTING);
    assert_eq!(
        run.step(fixtures::rows(Box::new([take_over_row(8, TAKEN_SUFFIX), take_over_row(9, TAKEN_SUFFIX)]))),
        listing(2)
    );
    assert_eq!(
        run.step(fixtures::rows(Box::new([take_over_row(10, b"oldtoken")]))),
        send(Request::DeleteToken { login: b"root".as_slice().into(), id: 10 })
    );
    assert_eq!(run.step(Event::Reply(Reply::Empty)), listing(2));
}

#[test]
fn a_short_page_with_nothing_to_delete_ends_done() {
    assert_eq!(at(LISTING).step(fixtures::rows(Box::new([]))), done());
}

#[test]
fn a_row_that_survives_its_delete_stops_the_plan_unexpected() {
    for answer in [Reply::Empty, Reply::Status(404)] {
        let mut run = at(DELETING);
        run.step(Event::Reply(answer));
        assert_eq!(
            run.step(fixtures::rows(Box::new([take_over_row(8, TAKEN_SUFFIX), take_over_row(7, b"oldtoken")]))),
            failure(Stage::Token, Why::Unexpected)
        );
    }
}

#[test]
fn a_refused_listing_or_delete_stops_at_the_token_step_naming_the_admin() {
    for (phase, reply, why) in [
        (LISTING, Reply::Status(500), Why::Refused(500)),
        (DELETING, Reply::Status(403), Why::Refused(403)),
        (LISTING, Reply::Down, Why::Down),
        (DELETING, Reply::Down, Why::Down),
        (SETTINGS, Reply::Status(503), Why::Refused(503)),
        (SETTINGS, Reply::Down, Why::Down),
    ] {
        assert_eq!(at(phase).step(Event::Reply(reply)), failure(Stage::Token, why));
    }
}

pub(super) fn phase_cases() -> [(&'static str, TakeOverRun, Event, Stage, &'static [u8]); 6] {
    [
        ("begin", 0, Event::Written, Stage::Admin),
        ("checking", CHECKING, Event::Begin, Stage::Admin),
        ("written", WRITTEN, Event::Begin, Stage::Admin),
        ("settings", SETTINGS, Event::Begin, Stage::Token),
        ("listing", LISTING, Event::Begin, Stage::Token),
        ("deleting", DELETING, Event::Begin, Stage::Token),
    ]
    .map(|(name, phase, wrong, stage)| (name, at(phase), wrong, stage, b"kit-admin".as_slice()))
}
#[test]
fn an_out_of_order_event_stops_each_take_over_phase() {
    for (name, mut run, wrong, stage, _) in phase_cases() {
        assert_eq!(run.step(wrong), failure(stage, Why::Unexpected), "{name}");
    }
}

pub(super) fn terminal_runs() -> [TakeOverRun; 2] {
    let mut ended = at(LISTING);
    assert_eq!(ended.step(fixtures::rows(Box::new([]))), done());
    let mut stopped = TakeOverRun::with_token(b"");
    assert_eq!(stopped.step(Event::Begin), failure(Stage::Admin, Why::Missing));
    [ended, stopped]
}
#[test]
fn an_ended_take_over_ignores_every_event() {
    for mut run in terminal_runs() {
        for event in [Event::Begin, Event::Read(None), Event::Reply(Reply::Down), Event::Written, fixtures::drawn()] {
            let mut out = skein_lib::Queue::uncapped();
            crate::take_over::step(&mut run.machine, event, &mut out);
            assert!(out.is_empty());
        }
    }
}

#[test]
#[expect(clippy::disallowed_macros, reason = "the redaction test inspects the takeover's Debug rendering")]
fn the_taken_token_is_dropped_at_every_end() {
    let live = at(CHECKING);
    assert!(format!("{:?}", live.machine).contains("taken: Some(Secret { len: 31 })"));
    for (mut run, event, expected) in [
        (at(LISTING), fixtures::rows(Box::new([])), done()),
        (TakeOverRun::with_token(b""), Event::Begin, failure(Stage::Admin, Why::Missing)),
        (at(CHECKING), fixtures::user_event(fixtures::user(9, b"person", false)), failure(Stage::Admin, Why::NotAdmin)),
        (at(CHECKING), Event::Reply(Reply::Status(401)), failure(Stage::Admin, Why::Refused(401))),
        (at(SETTINGS), fixtures::settings_event(0), failure(Stage::Token, Why::Unexpected)),
        (at(LISTING), Event::Reply(Reply::Status(500)), failure(Stage::Token, Why::Refused(500))),
        (at(DELETING), Event::Reply(Reply::Down), failure(Stage::Token, Why::Down)),
    ] {
        assert_eq!(run.step(event), expected);
        assert!(format!("{:?}", run.machine).contains("taken: None"));
    }
    let mut survivor = at(DELETING);
    survivor.step(Event::Reply(Reply::Empty));
    assert_eq!(
        survivor.step(fixtures::rows(Box::new([take_over_row(7, b"oldtoken")]))),
        failure(Stage::Token, Why::Unexpected)
    );
    assert!(format!("{:?}", survivor.machine).contains("taken: None"));
    for phase in 0..=DELETING {
        let mut run = at(phase);
        let event = if phase == 0 { Event::Written } else { Event::Begin };
        run.step(event);
        assert!(format!("{:?}", run.machine).contains("taken: None"));
    }
}

#[test]
fn a_reply_of_the_wrong_shape_is_unexpected_at_each_take_over_phase() {
    for (phase, event, stage) in [
        (CHECKING, Event::Reply(Reply::Empty), Stage::Admin),
        (CHECKING, fixtures::settings_event(2), Stage::Admin),
        (SETTINGS, fixtures::rows(Box::new([])), Stage::Token),
        (LISTING, fixtures::settings_event(2), Stage::Token),
        (LISTING, fixtures::rows(Box::new([Row::User(fixtures::admin())])), Stage::Token),
        (DELETING, fixtures::rows(Box::new([])), Stage::Token),
    ] {
        assert_eq!(at(phase).step(event), failure(stage, Why::Unexpected));
    }
}

#[test]
fn invalid_given_values_refuse_a_take_over_before_it_starts() {
    let mut given = fixtures::given();
    given.admin_name = Box::new([]);
    assert_eq!(
        crate::take_over::TakeOver::new(&given, secret(TAKEN)).expect_err("empty admin name"),
        crate::given::Refused { place: crate::given::Place::AdminName, other: None }
    );
}

#[test]
fn a_learned_cap_above_inner_pages_takeover_at_inner() {
    use skein_forge::forgejo::request::Page;
    let cap = super::host::limits().inner;
    let mut run = at(SETTINGS);
    assert_eq!(
        run.step(fixtures::host_bounded_settings()),
        send(Request::Tokens { login: b"root".as_slice().into(), page: Page { page: Some(1), limit: Some(cap) } })
    );
    let mut rows = skein_lib::List::with_capacity(cap);
    for id in 1..=cap {
        rows.push(take_over_row(u64::from(id), TAKEN_SUFFIX)).expect("page fits");
    }
    assert_eq!(
        run.step(fixtures::rows(rows.into_boxed())),
        send(Request::Tokens { login: b"root".as_slice().into(), page: Page { page: Some(2), limit: Some(cap) } })
    );
    assert_eq!(run.step(fixtures::rows(Box::new([take_over_row(99, TAKEN_SUFFIX)]))), done());
}
