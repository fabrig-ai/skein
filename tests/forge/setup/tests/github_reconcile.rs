use skein_forge_setup::{
    host::{Need, Plan},
    item::{Item, accepted_item},
    reconcile::{Outcome, Stage, Why},
};
use skein_forge_setup_world::{
    Connect, Heard, HostWorld,
    fixtures::{
        self,
        github_stories::{
            FORGE, accept, accepted, closed, configured, connected, done, header, install, mutations, query, reconcile,
            request, response, writes,
        },
    },
};
use skein_lib::{Time, Wall};

#[test]
fn reconcile_prunes_missing_ids_and_uninstalls_old_strays() {
    // Signed: App, hook GET, three listing pages, DELETE 2, DELETE 3; fake checks each.
    let mut world = configured(&[]);
    world.put(Item::Accepted, accepted_item(&[1, 99]));
    world.pause_writes();
    reconcile(&mut world, 1);
    let routes: Vec<_> =
        world.github_requests().iter().map(|r| (r.method.as_str(), r.target.as_str(), r.auth)).collect();
    assert_eq!(
        routes,
        [
            ("GET", "/app", "App"),
            ("GET", "/app/hook/config", "App"),
            ("GET", "/app/installations?per_page=100&page=1", "App"),
            ("GET", "/app/installations?per_page=100&page=2", "App"),
            ("GET", "/app/installations?per_page=100&page=3", "App"),
        ]
    );
    assert!(world.heard().is_empty(), "Done waits for persistence");
    assert_eq!(world.github_installation_ids(), [1, 2, 3]);
    assert_eq!(world.needs().last(), Some(&Need::Write { item: Item::Accepted, bytes: accepted_item(&[1]) }));
    world.write_pending();
    world.settle();
    done(&world, 1);
    assert_eq!(
        mutations(&world, 0),
        [("DELETE".into(), "/app/installations/2".into()), ("DELETE".into(), "/app/installations/3".into())]
    );
    assert_eq!(writes(&world, 0), [Item::Accepted]);
    assert_eq!(accepted(&world).as_ref(), [1]);
    assert_eq!(world.github_installation_ids(), [1]);
    closed(&mut world);
}

#[test]
fn a_second_github_reconcile_writes_nothing() {
    // Signed: first App/hook/three pages/two deletes, second App/hook/two pages.
    let mut world = configured(&[]);
    world.put(Item::Accepted, accepted_item(&[1, 99]));
    reconcile(&mut world, 1);
    done(&world, 1);
    let (needs, requests) = (world.needs().len(), world.github_requests().len());
    reconcile(&mut world, 2);
    done(&world, 2);
    assert!(writes(&world, needs).is_empty());
    assert!(mutations(&world, requests).is_empty());
    closed(&mut world);
}

#[test]
fn a_young_or_future_stray_stays_until_past_state_life() {
    // Signed: App/hook/three pages, then App/hook/three pages/delete (22 signings).
    let life = fixtures::github_given().limits.state_life.as_nanos();
    let wall = skein_forge_machine_world::github::WALL_BASE.as_nanos();
    let mut world = configured(&[]);
    world.put(Item::Accepted, accepted_item(&[]));
    let young = world.github_reinsert(1, Wall::from_nanos(wall));
    let boundary = world.github_reinsert(2, Wall::from_nanos(wall - life));
    let future = world.github_reinsert(3, Wall::from_nanos(wall + life));
    reconcile(&mut world, 1);
    done(&world, 1);
    for id in [young, boundary, future] {
        assert!(world.github_installation_ids().contains(&id));
    }
    assert!(mutations(&world, 0).is_empty());
    world.advance_to(Time::from_nanos(1));
    reconcile(&mut world, 2);
    done(&world, 2);
    for (id, retained) in [(young, true), (boundary, false), (future, true)] {
        assert_eq!(world.github_installation_ids().contains(&id), retained, "id {id}");
    }
    closed(&mut world);
}

#[test]
fn an_install_state_accepts_before_reconcile_can_remove_the_stray() {
    // Signed: conversion proof, Accept, App, hook, three pages, two deletes (18 signings).
    let (mut world, _) = connected(7);
    // Conversion starts a fresh App, so its installation store has room.
    let (state, url) = install(&mut world, 3);
    let reply = world.github_page(&request("GET", core::str::from_utf8(&url).expect("URL"), b""));
    let (head, _) = response(&reply, 302);
    let fields = query(header(head, "Location").expect("setup redirect"));
    assert_eq!(fields["state"], state.as_ref());
    let id: u64 = core::str::from_utf8(&fields["installation_id"]).expect("id text").parse().expect("id");
    accept(&mut world, 4, &state, id);
    let life = fixtures::github_given().limits.state_life.as_nanos();
    world.advance_to(Time::from_nanos(life + 1));
    reconcile(&mut world, 5);
    done(&world, 5);
    assert_eq!(accepted(&world).as_ref(), [id]);
    assert_eq!(world.github_installation_ids(), [id]);
    closed(&mut world);
}

#[test]
fn reconcile_sets_the_hook_to_the_apps_invalid_host() {
    // Signed: App/hook/PATCH/three pages/delete, then App/hook/two pages (22 signings).
    let mut world = HostWorld::github_scenario(
        7,
        &[],
        skein_fake_forge_github::service::AppFacts::Complete,
        b"https://wrong.test/",
    );
    reconcile(&mut world, 1);
    done(&world, 1);
    assert_eq!(
        mutations(&world, 0),
        [("PATCH".into(), "/app/hook/config".into()), ("DELETE".into(), "/app/installations/3".into())]
    );
    assert!(world.file(Item::WebhookSecret).is_none());
    let requests = world.github_requests().len();
    reconcile(&mut world, 2);
    done(&world, 2);
    assert!(mutations(&world, requests).is_empty());
    closed(&mut world);
}

#[test]
fn a_lacking_permission_does_not_skip_hook_or_installation_checks() {
    // Signed: App/hook/PATCH/three pages/delete (14 signings).
    let mut world = HostWorld::github(7, &[]); // Configured App has empty metadata.
    reconcile(&mut world, 1);
    let Some(Heard::Done { outcome: Outcome::Lacking { lacking }, .. }) = world.heard().last() else {
        panic!("Lacking")
    };
    assert!(lacking.iter().any(|entry| matches!(entry, skein_forge_setup::github::Lacking::Permission { .. })));
    assert!(world.github_requests().iter().any(|row| row.method == "PATCH"));
    assert_eq!(world.github_installation_ids(), [1, 2]);
    closed(&mut world);
}

#[test]
fn a_failed_second_listing_page_does_not_drop_accepted_or_uninstall() {
    // Each row signs App/hook/two pages; scripts have no fake signing checks.
    for (status, body) in [(200, b"{broken".as_slice()), (403, b"{}".as_slice())] {
        let mut world = configured(&[Connect::Made, Connect::Made, Connect::Made, Connect::Answers(status, body)]);
        let before = accepted(&world);
        reconcile(&mut world, 1);
        assert!(
            matches!(world.heard().last(), Some(Heard::Stop { failure, .. }) if failure.step == Stage::Installation)
        );
        assert_eq!(accepted(&world), before);
        assert!(writes(&world, 0).is_empty());
        assert!(mutations(&world, 0).is_empty());
        assert_eq!(world.github_installation_ids(), [1, 2, 3]);
        closed(&mut world);
    }
}

#[test]
fn a_key_the_fake_refuses_stops_before_any_mutation() {
    // One signed App request and one fake check, using a different public fixture key.
    let mut world = configured(&[]);
    world.put(Item::AppKey, fixtures::secret(skein_fake_forge_github::keys::KEYS[1]));
    reconcile(&mut world, 1);
    assert!(
        matches!(world.heard().last(), Some(Heard::Stop { failure, .. }) if failure.step == Stage::App && failure.subject.as_ref() == b"7" && failure.why == Why::Refused(401))
    );
    assert_eq!(world.github_requests().len(), 1);
    assert!(writes(&world, 0).is_empty());
    assert!(mutations(&world, 0).is_empty());
    closed(&mut world);
}

#[test]
fn github_setup_traces_and_failures_contain_no_secret() {
    // Conversion proof, Accept, mint, reconcile and one diagnostic JWT: 21 signings.
    let (mut world, code) = connected(7);
    let (state, _) = install(&mut world, 3);
    accept(&mut world, 4, &state, 1);
    world.run_plan(
        5,
        Plan::Reachable { forge: FORGE, installation: Some(1), page: core::num::NonZeroU32::new(1).expect("page") },
    );
    world.settle();
    reconcile(&mut world, 6);
    done(&world, 6);
    accept(&mut world, 7, &state, 1);
    let diagnostics = format!("{:?} {:?} {:?}", world.github_requests(), world.needs(), &world.heard()[1..]);
    let der = skein_forge_machine_world::github::app_key();
    let pem = skein_fake_forge_github::keys::pem(der).expect("fixture PEM");
    let mut rng = skein_lib::Rng::new(7);
    let hex: Vec<_> = (0..52).map(|_| b"0123456789abcdef"[usize::try_from(rng.below(16)).expect("nibble")]).collect();
    assert!(hex[..20] == *code.expose(), "fixture conversion code draw");
    let mut rng = skein_lib::Rng::new(7);
    let mut token = b"ghs_".to_vec();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    for _ in 0..36 {
        token.push(alphabet[usize::try_from(rng.below(62)).expect("alphanumeric")]);
    }
    let issued = Wall::from_nanos(skein_forge_machine_world::github::WALL_BASE.as_nanos() - 60_000_000_000);
    let expires = Wall::from_nanos(issued.as_nanos() + skein_forge::github::jwt::LIFE.as_nanos());
    let bearer = skein_forge_machine_world::github::bearer(der, 8, issued, expires);
    let jwt = bearer.strip_prefix(b"Bearer ").expect("diagnostic bearer");
    for secret in [der, pem.as_ref(), code.expose(), &hex[20..], token.as_slice(), jwt, b"ghs_", b"eyJ"] {
        assert!(!diagnostics.as_bytes().windows(secret.len()).any(|part| part == secret));
        for spelling in [format!("{secret:?}"), format!("{secret:x?}"), format!("{secret:X?}")] {
            assert!(!diagnostics.contains(&spelling));
        }
    }
    for item in [Item::Accepted, Item::User, Item::WebhookSecret, Item::Pasted, Item::ApiToken, Item::GitToken] {
        if let Some(value) = world.file(item) {
            assert!(!value.expose().windows(der.len()).any(|part| part == der));
        }
    }
    closed(&mut world);
}
