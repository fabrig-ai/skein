use super::*;
use crate::tests::fixtures;

// forge-connection.md 6.1 reserves the listener's owner prefix.
const LISTENER_OWNER: u64 = 1_u64 << 59_u32;
const LAST_API_OWNER: u64 = LISTENER_OWNER - 1;
fn token_fixture() -> (Component, Env<Limits>, Queue<skein_io::Request>) {
    let limits = fixtures::limits();
    let c = Component::new(fixtures::settings(), &limits, &fixtures::shared()).expect("valid");
    let env = Env { now: Time::ZERO, wall: skein_lib::Wall::EPOCH, limits };
    (c, env, Queue::uncapped())
}
#[test]
#[should_panic(expected = "connection token prefix")]
fn api_tokens_stop_before_the_listener_range() {
    let (mut c, env, mut io) = token_fixture();
    c.next_token = LISTENER_OWNER;
    c.open_own(0, &env, &mut io);
}
#[test]
fn the_last_api_token_is_still_issued() {
    let (mut c, env, mut io) = token_fixture();
    c.next_token = LAST_API_OWNER;
    assert_eq!(c.open_own(0, &env, &mut io).raw(), LAST_API_OWNER);
}

#[test]
fn forgejo_starts_leave_the_github_reserved_slot_free() {
    // Three Forgejo starts contend for the two call slots; GitHub has its own third slot.
    const FORGEJO_FORGES: u32 = 3;
    const GITHUB_AT: u32 = FORGEJO_FORGES;
    const FORGES: u32 = FORGEJO_FORGES + 1;
    let limits = Limits { forges: FORGES, ..fixtures::limits() };
    let mut settings = fixtures::settings();
    let mut forges = List::with_capacity(FORGES);
    for number in 1..=FORGEJO_FORGES {
        let mut forge = fixtures::settings().forges.into_boxed().into_iter().next().expect("fixture forge");
        forge.number = u16::try_from(number).expect("forge number");
        forges.push(forge).expect("forge room");
    }
    let mut github = fixtures::github_settings().forges.into_boxed().into_iter().next().expect("GitHub forge");
    github.number = u16::try_from(FORGES).expect("forge number");
    forges.push(github).expect("forge room");
    settings.forges = forges;
    let env = Env { now: Time::ZERO, wall: skein_lib::Wall::EPOCH, limits };
    let mut c = Component::new(settings, &limits, &fixtures::shared()).expect("fixture limits");
    let mut up = Queue::uncapped();
    let mut io = Queue::uncapped();
    c.begin(&env, &mut up, &mut io);
    assert_eq!(c.call_connections(), limits.connections);
    assert!(c.own_tokens.get(GITHUB_AT).expect("GitHub slot").is_some());
    assert_eq!(c.connections.len(), 3);
}

use crate::tests::github_renewal::{ready as github_ready, renewed_contents, renewed_full, renewing};

fn refuse(h: &mut fixtures::Harness, revision: u64) {
    h.component.github_token_refused(0, 1, revision, &h.env);
}
fn revision(h: &fixtures::Harness) -> u64 {
    h.component.test_github().rows.get(&1).expect("row").token_revision
}
#[test]
fn call_401_coalesces_one_renewal() {
    let mut h = github_ready();
    let revision = revision(&h);
    refuse(&mut h, revision);
    refuse(&mut h, revision);
    let row = h.component.test_github().rows.get(&1).expect("row");
    assert!(row.full.pending && row.contents.pending);
    h.run(skein_lib::Duration::ZERO);
    h.connected(fixtures::FORGE);
    renewed_full(&mut h);
    let gh = h.component.test_github();
    assert_eq!(gh.mint, Some((1, skein_forge::github::request::Scope::Contents)));
    let row = gh.rows.get(&1).expect("row");
    assert!(!row.full.pending && !row.contents.pending);
    renewed_contents(&mut h);
    h.run(skein_lib::Duration::ZERO);
    assert_eq!(h.git_tokens.as_ref().expect("token history").len(), 2);
    match h.component.test_github().work {
        crate::installations::Work::Idle => {}
        crate::installations::Work::Minting { .. }
        | crate::installations::Work::Identity { .. }
        | crate::installations::Work::Waiting { .. } => unreachable!("coalesced scopes settled"),
    }
}
#[test]
fn call_401_coalesces_an_already_running_full_renewal() {
    let mut h = renewing();
    let revision = revision(&h);
    refuse(&mut h, revision);
    refuse(&mut h, revision);
    assert!(!h.component.test_github().rows.get(&1).expect("row").full.pending);
    renewed_full(&mut h);
    let gh = h.component.test_github();
    assert_eq!(gh.mint, Some((1, skein_forge::github::request::Scope::Contents)));
    assert!(!gh.rows.get(&1).expect("row").full.pending);
}
#[test]
fn stale_call_401_cannot_renew_a_replaced_full() {
    let mut h = renewing();
    let old = revision(&h);
    renewed_full(&mut h);
    refuse(&mut h, old);
    let row = h.component.test_github().rows.get(&1).expect("row");
    assert!(!row.full.pending && !row.contents.pending);
    assert_eq!(row.refused_revision, None);
}
#[test]
fn call_401_cannot_revive_gone() {
    let mut h = github_ready();
    let old = revision(&h);
    h.send(Request::Installations { forge: fixtures::FORGE, accepted: Box::from([]) });
    refuse(&mut h, old);
    let row = h.component.test_github().rows.get(&1).expect("Gone row");
    assert!(!row.full.pending && !row.contents.pending);
    assert_eq!(row.refused_revision, None);
}
#[test]
fn call_401_cannot_revive_a_replaced_connect() {
    let mut h = github_ready();
    let old = revision(&h);
    h.send(Request::Connect {
        forge: fixtures::FORGE,
        access: fixtures::app_access(skein_fake_forge_github::keys::KEYS.first().expect("public key"), 7, 9),
    });
    refuse(&mut h, old);
    let row = h.component.test_github().rows.get(&1).expect("replacement row");
    assert!(!row.full.pending && !row.contents.pending);
    assert_eq!(row.refused_revision, None);
}
#[test]
fn call_401_during_backoff_keeps_one_retry() {
    let mut h = renewing();
    h.answer(fixtures::FORGE, &fixtures::status(503));
    let until = h.component.test_github().deadline();
    let revision = revision(&h);
    refuse(&mut h, revision);
    refuse(&mut h, revision);
    assert_eq!(h.component.test_github().deadline(), until);
    let row = h.component.test_github().rows.get(&1).expect("row");
    assert!(!row.full.pending && row.contents.pending);
}

#[test]
fn call_401_coalesces_an_already_running_contents_renewal() {
    let mut h = crate::tests::github_renewal::contents_renewing();
    let revision = revision(&h);
    refuse(&mut h, revision);
    refuse(&mut h, revision);
    let row = h.component.test_github().rows.get(&1).expect("row");
    assert!(row.full.pending);
    assert!(!row.contents.pending);
    renewed_contents(&mut h);
    renewed_full(&mut h);
    h.run(skein_lib::Duration::ZERO);
    assert_eq!(h.git_tokens.as_ref().expect("token history").len(), 2);
    assert!(h.component.test_github().mint.is_none());
}

#[test]
fn renewal_local_signing_refusal_stops_invalid_key() {
    let mut h = github_ready();
    h.component.settings.forges.get_mut(0).expect("forge").access.as_mut().expect("access").credential =
        skein_forge::credential::Credential::App { id: 7, key: skein_lib::Secret::new(Box::from([0; 64])) };
    h.component.test_github_mut().jwt.forget();
    let revision = revision(&h);
    refuse(&mut h, revision);
    h.run(skein_lib::Duration::ZERO);
    h.connected(fixtures::FORGE);
    h.run(skein_lib::Duration::ZERO);
    assert_eq!(h.stopped(), Some((fixtures::FORGE, crate::Stopped::Invalid { value: crate::Value::Key })));
    assert_eq!(h.connected_count, 1);
}
