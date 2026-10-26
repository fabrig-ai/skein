use super::{fixtures as f, read_support as r};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api,
    github::{
        document::{Document, Kind},
        plans::start::{Identity, Step},
        request::{Method, Request},
    },
};

#[test]
fn github_fake_viewer_drives_the_identity_plan_and_user_resolves_the_same_bot() {
    let mut h = r::harness();
    let token = r::token(&mut h);
    let mut plan = Identity::new();
    let Step::Send(exchange) = plan.begin(r::bounds()).unwrap() else { panic!("identity sends") };
    let mut result = None;
    for event in r::exchange_events(&mut h, &exchange, &token).into_boxed() {
        result = plan.heard(event, r::bounds()).unwrap();
    }
    let Some(Step::Done(Ok(viewer))) = result else { panic!("viewer answer") };
    assert_eq!(viewer.id, f::BOT);
    assert_eq!(viewer.login.as_ref(), b"test-app[bot]");
    let response = h.ask(Method::Get, b"/user/9", Some(token.expose()), b"");
    let (_, Document::User(user)) = h.decode(Kind::User, &response) else { panic!("bot user") };
    assert_eq!((user.id, user.login), (viewer.id, viewer.login));
    assert_eq!(h.calls, 0);
    let response =
        h.ask(Method::Post, b"/graphql", Some(token.expose()), br#"{"query":"query { viewer { databaseId login } }"}"#);
    assert!(matches_graphql(&h, &response));
}
fn matches_graphql(h: &f::Harness, response: &crate::service::Response) -> bool {
    assert_eq!(response.status, 200);
    let (_, Document::Graphql(error)) = h.decode(Kind::Viewer, response) else { panic!("unsupported document errors") };
    error.kind.as_ref() == b"UNPROCESSABLE"
}
#[test]
fn github_fake_people_plans_resolve_users_permissions_and_collaborator_pages() {
    let mut h = r::harness();
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Admin);
    let token = r::token(&mut h);
    assert_eq!(
        r::read(&mut h, &token, api::Read::Permission { user: 31 }),
        Ok(api::Answer::Permission(api::Permission::Admin))
    );
    assert_eq!(
        r::read(&mut h, &token, api::Read::Permission { user: 22 }),
        Ok(api::Answer::Permission(api::Permission::None))
    );
    assert_eq!(r::read(&mut h, &token, api::Read::Permission { user: 999 }), Err(api::Error::Missing));
    for (page, count, more) in [(1, 2, true), (2, 1, false)] {
        let api::Answer::Collaborators { collaborators: users, more: actual } =
            r::read(&mut h, &token, api::Read::Collaborators { page }).unwrap()
        else {
            panic!("collaborators")
        };
        assert_eq!(users.len(), count);
        assert_eq!(actual, more);
        if page == 2 {
            assert_eq!(users.first().unwrap().user, 31);
            assert_eq!(users.first().unwrap().permission, api::Permission::Admin);
        }
    }
    assert_eq!(h.hooks, 0);
}
#[test]
fn github_fake_people_scope_and_all_permissions() {
    let mut h = r::harness();
    let token = r::token(&mut h);
    for permission in [a::Permission::None, a::Permission::Read, a::Permission::Write, a::Permission::Admin] {
        fake::grant(&mut h.domain, b"acme/widgets", 31, permission);
        let api::Answer::Permission(actual) = r::read(&mut h, &token, api::Read::Permission { user: 31 }).unwrap()
        else {
            panic!("permission")
        };
        let expected = match permission {
            a::Permission::None => api::Permission::None,
            a::Permission::Read => api::Permission::Read,
            a::Permission::Write => api::Permission::Write,
            a::Permission::Admin => api::Permission::Admin,
        };
        assert_eq!(actual, expected);
    }
    for request in [
        Request::Collaborators { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), page: 1, per_page: 100 },
        Request::Permission { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), login: f::boxed(b"alice") },
    ] {
        let calls = h.calls;
        let encoded = skein_forge::github::request::encode(&request, &h.limits.github).unwrap();
        let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), b"");
        f::refusal(&h, &response, 404, b"Not Found");
        assert_eq!(h.calls, calls);
    }
}

#[test]
fn github_fake_bot_collaborator_permission_is_not_a_role() {
    let mut h = r::harness();
    let token = r::token(&mut h);
    let calls = h.calls;
    let response = h.ask(
        Method::Get,
        b"/repos/acme/widgets/collaborators/test-app%5Bbot%5D/permission",
        Some(token.expose()),
        b"",
    );
    assert_eq!(response.status, 200);
    assert_eq!(response.body.as_ref(), br#"{"permission":"none","role_name":""}"#);
    assert_eq!(h.decode(Kind::Permission, &response).1, Document::Permission(api::Permission::None));
    assert_eq!(h.calls, calls);
}

#[test]
fn github_fake_selected_installation_refuses_locate_before_permission() {
    let mut h = r::harness();
    let token = r::token(&mut h);
    for target in [b"/repos/acme/absent".as_slice(), b"/repos/acme/absent/collaborators/test-app%5Bbot%5D/permission"] {
        let response = h.ask(Method::Get, target, Some(token.expose()), b"");
        f::refusal(&h, &response, 404, b"Not Found");
    }
    assert_eq!(h.calls, 0);
}
