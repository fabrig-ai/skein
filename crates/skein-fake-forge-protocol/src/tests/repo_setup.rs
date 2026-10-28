use super::fixtures::{self, Harness, boxed};
use crate::{Config, Repository, setup::Store};
use skein_forge::forgejo::{
    document::{Document, Row},
    request::{Grant, HookSpec, Page, Repo, Request},
};
use skein_lib::{List, Secret};
const REPOSITORY: u32 = 2;
const OTHER: u32 = 3;
pub(super) fn repo() -> Repo {
    Repo { owner: boxed(b"acme"), name: boxed(b"widgets") }
}
pub(super) fn other() -> Repo {
    Repo { owner: boxed(b"bob"), name: boxed(b"widgets") }
}
fn config() -> Config {
    let mut config = fixtures::config();
    config.repositories = List::with_capacity(2);
    config.repositories.push(Repository { id: REPOSITORY, full: boxed(b"acme/widgets"), protected: None }).unwrap();
    config.repositories.push(Repository { id: OTHER, full: boxed(b"bob/widgets"), protected: None }).unwrap();
    config.users = List::with_capacity(2);
    config.users.push((1, boxed(b"alice"))).unwrap();
    config.users.push((2, boxed(b"bob"))).unwrap();
    config
}
pub(super) fn harness() -> Harness {
    let mut h = Harness::new(config());
    let admin = h.service.setup_mut().account(b"alice", true, b"");
    h.service.setup_mut().account(b"bob", false, b"");
    h.service.setup_mut().token(admin, b"widgets", &[b"all"], Secret::new(boxed(fixtures::TOKEN)));
    h
}
fn collaborator(permission: Grant) -> Request {
    Request::AddCollaborator { at: repo(), login: boxed(b"alice"), permission }
}
fn spec() -> HookSpec {
    HookSpec { url: boxed(b"https://hooks.example/"), ..fixtures::hook_spec() }
}
fn create() -> Request {
    Request::CreateRepoHook { at: repo(), hook: spec() }
}
fn list(page: u32) -> Request {
    Request::RepoHooks { at: repo(), page: Page { page: Some(page), limit: Some(2) } }
}
fn ids(h: &Harness) -> List<u64> {
    let mut ids = List::with_capacity(h.env.limits.documents.inner);
    for hook in h.service.setup().repository_hooks(REPOSITORY) {
        ids.push(hook.id).unwrap();
    }
    ids
}
#[test]
fn fake_forgejo_collaborator_put_stores_the_permission() {
    let mut h = harness();
    h.send(&collaborator(Grant::Write));
    assert_eq!(h.status(), 204);
    assert!(h.body().is_empty());
    assert_eq!(h.service.setup().collaborator(REPOSITORY, b"alice"), Some(Grant::Write));
    assert_eq!(h.calls, 0);
    assert_eq!(h.service.tally().routes.get(&crate::Route::AddCollaborator), Some(&1));
    // Scenario-configured users need no setup account.
    h.service.setup_mut().delete_account(b"bob");
    h.send(&Request::AddCollaborator { at: repo(), login: boxed(b"bob"), permission: Grant::Read });
    assert_eq!(h.status(), 204);
}
#[test]
fn fake_forgejo_collaborator_put_again_replaces_the_permission() {
    let mut h = harness();
    h.send(&collaborator(Grant::Read));
    h.send(&collaborator(Grant::Admin));
    assert_eq!(h.status(), 204);
    assert_eq!(h.service.setup().collaborator(REPOSITORY, b"alice"), Some(Grant::Admin));
}
#[test]
fn fake_forgejo_collaborator_unknown_repository_or_user_is_404() {
    let mut h = harness();
    for request in [
        Request::AddCollaborator {
            at: Repo { owner: boxed(b"alice"), ..repo() },
            login: boxed(b"alice"),
            permission: Grant::Write,
        },
        Request::AddCollaborator { at: repo(), login: boxed(b"acme"), permission: Grant::Write },
    ] {
        h.send(&request);
        assert_eq!(h.status(), 404);
    }
    assert_eq!(h.service.setup().collaborator(REPOSITORY, b"alice"), None);
}
#[test]
fn fake_forgejo_collaborator_put_by_a_plain_user_is_403() {
    let mut h = harness();
    h.service.setup_mut().token(2, b"widgets", &[b"all"], Secret::new(boxed(b"example-plain-user")));
    let encoded = skein_forge::forgejo::client::encode(&collaborator(Grant::Write), &h.env.limits.documents).unwrap();
    h.ask_as(b"PUT", &encoded.target, encoded.body.as_deref().unwrap(), b"example-plain-user");
    assert_eq!(h.status(), 403);
    assert_eq!(h.service.setup().collaborator(REPOSITORY, b"alice"), None);
    assert!(skein_lib::bytes::find(&h.body(), b"user should be the site admin").is_some());
    let hook_body = skein_forge::forgejo::client::encode(&create(), &h.env.limits.documents).unwrap().body.unwrap();
    for (method, target, body) in [
        (b"GET".as_slice(), b"/api/v1/repos/acme/widgets/hooks".as_slice(), b"".as_slice()),
        (b"POST", b"/api/v1/repos/acme/widgets/hooks", hook_body.as_ref()),
        (b"PATCH", b"/api/v1/repos/acme/widgets/hooks/1", hook_body.as_ref()),
        (b"DELETE", b"/api/v1/repos/acme/widgets/hooks/1", b""),
    ] {
        h.ask_as(method, target, body, b"example-plain-user");
        assert_eq!(h.status(), 403);
    }
    assert_eq!(h.service.setup().repository_hooks(REPOSITORY).count(), 0);
}
#[test]
fn fake_forgejo_hook_create_stores_url_events_active_and_secret() {
    let mut h = harness();
    let request = create();
    h.send(&request);
    assert_eq!(h.status(), 201);
    let row = h.service.setup().repository_hooks(REPOSITORY).next().unwrap();
    let spec = spec();
    assert_eq!(row.url, spec.url);
    assert_eq!(row.events, spec.events);
    assert_eq!(row.active, spec.active);
    assert!(row.secret == spec.secret);
    assert_eq!(
        h.decoded(&request).document,
        Document::Hook(skein_forge::forgejo::document::Hook {
            id: 1,
            url: spec.url,
            events: spec.events,
            active: spec.active
        })
    );
    assert_eq!(h.calls, 0);
}
#[test]
fn fake_forgejo_hook_list_pages_in_id_order() {
    let mut h = harness();
    for _ in 0_u32..3 {
        h.send(&create());
        assert_eq!(h.status(), 201);
    }
    for (page, expected) in [(1, &[1, 2][..]), (2, &[3]), (3, &[]), (u32::MAX, &[])] {
        let request = list(page);
        h.send(&request);
        assert_eq!(h.status(), 200);
        let rows = h.decoded(&request).rows;
        let mut got = List::with_capacity(2);
        for row in &rows {
            let Row::Hook(hook) = row else { panic!("hook row") };
            got.push(hook.id).unwrap();
        }
        assert_eq!(got.as_slice(), expected);
    }
}
#[test]
fn fake_forgejo_hook_never_shows_its_secret() {
    let mut h = harness();
    for request in [create(), list(1), Request::EditRepoHook { at: repo(), id: 1, hook: spec() }] {
        h.send(&request);
        assert!(h.status() < 300);
        let body = h.body();
        assert!(skein_lib::bytes::find(&body, b"secret").is_none());
        assert!(skein_lib::bytes::find(&body, spec().secret.expose()).is_none());
    }
}
#[test]
fn fake_forgejo_hook_of_one_url_can_be_made_twice() {
    let mut h = harness();
    h.send(&create());
    h.send(&create());
    assert_eq!(ids(&h).as_slice(), &[1, 2]);
    for hook in h.service.setup().repository_hooks(REPOSITORY) {
        assert_eq!(hook.url, spec().url);
    }
}
#[test]
fn fake_forgejo_hook_edit_and_delete_leave_the_others() {
    let mut h = harness();
    h.send(&create());
    h.send(&create());
    h.send(&Request::CreateRepoHook { at: other(), hook: spec() });
    let changed = HookSpec {
        active: false,
        events: Box::new([]),
        url: boxed(b"https://hooks.example/other"),
        secret: Secret::new(boxed(b"example-new-hook")),
    };
    h.send(&Request::EditRepoHook { at: repo(), id: 1, hook: changed.clone() });
    assert_eq!(h.status(), 200);
    let hook = h.service.setup().repository_hooks(REPOSITORY).next().unwrap();
    assert_eq!(hook.url, changed.url);
    assert!(!hook.active);
    assert!(hook.events.is_empty());
    assert!(hook.secret == changed.secret);
    assert_eq!(h.service.setup().repository_hooks(REPOSITORY).nth(1).unwrap().url, spec().url);
    for at in [repo(), other(), Repo { owner: boxed(b"alice"), ..repo() }] {
        let id = if at == repo() { 99 } else { 1 };
        h.send(&Request::EditRepoHook { at: at.clone(), id, hook: spec() });
        assert_eq!(h.status(), 404);
        h.send(&Request::DeleteRepoHook { at, id });
        assert_eq!(h.status(), 404);
    }
    h.send(&Request::DeleteRepoHook { at: repo(), id: 1 });
    assert_eq!(h.status(), 204);
    assert!(h.body().is_empty());
    assert_eq!(ids(&h).as_slice(), &[2]);
    assert_eq!(h.service.setup().repository_hooks(OTHER).count(), 1);
    for request in [
        Request::RepoHooks { at: Repo { owner: boxed(b"alice"), ..repo() }, page: Page { page: None, limit: None } },
        Request::CreateRepoHook { at: Repo { owner: boxed(b"alice"), ..repo() }, hook: spec() },
    ] {
        h.send(&request);
        assert_eq!(h.status(), 404);
    }
}
#[test]
fn fake_forgejo_hooks_of_one_repository_are_not_those_of_another() {
    let mut h = harness();
    h.send(&create());
    h.send(&Request::CreateRepoHook { at: other(), hook: spec() });
    let request = Request::RepoHooks { at: other(), page: Page { page: None, limit: None } };
    h.send(&request);
    assert_eq!(h.decoded(&request).rows.len(), 1);
    let Row::Hook(hook) = h.decoded(&request).rows[0].clone() else { panic!("hook") };
    assert_eq!(hook.id, 2);
    assert_eq!(ids(&h).as_slice(), &[1]);
}
#[test]
fn fake_forgejo_setup_store_full_refuses_one_more() {
    let mut limits = fixtures::limits();
    limits.documents.inner = 2;
    let config = config();
    let mut store = Store::new(&limits);
    let admin = store.account(b"alice", true, b"");
    for at in [repo(), other()] {
        let request = Request::AddCollaborator { at, login: boxed(b"alice"), permission: Grant::Write };
        assert_eq!(store.answer(&request, admin, &config, &limits).unwrap().status, 204);
    }
    let request = Request::AddCollaborator { at: repo(), login: boxed(b"bob"), permission: Grant::Read };
    assert_eq!(store.answer(&request, admin, &config, &limits).unwrap().status, 503);
    assert_eq!(store.answer(&collaborator(Grant::Admin), admin, &config, &limits).unwrap().status, 204);
    for _ in 0_u32..2 {
        assert_eq!(store.answer(&create(), admin, &config, &limits).unwrap().status, 201);
    }
    assert_eq!(store.answer(&create(), admin, &config, &limits).unwrap().status, 503);
    assert_eq!(store.repository_hooks(REPOSITORY).count(), 2);
}
use skein_fake_forge_domain::{
    self as domain,
    api::{Error, Permission, What},
};
struct GrantCrossing {
    name: &'static str,
    grant: Grant,
    prior: Option<Grant>,
    missing: bool,
    full: bool,
    status: u16,
    store: Option<Grant>,
    permission: Result<Permission, Error>,
}
fn grant_crossings() -> [GrantCrossing; 6] {
    [
        GrantCrossing {
            name: "read",
            grant: Grant::Read,
            prior: None,
            missing: false,
            full: false,
            status: 204,
            store: Some(Grant::Read),
            permission: Ok(Permission::Read),
        },
        GrantCrossing {
            name: "write",
            grant: Grant::Write,
            prior: None,
            missing: false,
            full: false,
            status: 204,
            store: Some(Grant::Write),
            permission: Ok(Permission::Write),
        },
        GrantCrossing {
            name: "admin",
            grant: Grant::Admin,
            prior: None,
            missing: false,
            full: false,
            status: 204,
            store: Some(Grant::Admin),
            permission: Ok(Permission::Admin),
        },
        GrantCrossing {
            name: "replace in full domain",
            grant: Grant::Admin,
            prior: Some(Grant::Read),
            missing: false,
            full: false,
            status: 204,
            store: Some(Grant::Admin),
            permission: Ok(Permission::Admin),
        },
        GrantCrossing {
            name: "store-only repository",
            grant: Grant::Write,
            prior: None,
            missing: true,
            full: false,
            status: 204,
            store: Some(Grant::Write),
            permission: Err(Error::Missing(What::Repository)),
        },
        GrantCrossing {
            name: "domain full preserves both",
            grant: Grant::Write,
            prior: Some(Grant::Read),
            missing: false,
            full: true,
            status: 503,
            store: Some(Grant::Read),
            permission: Ok(Permission::None),
        },
    ]
}

#[test]
fn setup_grants_cross_into_the_lent_domain_atomically() {
    for row in grant_crossings() {
        let mut h = fixtures::setup_harness();
        h.service.setup_mut().account(b"spare", false, b"");
        let bot = h.service.setup_mut().account(b"bot", false, b"");
        if row.missing {
            h.domain = domain::Domain::new(&h.fake_env.limits, 3);
        }
        if let Some(prior) = row.prior {
            h.service.setup_mut().grant(REPOSITORY, b"bot", prior).unwrap();
            if !row.full {
                domain::grant(&mut h.domain, b"org/repo", bot, Permission::Read);
            }
        }
        if row.full {
            domain::grant(&mut h.domain, b"org/repo", 3, Permission::Read);
        }
        h.send(&Request::AddCollaborator { at: fixtures::at(), login: boxed(b"bot"), permission: row.grant });
        assert_eq!(h.status(), row.status, "{}", row.name);
        assert_eq!(h.service.setup().collaborator(REPOSITORY, b"bot"), row.store, "{}", row.name);
        assert_eq!(h.domain.permission(b"org/repo", bot), row.permission, "{}", row.name);
        assert_eq!(h.calls, 0, "only the grant crosses");
    }
}
