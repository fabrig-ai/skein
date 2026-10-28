use super::fixtures::{self, CheckRun, secret};
use crate::call::Auth;
use crate::{
    item::Item,
    reconcile::{Event, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::forgejo::request::Request;

fn person() -> Event {
    fixtures::user_event(fixtures::user(7, b"alice", false))
}
fn identity() -> Need {
    Need::Done(Outcome::Identity { id: 7, login: b"alice".as_slice().into() })
}
fn failure(subject: &[u8], why: Why) -> Need {
    fixtures::named_failure(Stage::Token, subject, why)
}
fn prefix() -> [Event; 6] {
    [
        Event::Begin,
        Event::Read(Some(secret(b"api-token"))),
        person(),
        Event::Read(Some(secret(b"git-token"))),
        person(),
        Event::Read(None),
    ]
}
// Number of accepted events before each outstanding need.
const API_READ: usize = 1;
const API_USER: usize = 2;
const GIT_READ: usize = 3;
const GIT_USER: usize = 4;
const USER_READ: usize = 5;
const USER_WRITTEN: usize = 6;
fn at(phase: usize) -> CheckRun {
    let mut run = CheckRun::new();
    for event in prefix().into_iter().take(phase) {
        run.step(event);
    }
    run
}

#[test]
fn the_api_token_is_sent_to_the_users_route_and_the_git_token_after_it() {
    let mut run = CheckRun::new();
    assert_eq!(run.step(Event::Begin), Need::Read(Item::ApiToken));
    assert_eq!(
        run.step(Event::Read(Some(secret(b"api-token")))),
        Need::Send { request: Request::User.into(), auth: Auth::Token(secret(b"api-token")) }
    );
    assert_eq!(run.step(person()), Need::Read(Item::GitToken));
    assert_eq!(
        run.step(Event::Read(Some(secret(b"git-token")))),
        Need::Send { request: Request::User.into(), auth: Auth::Token(secret(b"git-token")) }
    );
    assert_eq!(run.step(person()), Need::Read(Item::User));
}

#[test]
fn a_user_not_yet_kept_is_written_as_eight_bytes_then_done_with_the_identity() {
    let mut run = at(USER_READ);
    assert_eq!(run.step(Event::Read(None)), Need::Write { item: Item::User, bytes: secret(&7u64.to_le_bytes()) });
    assert_eq!(run.step(Event::Written), identity());
}

#[test]
fn a_kept_user_equal_to_the_tokens_is_not_written_again() {
    assert_eq!(at(USER_READ).step(Event::Read(Some(secret(&7u64.to_le_bytes())))), identity());
}

#[test]
fn a_kept_user_that_differs_is_other_user_naming_api() {
    assert_eq!(at(USER_READ).step(Event::Read(Some(secret(&8u64.to_le_bytes())))), failure(b"api", Why::OtherUser));
}

#[test]
fn a_kept_value_that_does_not_decode_is_other_user() {
    for value in [b"7".as_slice(), &0u64.to_le_bytes()] {
        assert_eq!(at(USER_READ).step(Event::Read(Some(secret(value)))), failure(b"api", Why::OtherUser));
    }
}

#[test]
fn an_empty_kept_user_is_none() {
    assert_eq!(
        at(USER_READ).step(Event::Read(Some(secret(b"")))),
        Need::Write { item: Item::User, bytes: secret(&7u64.to_le_bytes()) }
    );
}

#[test]
fn an_api_token_of_a_site_admin_is_refused_bot_is_admin() {
    assert_eq!(
        at(API_USER).step(fixtures::user_event(fixtures::user(1, b"root", true))),
        failure(b"api", Why::BotIsAdmin)
    );
}

#[test]
fn a_user_of_id_zero_is_unexpected() {
    assert_eq!(
        at(API_USER).step(fixtures::user_event(fixtures::user(0, b"alice", false))),
        failure(b"api", Why::Unexpected)
    );
}

#[test]
fn a_missing_or_empty_api_token_stops_before_any_request() {
    for value in [None, Some(secret(b""))] {
        assert_eq!(at(API_READ).step(Event::Read(value)), failure(b"api", Why::Missing));
    }
}

#[test]
fn a_missing_or_empty_git_token_stops_naming_git() {
    for value in [None, Some(secret(b""))] {
        assert_eq!(at(GIT_READ).step(Event::Read(value)), failure(b"git", Why::Missing));
    }
}

#[test]
fn a_git_token_of_another_user_is_other_user_naming_git() {
    assert_eq!(
        at(GIT_USER).step(fixtures::user_event(fixtures::user(8, b"another", false))),
        failure(b"git", Why::OtherUser)
    );
}

#[test]
fn a_refused_token_names_which_token_and_the_status() {
    for (phase, subject, status) in
        [(API_USER, b"api".as_slice(), 401), (GIT_USER, b"git".as_slice(), 401), (GIT_USER, b"git".as_slice(), 403)]
    {
        assert_eq!(at(phase).step(Event::Reply(Reply::Status(status))), failure(subject, Why::Refused(status)));
    }
}

#[test]
fn a_forge_that_stops_answering_stops_the_check_naming_the_token() {
    for (phase, subject) in [(API_USER, b"api".as_slice()), (GIT_USER, b"git".as_slice())] {
        assert_eq!(at(phase).step(Event::Reply(Reply::Down)), failure(subject, Why::Down));
    }
}

#[test]
fn a_reply_that_is_not_a_user_is_unexpected() {
    for (phase, subject) in [(API_USER, b"api".as_slice()), (GIT_USER, b"git".as_slice())] {
        for event in [fixtures::rows(Box::new([])), fixtures::settings_event(2), Event::Reply(Reply::Empty)] {
            assert_eq!(at(phase).step(event), failure(subject, Why::Unexpected));
        }
    }
}

pub(super) fn phase_cases() -> [(&'static str, CheckRun, Event, Stage, &'static [u8]); 7] {
    [
        ("begin", 0, Event::Written, b"api".as_slice()),
        ("api read", API_READ, Event::Begin, b"api".as_slice()),
        ("api user", API_USER, Event::Begin, b"api".as_slice()),
        ("git read", GIT_READ, Event::Begin, b"git".as_slice()),
        ("git user", GIT_USER, Event::Begin, b"git".as_slice()),
        ("user read", USER_READ, Event::Begin, b"api".as_slice()),
        ("user written", USER_WRITTEN, Event::Begin, b"api".as_slice()),
    ]
    .map(|(name, phase, wrong, subject)| (name, at(phase), wrong, Stage::Token, subject))
}
#[test]
fn an_out_of_order_event_stops_each_check_phase() {
    for (name, mut run, wrong, _, subject) in phase_cases() {
        assert_eq!(run.step(wrong), failure(subject, Why::Unexpected), "{name}");
    }
}

pub(super) fn terminal_runs() -> [CheckRun; 2] {
    let mut done = at(USER_READ);
    assert_eq!(done.step(Event::Read(Some(secret(&7u64.to_le_bytes())))), identity());
    let mut stop = at(API_READ);
    assert_eq!(stop.step(Event::Read(None)), failure(b"api", Why::Missing));
    [done, stop]
}
#[test]
fn an_ended_check_ignores_every_event() {
    for mut run in terminal_runs() {
        for event in [Event::Begin, Event::Read(None), Event::Reply(Reply::Down), Event::Written, fixtures::drawn()] {
            let mut out = skein_lib::Queue::uncapped();
            crate::check::step(&mut run.machine, event, &mut out);
            assert!(out.is_empty());
        }
    }
}
