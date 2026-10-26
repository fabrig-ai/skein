use super::{
    fixtures::{self as f, Harness, bearer, refusal},
    wall,
};
use crate::{
    installations::{Kind, Reach, Scope},
    service::{self, Outcome, Problem, Request, Service},
};
use skein_forge::github::{self, request::Method};
use skein_lib::{ReplyTo, Token};

#[test]
fn github_fake_heads_are_checked() {
    let mut h = Harness::new();
    for (agent, version, target, expected) in [
        (false, None, b"/app".as_slice(), 403),
        (true, None, b"/app", 400),
        (true, Some(b"2020-01-01".as_slice()), b"/app", 400),
        (true, Some(github::pin::API_VERSION.as_bytes()), b"/user", 401),
        (true, Some(github::pin::API_VERSION.as_bytes()), b"/repos/o/n/pulls", 404),
        (true, Some(github::pin::API_VERSION.as_bytes()), b"/user/1", 401),
    ] {
        let outcome = h.service.request(
            &Request { validator: None, method: Method::Get, target, authorization: None, agent, version, body: b"" },
            h.now,
            ReplyTo::new(Token::new(77)),
        );
        let Outcome::Reply(response) = outcome else {
            panic!("head refusal precedes credential and domain");
        };
        assert_eq!(response.status, expected);
    }
    assert_eq!(h.calls, 0);
}

#[test]
fn github_fake_zero_paging_is_unrouted() {
    let mut h = Harness::new();
    for target in [
        b"/app/installations?per_page=100&page=0".as_slice(),
        b"/installation/repositories?per_page=100&page=0",
        b"/repos/acme/widgets/branches?per_page=0&page=1",
        b"/repos/acme/widgets/branches?per_page=2&page=0",
    ] {
        let response = h.ask(Method::Get, target, None, b"");
        refusal(&h, &response, 404, b"Not Found");
    }
}

#[test]
fn github_fake_worst_case_holds_the_stores() {
    let limits = f::limits();
    assert!(service::worst_case(&limits, &f::config()).is_some(), "valid budgets");
    let mut overflow = limits;
    overflow.installations.installations = u32::MAX;
    overflow.installations.listed = u32::MAX;
    assert_eq!(service::worst_case(&overflow, &f::config()), None);
    let mut h = Harness::new();
    for _token in 0..limits.installations.tokens {
        h.service.installations_mut().mint(1, Scope::Full, wall(1200)).expect("fill token store");
    }
    assert_eq!(
        h.service.installations_mut().install(99, b"full", Kind::User, Reach::All, h.now),
        Err(crate::installations::Install::Full)
    );
    let token = h.service.installations_mut().mint(1, Scope::Full, h.now);
    assert_eq!(token, Err(crate::installations::Mint::Full));
    let response = h.app(Method::Get, b"/app", b"");
    assert_eq!(response.status, 200, "full stores still serve reads");
    let response = h.app(Method::Post, b"/app/installations/1/access_tokens", b"");
    refusal(&h, &response, 503, b"Server Error");
}

#[test]
fn github_fake_config_and_limits_refused_at_admission() {
    for invalid in 0..7_u32 {
        let mut limits = f::limits();
        match invalid {
            0 => limits.page = 0,
            1 => limits.page = 101,
            2 => limits.page = limits.github.inner.checked_add(1).expect("fixture bound"),
            3 => limits.body = limits.github.body_bytes.checked_add(1).expect("fixture bound"),
            4 => limits.response = 127,
            5 => limits.github.json.depth = 1,
            6 => limits.github.string_bytes = 0,
            _ => panic!("fixture case"),
        }
        assert_eq!(Service::new(f::config(), &limits, 1).expect_err("invalid limits"), Problem::Limits);
    }
    for invalid in 0..6_u32 {
        let mut config = f::config();
        match invalid {
            0 => config.repositories.get_mut(0).expect("row").id = 0,
            1 => config.repositories.get_mut(0).expect("row").id = 2,
            2 => config.repositories.get_mut(0).expect("row").name = f::boxed(b"widgets"),
            3 => config.hook = f::boxed(&[b'x'; 257]),
            4 => config.bot = 0,
            5 => config.bot = config.app.id,
            _ => panic!("fixture case"),
        }
        assert_eq!(Service::new(config, &f::limits(), 1).expect_err("invalid config"), Problem::Config);
    }
}

#[test]
fn github_fake_body_and_response_bounds() {
    let mut limits = f::limits();
    limits.body = 8;
    let mut h = Harness::with(limits, f::config());
    let response = h.app(Method::Patch, b"/app/hook/config", br#"{"url":"https://hook.test/x"}"#);
    refusal(&h, &response, 422, b"Validation Failed");
    let response = h.app(Method::Get, b"/app/hook/config", b"");
    assert!(
        skein_lib::bytes::find(&response.body, b"https://hook.test/start").is_some(),
        "refused update leaves hook alone"
    );
    let token = bearer(h.mint(1, false).token.expose());
    let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), f::VIEWER);
    assert_eq!(response.status, 200);
    let (_, document) = h.decode(github::document::Kind::Viewer, &response);
    let github::document::Document::Graphql(_) = document else {
        panic!("oversized query is GraphQL refusal");
    };
    for target in [b"/repositories/2".as_slice(), f::REACHABLE] {
        let response = h.ask(Method::Get, target, Some(token.expose()), b"123456789");
        refusal(&h, &response, 413, b"Payload Too Large");
    }
    let response = h.ask(Method::Get, b"/repositories/999", Some(token.expose()), b"123456789");
    refusal(&h, &response, 404, b"Not Found");
    let response = h.app(Method::Get, b"/app", b"123456789");
    refusal(&h, &response, 413, b"Payload Too Large");
    assert_eq!(h.calls, 0, "oversized bodies are refused before domain calls");
    let mut limits = f::limits();
    limits.response = 192;
    let mut h = Harness::with(limits, f::config());
    let token = bearer(h.mint(1, false).token.expose());
    let response = h.ask(Method::Get, b"/repositories/2", Some(token.expose()), b"");
    refusal(&h, &response, 500, b"Server Error");
    assert!(response.body.len() <= 192, "bounded fallback");
}
