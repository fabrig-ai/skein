use super::fixtures::{self, AdoptRun, secret};
use crate::{
    adopt::Adopt,
    given::{Place, Refused},
    item::Item,
    reconcile::{Event, Need, Reply, Stage, Why},
};
use skein_forge::forgejo::{
    document::Permission,
    request::{Grant, Repo, Request},
};
fn at() -> Repo {
    Repo { owner: b"acme".as_slice().into(), name: b"specimen".as_slice().into() }
}
fn permission_read() -> Request {
    Request::Permission { at: at(), login: b"kit".as_slice().into() }
}
fn add() -> Request {
    Request::AddCollaborator { at: at(), login: b"kit".as_slice().into(), permission: Grant::Write }
}
// A prefix for each request, including all three access phases.
fn request(phase: usize) -> AdoptRun {
    let mut run = fixtures::adopt();
    run.step(Event::Begin);
    run.step(Event::Read(Some(secret(fixtures::ADMIN))));
    if phase > 0 {
        run.step(fixtures::user_event(fixtures::admin()));
    }
    if phase > 1 {
        run.step(fixtures::repository(7, b"acme", b"specimen"));
    }
    if phase > 2 {
        run.step(fixtures::permission(Permission::None));
    }
    run
}
fn failure(phase: usize, why: Why) -> Need {
    fixtures::named_failure(
        if phase == 0 { Stage::Admin } else { Stage::Repository },
        match phase {
            0 => b"kit-admin",
            1 => b"7",
            _ => b"acme/specimen",
        },
        why,
    )
}
#[test]
fn adopt_checks_the_admins_token_then_reads_the_repository_by_id() {
    let mut run = fixtures::adopt();
    assert_eq!(run.step(Event::Begin), Need::Read(Item::AdminToken));
    assert_eq!(run.step(Event::Read(Some(secret(fixtures::ADMIN)))), fixtures::send(Request::User));
    assert_eq!(run.step(fixtures::user_event(fixtures::admin())), fixtures::send(Request::RepositoryById { id: 7 }));
}
#[test]
fn a_missing_or_empty_admin_token_stops_adopt_naming_the_admin() {
    for value in [None, Some(secret(b""))] {
        let mut run = fixtures::adopt();
        run.step(Event::Begin);
        assert_eq!(run.step(Event::Read(value)), failure(0, Why::Missing));
    }
}
#[test]
fn a_non_admin_stops_adopt_naming_the_admin() {
    assert_eq!(request(0).step(fixtures::user_event(fixtures::person(b"ada", b"Ada"))), failure(0, Why::NotAdmin));
}
#[test]
fn the_permission_is_read_of_the_path_the_forge_gave() {
    assert_eq!(request(1).step(fixtures::repository(7, b"acme", b"specimen")), fixtures::send(permission_read()));
}
#[test]
fn a_bot_with_write_or_more_is_left_as_it_is() {
    for permission in [Permission::Write, Permission::Admin, Permission::Owner] {
        assert_eq!(request(2).step(fixtures::permission(permission)), Need::Done(fixtures::done()));
    }
}
#[test]
#[expect(
    clippy::disallowed_macros,
    reason = "the test inspects the ended adoption's Debug rendering for a dropped token"
)]
fn a_bot_below_write_is_made_a_collaborator_with_write() {
    for permission in [Permission::None, Permission::Read] {
        let mut run = request(2);
        assert_eq!(run.step(fixtures::permission(permission)), fixtures::send(add()));
        assert_eq!(run.step(Event::Reply(Reply::Empty)), Need::Done(fixtures::done()));
        assert!(format!("{:?}", run.machine).contains("admin: None"));
    }
}
#[test]
fn a_repository_the_forge_no_longer_has_stops_adopt_missing_naming_its_id() {
    assert_eq!(request(1).step(Event::Reply(Reply::Status(404))), failure(1, Why::Missing));
}
#[test]
fn a_refused_write_names_the_repositorys_path() {
    for (phase, status) in [(3, 403), (1, 403), (2, 404)] {
        assert_eq!(request(phase).step(Event::Reply(Reply::Status(status))), failure(phase, Why::Refused(status)));
    }
}
#[test]
fn a_forge_that_stops_answering_stops_adopt_at_each_request() {
    for phase in 0..4 {
        assert_eq!(request(phase).step(Event::Reply(Reply::Down)), failure(phase, Why::Down));
    }
}
#[test]
fn a_reply_of_another_kind_is_unexpected_at_each_request() {
    for phase in 0..4 {
        assert_eq!(request(phase).step(fixtures::settings_event(2)), failure(phase, Why::Unexpected));
    }
}
#[test]
fn a_repository_of_another_id_is_unexpected() {
    assert_eq!(request(1).step(fixtures::repository(8, b"acme", b"specimen")), failure(1, Why::Unexpected));
}
pub(super) fn phase_cases() -> [(&'static str, AdoptRun, Stage, &'static [u8]); 6] {
    let mut read = fixtures::adopt();
    read.step(Event::Begin);
    [
        ("begin", fixtures::adopt(), Stage::Admin, b"kit-admin".as_slice()),
        ("admin read", read, Stage::Admin, b"kit-admin".as_slice()),
        ("admin check", request(0), Stage::Admin, b"kit-admin".as_slice()),
        ("repository", request(1), Stage::Repository, b"7".as_slice()),
        ("permission", request(2), Stage::Repository, b"acme/specimen".as_slice()),
        ("collaborator", request(3), Stage::Repository, b"acme/specimen".as_slice()),
    ]
}
#[test]
fn an_out_of_order_event_stops_each_adopt_phase() {
    for (name, mut run, stage, subject) in phase_cases() {
        assert_eq!(run.step(Event::Written), fixtures::named_failure(stage, subject, Why::Unexpected), "{name}");
    }
    for phase in 0..4 {
        for event in [Event::Begin, Event::Read(None), fixtures::drawn()] {
            assert_eq!(request(phase).step(event), failure(phase, Why::Unexpected));
        }
    }
}
pub(super) fn terminal_runs() -> [AdoptRun; 2] {
    let mut done = request(2);
    done.step(fixtures::permission(Permission::Write));
    let mut stopped = request(3);
    stopped.step(Event::Reply(Reply::Down));
    [done, stopped]
}
#[test]
fn an_ended_adopt_ignores_every_event() {
    for mut run in terminal_runs() {
        for event in [Event::Begin, Event::Read(None), Event::Reply(Reply::Down), Event::Written, fixtures::drawn()] {
            let mut out = skein_lib::Queue::uncapped();
            crate::adopt::step(&mut run.machine, event, &mut out);
            assert!(out.is_empty());
        }
    }
}
#[test]
fn adopt_refuses_repository_zero_naming_its_place() {
    assert_eq!(Adopt::new(&fixtures::given(), 0).err(), Some(Refused { place: Place::Repository(0), other: None }));
}
#[test]
fn adopt_refuses_a_given_that_is_refused() {
    let mut given = fixtures::given();
    given.word = b"!".as_slice().into();
    assert_eq!(Adopt::new(&given, 7).err(), crate::given::check(&given).err());
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the redaction test inspects the adoption's Debug rendering")]
fn no_secret_is_in_an_adopts_failure_debug() {
    let mut run = request(3);
    let failed = run.step(Event::Reply(Reply::Status(403)));
    let debug = format!("{failed:?} {:?}", run.machine);
    for bytes in debug.as_bytes().windows(fixtures::ADMIN.len()) {
        assert_ne!(bytes, fixtures::ADMIN);
    }
    assert!(debug.contains("admin: None"));
}
