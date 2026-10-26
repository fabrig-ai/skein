use super::{fixtures as f, wall};
use crate::{
    keys,
    pages::Page,
    routes,
    service::{Outcome, Request},
};
use skein_forge::github::{
    self,
    document::{Document, Kind},
    request::Method,
};
use skein_lib::{ReplyTo, Secret, Token, base64, bytes};

#[test]
fn github_fake_conversion_hands_out_the_app() {
    let mut h = f::pages();
    for (index, expected_id) in [(0_usize, 8), (1, 10), (2, 11), (0, 12)] {
        let answer = f::create(&mut h, b"s1", f::NAME);
        let response = f::convert(&mut h, &f::code(&answer));
        assert_eq!(response.status, 201);
        let row = f::conversion(&h, &response);
        assert_eq!(row.id, expected_id);
        assert_eq!(row.slug.as_ref(), b"example-app");
        assert_eq!((row.owner.as_ref(), row.owner_id), (b"alice".as_slice(), 31));
        assert_eq!(row.webhook_secret.len(), 32);
        for byte in row.webhook_secret.expose() {
            assert!(byte.is_ascii_digit() || (b'a'..=b'f').contains(byte), "webhook hex");
        }
        let pem = row.pem.expose();
        let encoded = pem
            .strip_prefix(b"-----BEGIN RSA PRIVATE KEY-----\n")
            .expect("RSA PEM")
            .strip_suffix(b"-----END RSA PRIVATE KEY-----\n")
            .expect("RSA PEM end");
        let mut compact = skein_lib::List::with_capacity(u32::try_from(encoded.len()).expect("PEM size"));
        for byte in encoded {
            if *byte != b'\n' {
                compact.push(*byte).expect("base64 size");
            }
        }
        let der = base64::decode(base64::Alphabet::Standard, &compact.into_boxed(), 2048).expect("PEM base64");
        assert!(der.as_ref() == *keys::KEYS.get(index).expect("rotating key"), "conversion gives the fixed key");
        assert!(bytes::find(&response.body, b"client_id").is_none(), "OAuth pair absent from dialect document");
        assert!(bytes::find(&response.body, b"client_secret").is_none(), "OAuth pair absent from dialect document");
    }
}

#[test]
fn github_fake_conversion_makes_the_app_the_services() {
    let mut h = f::pages();
    let old = h.jwt.clone();
    let answer = f::create(&mut h, b"s1", f::NAME);
    let response = f::convert(&mut h, &f::code(&answer));
    assert_eq!(response.status, 201);
    h.jwt = f::authorization_for(8, keys::KEYS.first().expect("first key"), 1000, 1600);
    let response = h.app(Method::Get, b"/app", b"");
    assert_eq!(response.status, 200);
    let (_, Document::App(app)) = h.decode(Kind::App, &response) else {
        panic!("App document");
    };
    assert_eq!((app.id, app.slug.as_ref()), (8, b"example-app".as_slice()));
    let response = h.ask(Method::Get, b"/app", Some(old.expose()), b"");
    f::refusal(&h, &response, 401, b"Bad credentials");
    assert_eq!(h.service.installations().iter().count(), 0, "new App has no installations");
    let response = h.app(Method::Get, b"/app/hook/config", b"");
    let (_, Document::HookConfig(hook)) = h.decode(Kind::HookConfig, &response) else {
        panic!("hook document");
    };
    assert_eq!(hook.url.as_ref(), b"https://hook.test/app");
    assert_eq!((h.calls, h.hooks), (0, 0));
}

#[test]
fn github_fake_conversion_code_serves_once() {
    let mut h = f::pages();
    let answer = f::create(&mut h, b"s1", f::NAME);
    let code = f::code(&answer);
    assert_eq!(f::convert(&mut h, &code).status, 201);
    for code in [&code, &Secret::new(f::boxed(b"00000000000000000000"))] {
        let response = f::convert(&mut h, code);
        f::refusal(&h, &response, 404, b"Not Found");
    }
    assert_eq!(h.service.manifest(b"s1").expect("manifest after use").name.as_ref(), f::NAME);
}

#[test]
fn github_fake_conversion_code_lives_an_hour() {
    let mut h = f::pages();
    let first = f::create(&mut h, b"s1", f::NAME);
    let second = f::create(&mut h, b"s2", f::NAME);
    h.now = wall(4799);
    assert_eq!(f::convert(&mut h, &f::code(&first)).status, 201);
    h.now = wall(4800);
    let response = f::convert(&mut h, &f::code(&second));
    f::refusal(&h, &response, 404, b"Not Found");
}

#[test]
fn github_fake_conversion_is_an_api_route_without_a_credential() {
    let mut h = f::pages();
    let answer = f::create(&mut h, b"s1", b"!!!");
    let target = f::conversion_target(&f::code(&answer));
    for (agent, version, status, message) in [
        (false, Some(github::pin::API_VERSION.as_bytes()), 403, b"User agent required".as_slice()),
        (true, Some(b"wrong".as_slice()), 400, b"Invalid API version"),
    ] {
        let outcome = h.service.request(
            &Request {
                validator: None,
                method: Method::Post,
                target: &target,
                authorization: None,
                version,
                agent,
                body: b"",
            },
            h.now,
            ReplyTo::new(Token::new(77)),
        );
        let Outcome::Reply(response) = outcome else {
            panic!("conversion never calls domain");
        };
        f::refusal(&h, &response, status, message);
    }
    let body =
        bytes::zeroed(usize::try_from(h.limits.body.checked_add(1).expect("body past bound")).expect("body size"));
    let response = h.ask(Method::Post, &target, None, &body);
    f::refusal(&h, &response, 413, b"Payload Too Large");
    for _retry in 0..2_u32 {
        let response = h.ask(Method::Post, &target, None, b"ignored body");
        f::refusal(&h, &response, 422, b"Validation Failed");
    }
    assert_eq!(h.service.app.id, 7, "invalid slug does not consume code or replace App");
    let answer = f::create(&mut h, b"s1", b"--EXAMPLE__ App!!--");
    let response = f::convert(&mut h, &f::code(&answer));
    assert_eq!(response.status, 201);
    assert_eq!(f::conversion(&h, &response).slug.as_ref(), b"example-app");
}

#[test]
fn github_fake_conversion_response_bound_preserves_the_app() {
    let mut h = f::pages();
    let answer = f::create(&mut h, b"s1", f::NAME);
    let code = f::code(&answer);
    h.service.limits.response = 128;
    let failed = f::create(&mut h, b"s1", b"Replacement");
    assert_eq!(failed.status, 500);
    assert_eq!(h.service.manifest(b"s1").expect("original manifest").name.as_ref(), f::NAME);
    assert_eq!(f::convert(&mut h, &code).status, 500);
    assert_eq!(h.service.app.id, 7, "failed writer leaves the App alone");
    assert_eq!(h.service.hook.as_ref(), b"https://hook.test/start");
    h.service.limits.response = h.limits.response;
    let response = f::convert(&mut h, &code);
    assert_eq!(response.status, 201, "failed writer does not consume the code");
    let row = f::conversion(&h, &response);
    assert_eq!(row.id, 8);
    assert_eq!(h.service.app.key, *keys::KEYS.first().expect("first key"), "failed writer keeps key rotation");
}

#[test]
fn github_fake_conversion_secrets_stay_out_of_debug() {
    let mut h = f::pages();
    let answer = f::create(&mut h, b"s1", f::NAME);
    let code = f::code(&answer);
    let target = f::conversion_target(&code);
    let route = routes::decode(Method::Post, &target, &h.limits.github).expect("conversion route");
    let request = Request {
        validator: None,
        method: Method::Post,
        target: &target,
        authorization: None,
        version: None,
        agent: false,
        body: code.expose(),
    };
    let page = Page { method: Method::Post, target: &target, body: code.expose() };
    let response = f::convert(&mut h, &code);
    let conversion = f::conversion(&h, &response);
    let record = h.service.pages.states.get(b"s1".as_slice()).expect("record");
    let debug = alloc::fmt::format(format_args!(
        "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
        h.service, h.service.app, record, record.manifest, page, answer, request, response, route
    ));
    for secret in [code.expose(), conversion.pem.expose(), conversion.webhook_secret.expose(), b"BEGIN".as_slice()] {
        assert!(bytes::find(debug.as_bytes(), secret).is_none(), "Debug excludes secret text");
        let array = alloc::fmt::format(format_args!("{secret:?}"));
        assert!(!debug.contains(array.as_str()), "Debug excludes secret bytes");
    }
}

#[test]
fn github_fake_converted_slug_reads_the_bot_without_a_credential() {
    let mut h = f::pages();
    for state in [b"s1", b"s2", b"s3"] {
        let answer = f::create(&mut h, state, f::NAME);
        let response = f::convert(&mut h, &f::code(&answer));
        let app = f::conversion(&h, &response);
        let request = github::request::Request::BotUser { slug: app.slug };
        let target = github::request::encode(&request, &h.limits.github).expect("bot request").target;
        for _read in 0..2_u32 {
            let response = h.ask(Method::Get, &target, None, b"");
            assert_eq!(response.status, 200);
            assert!(bytes::find(&response.body, br#""login":"example-app[bot]""#).is_some());
            assert!(bytes::find(&response.body, br#""type":"Bot""#).is_some());
            let (_, Document::BotUser(bot)) = h.decode(Kind::BotUser, &response) else {
                panic!("bot document");
            };
            assert_eq!(bot, f::BOT);
            assert_ne!(bot, app.id);
        }
        let response = h.ask(Method::Get, b"/users/absent%5Bbot%5D", None, b"");
        f::refusal(&h, &response, 404, b"Not Found");
    }
    assert_eq!((h.calls, h.hooks), (0, 0));
}

#[test]
fn github_fake_new_app_does_not_inherit_the_seeded_apps_installation() {
    let mut h = f::pages();
    let token = h.mint(1, false);
    let input = f::manifest(f::NAME, f::REDIRECT, f::SETUP);
    let created = h.page(Method::Post, b"/organizations/acme/settings/apps/new?state=new-app", &f::form(&input));
    assert_eq!(created.status, 200);
    let converted = f::convert(&mut h, &f::code(&created));
    assert_eq!(converted.status, 201);
    let installed = h.page(Method::Get, &f::parts(&[f::INSTALL, b"new-app"]), b"");
    assert_eq!(installed.status, 302);
    assert_eq!(
        installed.location.as_deref(),
        Some(b"https://setup.test/install?installation_id=4&setup_action=install&state=new-app".as_slice())
    );
    assert_eq!(h.service.installations().iter().count(), 1);
    assert!(h.service.installations().get(1).is_none());
    assert_eq!(
        h.service.installations().token(token.token.expose(), h.now),
        Err(crate::installations::Denied::Unknown)
    );
}
