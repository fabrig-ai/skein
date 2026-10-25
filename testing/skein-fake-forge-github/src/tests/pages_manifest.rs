use super::{
    fixtures::{self as f, Harness},
    wall,
};
use crate::{
    pages::Media,
    service::{Problem, Service},
};
use skein_forge::github::{request::Method, setup};
use skein_lib::bytes;

#[test]
fn github_fake_manifest_page_has_one_create_link() {
    let mut h = f::pages();
    for (target, state, account) in [
        (b"/settings/apps/new?state=s1".as_slice(), b"s1".as_slice(), b"alice".as_slice()),
        (b"/organizations/acme/settings/apps/new?state=s2", b"s2", b"acme"),
    ] {
        let answer = h.page(Method::Post, target, &f::form(&f::manifest(f::NAME, f::REDIRECT, f::SETUP)));
        assert_eq!(answer.status, 200);
        assert_eq!(answer.media, Media::Html);
        assert!(answer.location.is_none(), "manifest serves HTML, not a redirect");
        assert_eq!(bytes::count(&answer.body, b"<a", 2), 1);
        let code = f::code(&answer);
        assert_eq!(code.len(), 20);
        for byte in code.expose() {
            assert!(byte.is_ascii_digit() || (b'a'..=b'f').contains(byte), "lowercase hex code");
        }
        assert_eq!(f::link(&answer), f::parts(&[f::REDIRECT, b"?code=", code.expose(), b"&state=", state]));
        assert_eq!(
            answer.body,
            f::parts(&[
                b"<!doctype html><meta charset=\"utf-8\"><title>GitHub App</title><a id=\"create\" href=\"",
                f::REDIRECT,
                b"?code=",
                code.expose(),
                b"&amp;state=",
                state,
                b"\">Create GitHub App</a>",
            ])
        );
        assert_eq!(h.service.manifest(state).expect("manifest record").account.as_ref(), account);
    }
    assert_eq!((h.calls, h.hooks), (0, 0));
}

#[test]
fn github_fake_manifest_is_kept_by_state() {
    let mut h = f::pages();
    let first = f::create(&mut h, b"s1", f::NAME);
    let manifest = h.service.manifest(b"s1").expect("state record");
    assert_eq!(manifest.name.as_ref(), f::NAME);
    assert_eq!(manifest.url.as_ref(), b"https://setup.test/");
    assert!(manifest.public && manifest.setup_on_update, "manifest flags retained");
    assert_eq!(manifest.redirect_url.as_ref(), f::REDIRECT);
    assert_eq!(manifest.setup_url.as_deref(), Some(f::SETUP));
    let hook = manifest.hook_attributes.as_ref().expect("hook attributes");
    assert_eq!(hook.url.as_deref(), Some(b"https://hook.test/app".as_slice()));
    assert!(hook.active, "hook active retained");
    assert_eq!(manifest.default_permissions.len(), u32::try_from(setup::PERMISSIONS.len()).expect("permission count"));
    for (permission, (name, level)) in manifest.default_permissions.iter().zip(setup::PERMISSIONS) {
        assert_eq!(permission.name.as_ref(), name);
        assert_eq!(permission.access, level);
    }
    assert_eq!(manifest.default_events.len(), u32::try_from(setup::EVENTS.len()).expect("event count"));
    for (event, expected) in manifest.default_events.iter().zip(setup::EVENTS) {
        assert_eq!(event.as_ref(), expected);
    }
    assert!(h.service.manifest(b"zz").is_none(), "unknown state absent");
    let second = f::create(&mut h, b"s1", b"example-app");
    assert!(f::code(&first) != f::code(&second), "repost issues a new code");
    assert_eq!(h.service.manifest(b"s1").expect("replacement").name.as_ref(), b"example-app");
    let response = f::convert(&mut h, &f::code(&first));
    f::refusal(&h, &response, 404, b"Not Found");
}

#[test]
fn github_fake_manifest_is_refused_as_github_does() {
    let mut h = f::pages();
    for input in [
        b"not JSON".as_slice(), b"[]", b"null", b"{}",
        br#"{"name":"example-app","url":"https://setup.test/"}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"/convert"}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"https://setup.test/x?q"}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"https://setup.test/x#f"}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"https:///x"}"#,
        br#"{"name":"example-app","name":"example-app","url":"https://setup.test/","redirect_url":"https://setup.test/x"}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"https://setup.test/x","other":{"a":1,"a":2}}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"https://setup.test/x","setup_url":"/install"}"#,
        br#"{"name":false,"url":"https://setup.test/","redirect_url":"https://setup.test/x"}"#,
        br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"https://setup.test/x","default_events":{}}"#,
    ] {
        let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(input));
        f::page_refusal(&h, answer, 422, b"Validation Failed");
    }
    let body = f::form(&f::manifest(f::NAME, f::REDIRECT, f::SETUP));
    for target in [
        b"/organizations/alice/settings/apps/new?state=s1".as_slice(),
        b"/organizations/unknown/settings/apps/new?state=s1",
        b"/settings/apps/new",
        b"/settings/apps/new?state=",
        b"/settings/apps/new?state=s/1",
        b"/settings/apps/new?state=%61",
        b"/settings/apps/new?state=s1&state=s1",
    ] {
        let answer = h.page(Method::Post, target, &body);
        f::page_refusal(&h, answer, 422, b"Validation Failed");
    }
    for body in [b"manifest=%".as_slice(), b"manifest=%0", b"manifest=%gg", b"other=1", b"manifest={}&manifest={}"] {
        let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", body);
        f::page_refusal(&h, answer, 422, b"Validation Failed");
    }
    assert!(h.service.manifest(b"s1").is_none(), "no refused manifest retained");
    assert_eq!((h.calls, h.hooks), (0, 0));
}

#[test]
fn github_fake_manifest_bounds_are_checked() {
    let input = f::manifest(f::NAME, f::REDIRECT, f::SETUP);
    let body = f::form(&input);
    let string = f::manifest(&[b'a'; 256], f::REDIRECT, f::SETUP);
    let long_string = f::manifest(&[b'a'; 257], f::REDIRECT, f::SETUP);
    for (manifest, manifest_bytes, inner, body_bytes, status) in [
        (
            input.as_ref(),
            u32::try_from(input.len()).expect("manifest length"),
            14,
            u32::try_from(body.len()).expect("form length"),
            200,
        ),
        (
            input.as_ref(),
            u32::try_from(input.len().checked_sub(1).expect("manifest length")).expect("manifest bound"),
            14,
            4096,
            422,
        ),
        (input.as_ref(), 2048, 13, 4096, 422),
        (
            input.as_ref(),
            2048,
            14,
            u32::try_from(body.len().checked_sub(1).expect("form length")).expect("form bound"),
            413,
        ),
        (string.as_ref(), 2048, 14, 4096, 200),
        (long_string.as_ref(), 2048, 14, 4096, 422),
    ] {
        let mut limits = f::page_limits();
        limits.pages.manifest_bytes = manifest_bytes;
        limits.github.inner = inner;
        limits.body = body_bytes;
        let mut h = Harness::with(limits, f::config());
        let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(manifest));
        assert_eq!(answer.status, status);
        assert_eq!(h.service.manifest(b"s1").is_some(), status == 200);
    }
    let mut limits = f::page_limits();
    limits.github.inner = 1;
    // Keep the scenario's own account and repository tables inside the reduced bound.
    let mut config = f::config();
    config.accounts.truncate(1);
    config.person = f::boxed(b"acme");
    config.repositories.truncate(1);
    limits.page = 1;
    let mut h = Harness::with(limits, config);
    let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"http://setup.test/x","default_permissions":{"contents":"read","issues":"write"}}"#));
    f::page_refusal(&h, answer, 422, b"Validation Failed");
}

#[test]
fn github_fake_manifest_states_store_is_bounded() {
    let mut h = f::pages();
    let first = f::create(&mut h, b"s1", f::NAME);
    let second = f::create(&mut h, b"s2", f::NAME);
    assert_eq!(f::create(&mut h, b"s3", f::NAME).status, 200);
    let answer = f::create(&mut h, b"s4", f::NAME);
    f::page_refusal(&h, answer, 503, b"Server Error");
    let replacement = f::create(&mut h, b"s1", b"example-app");
    assert_eq!(replacement.status, 200);
    assert!(f::code(&first) != f::code(&replacement), "full store admits repost");
    assert_eq!(f::convert(&mut h, &f::code(&second)).status, 201);
    assert!(h.service.manifest(b"s2").is_some(), "used manifest kept before pressure");
    assert_eq!(f::create(&mut h, b"s4", f::NAME).status, 200);
    assert!(h.service.manifest(b"s2").is_none(), "used record reclaimed under pressure");
    h.now = wall(4800);
    assert_eq!(f::create(&mut h, b"s5", f::NAME).status, 200);
    assert!(h.service.manifest(b"s1").is_none(), "expired record reclaimed under pressure");
}

#[test]
fn github_fake_manifest_form_escaping_and_page_routes() {
    let mut h = f::pages();
    let redirect = b"https://setup.test/a&<>\"'";
    let answer = f::create(&mut h, b"s1", f::NAME);
    assert_eq!(answer.status, 200);
    let answer =
        h.page(Method::Post, b"/settings/apps/new?state=._~-AZ09", &f::form(&f::manifest(f::NAME, redirect, f::SETUP)));
    assert_eq!(answer.status, 200);
    assert!(
        bytes::find(&answer.body, b"a&amp;&lt;&gt;&quot;&#39;?code=").is_some(),
        "all attribute metacharacters escaped"
    );
    assert!(f::link(&answer).starts_with(redirect), "attribute round trip");
    for (method, target) in [
        (Method::Get, b"/settings/apps/new?state=s1".as_slice()),
        (Method::Post, b"/apps/example-app/installations/new?state=s1"),
        (Method::Get, b"/unknown"),
        (Method::Post, b"/organizations/acme/other/settings/apps/new?state=s1"),
    ] {
        let answer = h.page(method, target, b"");
        f::page_refusal(&h, answer, 404, b"Not Found");
    }
}

#[test]
fn github_fake_manifest_optional_fields_and_unknown_keys() {
    let mut h = f::pages();
    let input = br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"http://setup.test/convert","extra":[{"name":1},{"name":2}],"hook_attributes":{"active":false,"extra":{"active":true}}}"#;
    let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(input));
    assert_eq!(answer.status, 200);
    let manifest = h.service.manifest(b"s1").expect("minimal manifest");
    assert!(!manifest.public && !manifest.setup_on_update, "absent flags default false");
    assert!(manifest.setup_url.is_none(), "setup URL optional");
    assert!(manifest.default_events.is_empty(), "absent events empty");
    assert!(manifest.default_permissions.is_empty(), "absent permissions empty");
    let hook = manifest.hook_attributes.as_ref().expect("hook attributes");
    assert!(!hook.active && hook.url.is_none(), "hook fields retained independently");
    let response = f::convert(&mut h, &f::code(&answer));
    assert_eq!(response.status, 201);
    assert_eq!(h.service.hook.as_ref(), b"https://hook.test/start");
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1"]), b"");
    f::page_refusal(&h, answer, 422, b"Validation Failed");
    assert_eq!(h.service.installations().iter().count(), 0, "refused install leaves the new App uninstalled");
}

#[test]
fn github_fake_pages_config_is_validated() {
    for field in 0..6_u32 {
        let mut config = f::config();
        match field {
            0 => config.accounts.get_mut(0).expect("account").id = 0,
            1 => config.accounts.get_mut(0).expect("account").id = 31,
            2 => config.accounts.get_mut(0).expect("account").login = f::boxed(b"alice"),
            3 => config.person = f::boxed(b"unknown"),
            4 => config.accounts.get_mut(0).expect("account").login = f::boxed(b""),
            5 => config.accounts.get_mut(0).expect("account").login = f::boxed(&[b'a'; 257]),
            _ => panic!("fixture field"),
        }
        assert_eq!(Service::new(config, &f::page_limits(), 5).expect_err("invalid account config"), Problem::Config);
    }
    for (states, manifest_bytes) in [(0, 2048), (3, 0)] {
        let mut limits = f::page_limits();
        limits.pages.states = states;
        limits.pages.manifest_bytes = manifest_bytes;
        assert_eq!(Service::new(f::config(), &limits, 5).expect_err("zero bound"), Problem::Limits);
    }
}

#[test]
fn github_fake_manifest_uses_the_dialects_access_vocabulary() {
    let mut h = f::pages();
    let answer = h.page(Method::Post, b"/settings/apps/new?state=s1", &f::form(br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"http://setup.test/x","default_permissions":{"extra":"admin"}}"#));
    assert_eq!(answer.status, 200);
    let permission = h.service.manifest(b"s1").expect("manifest").default_permissions.get(0).expect("permission");
    assert_eq!(permission.access, setup::Access::Admin);
    let answer = h.page(Method::Post, b"/settings/apps/new?state=s2", &f::form(br#"{"name":"example-app","url":"https://setup.test/","redirect_url":"http://setup.test/x","default_permissions":{"extra":"owner"}}"#));
    assert_eq!(answer.status, 422);
    assert!(h.service.manifest(b"s2").is_none());
}
