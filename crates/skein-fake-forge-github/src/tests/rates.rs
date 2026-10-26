use super::{fixtures as f, wall};
use crate::service::Account;
use skein_forge::github::request::Method;
use skein_lib::{Duration, List, Secret};

#[test]
fn github_fake_person_rates_belong_to_the_user_and_are_separate_from_installations() {
    let mut config = f::config();
    config.rate_quota = 2;
    let mut accounts = List::with_capacity(3);
    for account in config.accounts.into_boxed() {
        accounts.push(account).unwrap();
    }
    accounts.push(Account { login: f::boxed(b"user-one"), id: 1, organization: false }).unwrap();
    config.accounts = accounts;
    config.tokens = List::with_capacity(3);
    for (value, user) in [(b"first-person".as_slice(), 1), (b"same-person", 1), (b"other-person", 31)] {
        config.tokens.push((Secret::new(f::boxed(value)), user, None)).unwrap();
    }
    let mut h = f::Harness::with(f::limits(), config);
    let tokens = [
        f::bearer(b"first-person"),
        f::bearer(b"same-person"),
        f::bearer(b"other-person"),
        f::bearer(h.mint(1, false).token.expose()),
    ];
    // User 1 and installation 1 deliberately have the same numeric id.
    for (credential, status, used) in
        [(0, 200, 1), (1, 200, 2), (2, 200, 1), (3, 200, 1), (0, 403, 2), (2, 200, 2), (3, 200, 2)]
    {
        let target = if credential == 3 { f::REACHABLE } else { b"/user" };
        let response = h.ask(Method::Get, target, Some(tokens.get(credential).unwrap().expose()), b"");
        assert_eq!(response.status, status);
        let rate = response.rate.unwrap().rate;
        assert_eq!((rate.limit, rate.used), (2, used));
    }
    assert_eq!(h.calls, 0);
}

#[test]
fn github_fake_revocation_is_charged_once_and_rate_refusal_keeps_the_token() {
    for (exhausted, contents) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut config = f::config();
        config.rate_quota = 1;
        config.rate_window = Duration::from_secs(60);
        let mut h = f::Harness::with(f::limits(), config);
        // Revoke a nonzero slot, leaving another token of the installation alive.
        let other = f::bearer(h.mint(1, contents).token.expose());
        let token = f::bearer(h.mint(1, contents).token.expose());
        if exhausted {
            let response = h.ask(Method::Get, f::REACHABLE, Some(token.expose()), b"");
            assert_eq!(response.status, 200);
            assert_eq!(response.rate.unwrap().rate.used, 1);
        }
        let response = h.ask(Method::Delete, b"/installation/token", Some(token.expose()), b"");
        assert_eq!(response.status, if exhausted { 403 } else { 204 });
        assert_eq!(response.rate.unwrap().rate.used, 1);
        let response = h.ask(Method::Get, f::REACHABLE, Some(other.expose()), b"");
        assert_eq!(response.status, 403);
        assert_eq!(response.rate.unwrap().rate.used, 1);
        h.now = wall(1260);
        h.env.now = h.env.now.saturating_add(Duration::from_secs(60));
        h.env.wall = h.now;
        let response = h.ask(Method::Get, f::REACHABLE, Some(token.expose()), b"");
        assert_eq!(response.status, if exhausted { 200 } else { 401 });
        if exhausted {
            assert_eq!(response.rate.unwrap().rate.used, 1);
        } else {
            assert!(response.rate.is_none());
            let response = h.ask(Method::Get, f::REACHABLE, Some(other.expose()), b"");
            assert_eq!(response.status, 200);
            assert_eq!(response.rate.unwrap().rate.used, 1);
        }
        assert_eq!(h.calls, 0);
    }
}

#[test]
fn github_fake_tokens_on_app_routes_pay_before_the_credential_class_refusal() {
    for installation in [false, true] {
        let mut config = f::config();
        config.rate_quota = 1;
        config.tokens = List::with_capacity(1);
        config.tokens.push((Secret::new(f::boxed(b"invented-person")), 31, None)).unwrap();
        let mut h = f::Harness::with(f::limits(), config);
        let token =
            if installation { f::bearer(h.mint(1, false).token.expose()) } else { f::bearer(b"invented-person") };
        for status in [401, 403] {
            let response = h.ask(Method::Get, b"/app", Some(token.expose()), b"");
            assert_eq!(response.status, status);
            assert_eq!(response.rate.unwrap().rate.used, 1);
        }
        assert!(h.app(Method::Get, b"/app", b"").rate.is_none());
        assert_eq!(h.calls, 0);
    }
}

#[test]
fn github_fake_person_token_expiration_is_optional_and_follows_every_answer() {
    let expiry = skein_forge::forgejo::time::read(b"2026-11-19T12:00:00Z").unwrap();
    for expires in [None, Some(expiry)] {
        let mut config = f::config();
        config.tokens = List::with_capacity(1);
        config.tokens.push((Secret::new(f::boxed(b"invented-person")), 31, expires)).unwrap();
        let mut h = f::Harness::with(f::limits(), config);
        let token = f::bearer(b"invented-person");
        for (target, status) in [(b"/user".as_slice(), 200), (b"/app", 401)] {
            let response = h.ask(Method::Get, target, Some(token.expose()), b"");
            assert_eq!(response.status, status);
            let heads = response.rate.unwrap();
            assert_eq!(heads.expires, expires);
            let mut headers = List::with_capacity(7);
            crate::rates::headers(heads, false, &mut headers);
            let mut expiration = None;
            for header in &headers {
                if header.is(b"github-authentication-token-expiration") {
                    expiration = Some(header.value.as_ref());
                }
            }
            match expires {
                Some(_) => assert_eq!(expiration, Some(b"2026-11-19 12:00:00 UTC".as_slice())),
                None => assert_eq!(expiration, None),
            }
        }
        let response = h.app(Method::Get, b"/app", b"");
        assert!(response.rate.is_none());
        let installation = f::bearer(h.mint(1, false).token.expose());
        let response = h.ask(Method::Get, f::REACHABLE, Some(installation.expose()), b"");
        assert_eq!(response.rate.unwrap().expires, None);
    }
}

#[test]
fn github_fake_repository_validator_checks_current_settings_and_304_is_free_at_the_limit() {
    let mut config = f::config();
    config.rate_quota = 2;
    let mut h = f::Harness::with_domain(f::limits(), config, super::read_support::domain_config());
    let token = super::read_support::token(&mut h);
    let target = b"/repos/acme/widgets";
    let first = h.ask(Method::Get, target, Some(token.expose()), b"");
    assert_eq!(first.status, 200);
    let tag = first.etag.unwrap();
    let same = h.ask_validated(Method::Get, target, Some(token.expose()), b"", Some(&tag));
    assert_eq!(same.status, 304);
    assert!(same.body.is_empty());
    assert_eq!(same.rate.unwrap().rate.used, first.rate.unwrap().rate.used);
    skein_fake_forge_domain::settings(
        &mut h.domain,
        b"acme/widgets",
        skein_fake_forge_domain::api::Settings {
            default: f::boxed(b"trunk"),
            merge: false,
            squash: false,
            rebase: true,
        },
    );
    let changed = h.ask_validated(Method::Get, target, Some(token.expose()), b"", Some(&tag));
    assert_eq!(changed.status, 200);
    assert_eq!(changed.rate.unwrap().rate.used, 2);
    let new_tag = changed.etag.unwrap();
    assert_ne!(tag, new_tag);
    let same = h.ask_validated(Method::Get, target, Some(token.expose()), b"", Some(&new_tag));
    assert_eq!(same.status, 304);
    assert_eq!(same.rate.unwrap().rate.used, 2);
    assert_eq!(h.ask_validated(Method::Get, target, Some(token.expose()), b"", Some(&tag)).status, 403);
}
