use super::{
    app,
    fixtures::{self as f, Harness, INSTALLATIONS, REACHABLE, VIEWER, bearer, refusal},
    wall,
};
use crate::{
    installations::{Scope, TOKEN_LIFE},
    keys::KEYS,
};
use skein_forge::github::{
    document::{Document, Kind, Row},
    request::Method,
};
use skein_lib::{Wall, bytes};

#[test]
fn github_fake_cached_jwt_is_refused_after_expiry_and_key_revocation() {
    for revoke in [false, true] {
        let mut h = Harness::new();
        assert_eq!(h.app(Method::Get, b"/app", b"").status, 200);
        assert_eq!(h.app(Method::Get, b"/app", b"").status, 200);
        if revoke {
            h.service.revoke_key();
        } else {
            h.now = wall(1600);
        }
        let response = h.app(Method::Get, b"/app", b"");
        refusal(&h, &response, 401, b"Bad credentials");
    }
}

// Each route runs separately so the RSA checks stay within the step budget.
#[test]
fn github_fake_app_routes_need_the_jwt() {
    app_jwt_route(b"/app", Kind::App);
}
#[test]
fn github_fake_app_hook_needs_the_jwt() {
    app_jwt_route(b"/app/hook/config", Kind::HookConfig);
}
#[test]
fn github_fake_app_installations_need_the_jwt() {
    app_jwt_route(INSTALLATIONS, Kind::InstallationRows);
}
#[test]
fn github_fake_app_installation_needs_the_jwt() {
    app_jwt_route(b"/app/installations/1", Kind::Installation);
}
fn app_jwt_route(target: &[u8], kind: Kind) {
    let mut h = Harness::new();
    let minted = h.mint(1, false);
    let other = f::authorization(KEYS.get(1).expect("second test key"), 1000, 1600);
    let expired = f::authorization(app().key, 500, 1100);
    let early = f::authorization(app().key, 1261, 1600);
    let installation = bearer(minted.token.expose());
    let response = h.app(Method::Get, target, b"");
    assert_eq!(response.status, 200);
    h.decode(kind, &response);
    for auth in [
        None,
        Some(other.expose()),
        Some(expired.expose()),
        Some(early.expose()),
        Some(installation.expose()),
        Some(b"Bearer garbage".as_slice()),
    ] {
        let response = h.ask(Method::Get, target, auth, b"");
        refusal(&h, &response, 401, b"Bad credentials");
    }
    assert_eq!(h.calls, 0);
    assert_eq!(h.hooks, 0);
}

#[test]
fn github_fake_installations_list_pages() {
    let mut h = Harness::new();
    for (target, ids, linked) in [
        (INSTALLATIONS, &[1_u64, 2][..], true),
        (b"/app/installations?per_page=100&page=2", &[3][..], false),
        (b"/app/installations?per_page=100&page=99", &[][..], false),
    ] {
        let response = h.app(Method::Get, target, b"");
        assert_eq!(response.status, 200);
        let (rows, document) = h.decode(Kind::InstallationRows, &response);
        assert_eq!(document, Document::Rows);
        assert_eq!(rows.len(), u32::try_from(ids.len()).expect("fixture count"));
        for (row, id) in rows.iter().zip(ids) {
            let Row::Installation(row) = row else {
                panic!("installation row");
            };
            assert_eq!(row.id, *id);
            assert_eq!(row.suspended, *id == 3);
            assert_eq!(
                row.kind,
                if *id == 1 {
                    skein_forge::github::document::Account::Organization
                } else {
                    skein_forge::github::document::Account::User
                }
            );
            assert_eq!(row.created, wall(999_u64.checked_add(*id).expect("fixture time")));
        }
        assert_eq!(response.link.is_some(), linked);
        if let Some(link) = response.link {
            assert_eq!(link.as_ref(), b"<http://127.0.0.1:3000/app/installations?per_page=100&page=2>; rel=\"next\", <http://127.0.0.1:3000/app/installations?per_page=100&page=2>; rel=\"last\"");
        }
    }
    let response = h.app(Method::Get, b"/app/installations/99", b"");
    refusal(&h, &response, 404, b"Not Found");
}

#[test]
fn github_fake_mint_both_scopes() {
    let mut h = Harness::new();
    for (contents, scope) in [(false, Scope::Full), (true, Scope::Contents)] {
        let minted = h.mint(1, contents);
        assert!(minted.token.expose().starts_with(b"ghs_"), "GitHub token prefix");
        assert_eq!(minted.expires.as_nanos(), h.now.as_nanos().checked_add(TOKEN_LIFE.as_nanos()).expect("expiry"));
        assert_eq!(h.service.installations().token(minted.token.expose(), h.now).expect("mint stored").scope, scope);
    }
    for (target, body, status, message) in [
        (b"/app/installations/1/access_tokens".as_slice(), b"{}".as_slice(), 422, b"Validation Failed".as_slice()),
        (b"/app/installations/99/access_tokens", b"invalid", 404, b"Not Found"),
        (b"/app/installations/3/access_tokens", b"invalid", 403, b"This installation has been suspended"),
    ] {
        let response = h.app(Method::Post, target, body);
        refusal(&h, &response, status, message);
    }
    let token = bearer(h.mint(1, false).token.expose());
    let response = h.ask(Method::Post, b"/app/installations/1/access_tokens", Some(token.expose()), b"");
    refusal(&h, &response, 401, b"Bad credentials");
}

#[test]
fn github_fake_mint_token_works_on_a_later_route() {
    let mut h = Harness::new();
    let token = bearer(h.mint(1, false).token.expose());
    let response = h.ask(Method::Get, REACHABLE, Some(token.expose()), b"");
    assert_eq!(response.status, 200);
    let (rows, _) = h.decode(Kind::ReachableRows, &response);
    assert_eq!(rows.len(), 2);
    assert!(bytes::find(&response.body, br#""total_count":2"#).is_some(), "reachable total");
    let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), VIEWER);
    let (_, Document::Viewer(viewer)) = h.decode(Kind::Viewer, &response) else {
        panic!("viewer");
    };
    assert_eq!(viewer.id, f::BOT);
    assert_eq!(viewer.login.as_ref(), b"test-app[bot]");
    for (method, target, body) in [(Method::Get, REACHABLE, b"".as_slice()), (Method::Post, b"/graphql", VIEWER)] {
        let response = h.app(method, target, body);
        refusal(&h, &response, 401, b"Bad credentials");
    }
    assert_eq!(h.calls, 0);
    assert_eq!(h.hooks, 0);
}

#[test]
fn github_fake_expired_token_is_refused() {
    let mut h = Harness::new();
    let minted = h.mint(1, false);
    let token = bearer(minted.token.expose());
    let before = Wall::from_nanos(minted.expires.as_nanos().checked_sub(1).expect("before expiry"));
    for (now, status) in [
        (before, 200),
        (minted.expires, 401),
        (Wall::from_nanos(minted.expires.as_nanos().checked_add(1).expect("after expiry")), 401),
    ] {
        h.now = now;
        for (method, target, body) in [
            (Method::Get, REACHABLE, b"".as_slice()),
            (Method::Post, b"/graphql", VIEWER),
            (Method::Get, b"/repositories/2", b""),
            (Method::Get, b"/repos/acme/widgets", b""),
            (Method::Get, b"/repos/acme/widgets/branches/trunk", b""),
            (Method::Get, b"/repos/acme/widgets/branches?per_page=100&page=1", b""),
        ] {
            let response = h.ask(method, target, Some(token.expose()), body);
            assert_eq!(response.status, status);
        }
    }
    assert_eq!(h.calls, 4, "expired tokens never reach domain");
    assert_eq!(h.hooks, 0);
}

#[test]
fn github_fake_reachable_pages() {
    let mut limits = f::limits();
    limits.page = 1;
    let mut h = Harness::with(limits, f::config());
    assert!(h.service.installations_mut().remove(2));
    let all = h
        .service
        .installations_mut()
        .install(22, b"acme", crate::installations::Kind::Organization, crate::installations::Reach::All, h.now)
        .unwrap();
    for (id, targets) in [(1, &[2_u64, 4][..]), (all, &[2_u64, 4, 6][..])] {
        let token = bearer(h.mint(id, false).token.expose());
        for (index, expected) in targets.iter().enumerate() {
            let page = u32::try_from(index).expect("fixture page").checked_add(1).expect("page");
            let request = skein_forge::github::request::Request::Reachable { page, per_page: 100 };
            let encoded = skein_forge::github::request::encode(&request, &h.limits.github).expect("target");
            let response = h.ask(Method::Get, &encoded.target, Some(token.expose()), b"");
            let total = if id == 1 { br#""total_count":2"#.as_slice() } else { br#""total_count":3"# };
            assert!(bytes::find(&response.body, total).is_some(), "total spans all pages");
            let (rows, _) = h.decode(Kind::ReachableRows, &response);
            assert_eq!(rows.len(), 1);
            let Row::Reachable(row) = rows.get(0).expect("one row") else {
                panic!("reachable row");
            };
            assert_eq!(row.id, *expected);
            assert_eq!(row.private, *expected == 2);
            assert_eq!(row.owner, 22);
            assert_eq!(row.login.as_ref(), b"acme");
            assert_eq!(response.link.is_some(), index.checked_add(1).expect("next index") < targets.len());
        }
        let response =
            h.ask(Method::Get, b"/installation/repositories?per_page=100&page=99", Some(token.expose()), b"");
        assert!(h.decode(Kind::ReachableRows, &response).0.is_empty(), "past last page empty");
        assert!(response.link.is_none(), "past last has no next");
    }
}

#[test]
fn github_fake_graphql_other_documents() {
    let mut h = Harness::new();
    let token = bearer(h.mint(1, false).token.expose());
    for body in [b"{}".as_slice(), b"not JSON", br#"{"query":"query { other }"}"#] {
        let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), body);
        assert_eq!(response.status, 200);
        let (_, Document::Graphql(error)) = h.decode(Kind::Viewer, &response) else {
            panic!("GraphQL errors");
        };
        assert_eq!(error.kind.as_ref(), b"UNPROCESSABLE");
    }
    let response = h.app(Method::Post, b"/graphql", b"{}");
    refusal(&h, &response, 401, b"Bad credentials");
}

#[test]
fn github_fake_hook_config_reads_and_sets() {
    let mut h = Harness::new();
    let response = h.app(Method::Patch, b"/app/hook/config", br#"{"url":"https://hook.test/x"}"#);
    assert_eq!(response.status, 200);
    let (_, Document::HookConfig(hook)) = h.decode(Kind::HookConfig, &response) else {
        panic!("hook document");
    };
    assert_eq!(hook.url.as_ref(), b"https://hook.test/x");
    let response = h.app(Method::Get, b"/app/hook/config", b"");
    assert_eq!(h.decode(Kind::HookConfig, &response).1, Document::HookConfig(hook));
    for body in [b"{}".as_slice(), br#"{"url":""}"#, b"not JSON", br#"{"url":null}"#] {
        let response = h.app(Method::Patch, b"/app/hook/config", body);
        refusal(&h, &response, 422, b"Validation Failed");
    }
    let token = bearer(h.mint(1, false).token.expose());
    let response = h.ask(Method::Patch, b"/app/hook/config", Some(token.expose()), br#"{"url":"https://hook.test/y"}"#);
    refusal(&h, &response, 401, b"Bad credentials");
    assert_eq!(h.hooks, 0);
}

#[test]
fn github_fake_uninstall_removes() {
    let mut h = Harness::new();
    let token = bearer(h.mint(1, false).token.expose());
    let other = bearer(h.mint(2, false).token.expose());
    let response = h.app(Method::Delete, b"/app/installations/1", b"");
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty(), "uninstall has no body");
    let response = h.app(Method::Get, b"/app/installations/1", b"");
    refusal(&h, &response, 404, b"Not Found");
    let response = h.ask(Method::Get, REACHABLE, Some(token.expose()), b"");
    refusal(&h, &response, 401, b"Bad credentials");
    let response = h.app(Method::Delete, b"/app/installations/1", b"");
    refusal(&h, &response, 404, b"Not Found");
    assert_eq!(h.app(Method::Get, b"/app/installations/2", b"").status, 200);
    assert_eq!(h.ask(Method::Get, REACHABLE, Some(other.expose()), b"").status, 200);
    assert_eq!(h.hooks, 0);
}

#[test]
fn github_fake_installation_routes_need_bearer_tokens() {
    let mut h = Harness::new();
    for (method, target, body) in [
        (Method::Get, REACHABLE, b"".as_slice()),
        (Method::Post, b"/graphql", VIEWER),
        (Method::Get, b"/repositories/2", b""),
        (Method::Get, b"/repos/acme/widgets", b""),
        (Method::Get, b"/repos/acme/widgets/branches/trunk", b""),
        (Method::Get, b"/repos/acme/widgets/branches?per_page=2&page=1", b""),
    ] {
        for auth in [None, Some(b"Bearer ghs_unknown".as_slice()), Some(b"token unknown".as_slice())] {
            let response = h.ask(method, target, auth, body);
            refusal(&h, &response, 401, b"Bad credentials");
        }
        let response = h.app(method, target, body);
        refusal(&h, &response, 401, b"Bad credentials");
    }
    assert_eq!(h.calls, 0);
}

#[test]
fn github_fake_suspension_preserves_but_refuses_tokens() {
    let mut h = Harness::new();
    let token = bearer(h.mint(1, false).token.expose());
    assert!(h.service.installations_mut().suspend(1, true), "known installation");
    for (method, target, body) in [
        (Method::Get, REACHABLE, b"".as_slice()),
        (Method::Post, b"/graphql", VIEWER),
        (Method::Get, b"/repositories/2", b""),
    ] {
        let response = h.ask(method, target, Some(token.expose()), body);
        refusal(&h, &response, 403, b"This installation has been suspended");
    }
    assert!(h.service.installations_mut().suspend(1, false), "known installation");
    assert_eq!(h.ask(Method::Get, REACHABLE, Some(token.expose()), b"").status, 200);
    assert_eq!(h.calls, 0);
}

#[test]
fn github_fake_mint_reports_full_and_narrowed_installation_levels() {
    use skein_forge::github::document::{Access, Level, Levels, Permission};
    let mut config = f::config();
    config.app.permissions = Box::from([
        Permission { name: f::boxed(b"contents"), access: Access::Write },
        Permission { name: f::boxed(b"pull_requests"), access: Access::Write },
    ]);
    let mut h = Harness::with(f::limits(), config);
    let full = h.mint(1, false);
    assert_eq!(full.permissions, Levels { contents: Level::Write, pull_requests: Level::Write });
    let contents = h.mint(1, true);
    assert_eq!(contents.permissions, Levels { contents: Level::Write, pull_requests: Level::None });
}
