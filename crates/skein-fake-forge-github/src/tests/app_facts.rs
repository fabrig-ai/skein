use super::{fixtures as f, wall};
use crate::{
    keys,
    service::{AppFacts, Problem, Service},
};
use skein_forge::github::{
    document::{Access, Document, Kind, Permission},
    request::Method,
    setup,
};
use skein_lib::bytes;

#[test]
fn github_fake_conversion_keeps_the_manifests_app_facts() {
    for scenario in [AppFacts::Complete, AppFacts::Omitted, AppFacts::Narrow] {
        let mut config = f::config();
        config.app_facts = scenario;
        let mut h = f::Harness::with(f::page_limits(), config);
        let answer = f::create(&mut h, b"s1", f::NAME);
        let response = f::convert(&mut h, &f::code(&answer));
        let converted = f::conversion(&h, &response);
        h.jwt = f::authorization_for(converted.id, keys::KEYS.first().expect("test key"), 1000, 1600);
        let response = h.app(Method::Get, b"/app", b"");
        assert_eq!(response.status, 200);
        let (_, Document::App(app)) = h.decode(Kind::App, &response) else { panic!("App document") };
        assert_eq!(app.id, converted.id);
        assert_eq!(app.slug, converted.slug);
        let posted = h.service.manifest(b"s1").expect("posted manifest");
        assert_eq!(posted.default_permissions.len(), 9);
        assert_eq!(posted.default_events.len(), 14);
        let missing = setup::lacks(&app);
        match scenario {
            AppFacts::Complete => {
                assert_eq!(app.permissions.as_ref(), posted.default_permissions.as_slice());
                assert_eq!(app.events.as_ref(), posted.default_events.as_slice());
                assert!(missing.permissions.is_empty());
                assert!(missing.events.is_empty());
            }
            AppFacts::Omitted => {
                assert!(bytes::find(&response.body, b"permissions").is_none());
                assert!(bytes::find(&response.body, b"events").is_none());
                assert!(app.permissions.is_empty());
                assert!(app.events.is_empty());
                assert_eq!(h.service.app.permissions.as_ref(), posted.default_permissions.as_slice());
                assert_eq!(h.service.app.events.as_ref(), posted.default_events.as_slice());
                assert_eq!(missing.permissions.len(), 9);
                assert_eq!(missing.events.len(), 14);
            }
            AppFacts::Narrow => {
                assert_eq!(
                    app.permissions.as_ref(),
                    &[Permission { name: f::boxed(b"contents"), access: Access::Read }]
                );
                assert_eq!(app.events.as_ref(), &[f::boxed(b"issues")]);
                assert_eq!(missing.permissions.len(), 9);
                assert_eq!(
                    missing.permissions.first().expect("missing write"),
                    &Permission { name: f::boxed(b"contents"), access: Access::Write }
                );
                assert_eq!(missing.events.len(), 13);
                assert!(!missing.events.contains(&f::boxed(b"issues")));
            }
        }
        assert_eq!((h.calls, h.hooks), (0, 0));
    }
}

#[test]
fn github_fake_conversion_keeps_unrequested_names_and_admin() {
    let mut h = f::pages();
    let input = br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"http://setup.test/x","default_permissions":{"extra":"admin","contents":"write","metadata":"read"},"default_events":["extra_event","push"]}"#;
    let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(input));
    assert_eq!(answer.status, 200);
    let response = f::convert(&mut h, &f::code(&answer));
    let converted = f::conversion(&h, &response);
    h.jwt = f::authorization_for(converted.id, keys::KEYS.first().expect("test key"), 1000, 1600);
    let response = h.app(Method::Get, b"/app", b"");
    let (_, Document::App(app)) = h.decode(Kind::App, &response) else { panic!("App document") };
    let posted = h.service.manifest(b"s1").expect("posted manifest");
    assert_eq!(app.permissions.as_ref(), posted.default_permissions.as_slice());
    assert_eq!(app.events.as_ref(), posted.default_events.as_slice());
    assert_eq!(app.permissions.first().expect("extra permission").access, Access::Admin);
    assert_eq!(app.permissions.first().expect("extra permission").name.as_ref(), b"extra");
    assert_eq!(app.events.first().expect("extra event").as_ref(), b"extra_event");
}

#[test]
fn github_fake_an_empty_manifest_converts_to_empty_app_facts() {
    for scenario in [AppFacts::Complete, AppFacts::Narrow] {
        let mut config = f::config();
        config.app_facts = scenario;
        let mut h = f::Harness::with(f::page_limits(), config);
        let input = br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"http://setup.test/x"}"#;
        let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(input));
        let response = f::convert(&mut h, &f::code(&answer));
        let converted = f::conversion(&h, &response);
        h.jwt = f::authorization_for(converted.id, keys::KEYS.first().expect("test key"), 1000, 1600);
        let response = h.app(Method::Get, b"/app", b"");
        let (_, Document::App(app)) = h.decode(Kind::App, &response) else { panic!("App document") };
        assert!(app.permissions.is_empty());
        assert!(app.events.is_empty());
    }
}

#[test]
fn github_fake_config_app_facts_obey_name_and_list_bounds() {
    for invalid in 0..4_u32 {
        let mut config = f::config();
        match invalid {
            0 => {
                config.app.permissions = Box::from([Permission { name: f::boxed(&[b'x'; 257]), access: Access::Read }]);
            }
            1 => config.app.events = Box::from([f::boxed(&[b'x'; 257])]),
            2 | 3 => {}
            _ => panic!("fixture case"),
        }
        if invalid == 2 {
            let mut permissions = skein_lib::List::with_capacity(9);
            for _entry in 0..9_u32 {
                permissions.push(Permission { name: f::boxed(b"x"), access: Access::Read }).expect("fixture count");
            }
            config.app.permissions = permissions.into_boxed();
        }
        if invalid == 3 {
            let mut events = skein_lib::List::with_capacity(9);
            for _entry in 0..9_u32 {
                events.push(f::boxed(b"x")).expect("fixture count");
            }
            config.app.events = events.into_boxed();
        }
        assert_eq!(Service::new(config, &f::limits(), 1).expect_err("invalid App facts"), Problem::Config);
    }
    let mut config = f::config();
    let mut permissions = skein_lib::List::with_capacity(8);
    let mut events = skein_lib::List::with_capacity(8);
    for _entry in 0..8_u32 {
        permissions.push(Permission { name: f::boxed(&[b'x'; 256]), access: Access::Admin }).expect("fixture count");
        events.push(f::boxed(&[b'x'; 256])).expect("fixture count");
    }
    config.app.permissions = permissions.into_boxed();
    config.app.events = events.into_boxed();
    let mut h = f::Harness::with(f::limits(), config);
    h.now = wall(1200);
    let response = h.app(Method::Get, b"/app", b"");
    assert_eq!(response.status, 200);
    let (_, Document::App(app)) = h.decode(Kind::App, &response) else { panic!("App document") };
    assert_eq!(app.permissions.len(), 8);
    assert_eq!(app.events.len(), 8);
}
