use super::{
    fixtures::{self, Harness, boxed},
    repo_setup::{harness as setup_harness, other, repo},
};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::forgejo::{
    document::{self as d, Document, Row},
    id::ObjectFormat,
    request::{Grant, Request},
};
use skein_lib::{Duration, Secret};

// repo_setup's acme/widgets and bob/widgets configuration.
const REPOSITORY: u32 = 2;
const OTHER: u32 = 3;
const BOB: u64 = 2;
const READ_TOKEN: &[u8] = b"setup-read-token";

fn harness() -> Harness {
    harness_with_default(b"main")
}
fn harness_with_default(default: &[u8]) -> Harness {
    let mut h = setup_harness();
    // The setup fixture names these two repositories; by-id reads inspect them.
    h.fake_env.limits.limits.repositories = 2;
    h.domain = fake::Domain::new(&h.fake_env.limits, 3);
    for (id, name, default) in
        [(REPOSITORY, b"acme/widgets".as_slice(), default), (OTHER, b"bob/widgets".as_slice(), b"main".as_slice())]
    {
        fake::repository(
            &mut h.domain,
            &h.fake_env.limits,
            a::Setup {
                id,
                name: boxed(name),
                default: boxed(default),
                tree: Box::from([]),
                labels: Box::from([]),
                checks: a::Checks {
                    cues: Box::new([]),
                    contexts: Box::from([]),
                    latency_min: Duration::ZERO,
                    latency_max: Duration::ZERO,
                    silent: 0,
                    passes: 0,
                    reruns: 0,
                    cue: None,
                },
                protection: None,
                hooked: false,
            },
        );
    }
    h
}

fn by_id() -> Request {
    Request::RepositoryById { id: u64::from(REPOSITORY) }
}
fn permission(login: &[u8]) -> Request {
    Request::Permission { at: repo(), login: boxed(login) }
}
fn grant(h: &mut Harness, login: &[u8], value: Grant) {
    h.service.setup_mut().grant(REPOSITORY, login, value).expect("scenario grant");
}
fn plain(scopes: &[&[u8]]) -> Harness {
    let mut h = harness();
    h.service.setup_mut().token(BOB, b"reads", scopes, Secret::new(boxed(READ_TOKEN)));
    h
}
fn ask_plain(h: &mut Harness, target: &[u8]) {
    h.ask_as(b"GET", target, b"", READ_TOKEN);
}
fn read_permission(h: &mut Harness, login: &[u8]) -> d::CollaboratorPermission {
    let request = permission(login);
    h.send(&request);
    let Document::Permission(value) = h.decoded(&request).document else { panic!("permission document") };
    value
}
fn expected_repository(id: u64, owner: &[u8], html_url: &[u8]) -> Document {
    Document::Repository(d::Repository {
        id,
        owner: d::User { id: 0, login: boxed(owner), is_admin: false, full_name: boxed(b"") },
        name: boxed(b"widgets"),
        object_format: ObjectFormat::Sha1,
        default_branch: boxed(b"main"),
        default_merge_style: d::MergeStyle::Squash,
        allow_merge_commits: true,
        allow_squash_merge: true,
        allow_rebase: true,
        html_url: boxed(html_url),
    })
}

#[test]
fn setup_by_id_reads_lent_settings_and_keeps_the_unhosted_default() {
    let mut h = harness_with_default(b"trunk");
    for (name, id, lent, changed, expected) in [
        ("seeded trunk", REPOSITORY, true, None, (b"trunk".as_slice(), true, true, true)),
        ("seeded main", OTHER, true, None, (b"main".as_slice(), true, true, true)),
        (
            "changed styles",
            REPOSITORY,
            true,
            Some(a::Settings { default: boxed(b"trunk"), merge: false, rebase: false, squash: false }),
            (b"trunk".as_slice(), false, false, false),
        ),
        ("no domain", REPOSITORY, false, None, (b"main".as_slice(), true, true, true)),
    ] {
        if let Some(settings) = changed {
            fake::settings(&mut h.domain, b"acme/widgets", settings);
        }
        h.lend_domain = lent;
        let request = Request::RepositoryById { id: u64::from(id) };
        h.send(&request);
        let Document::Repository(repository) = h.decoded(&request).document else { panic!("repository document") };
        assert_eq!(
            (
                repository.default_branch.as_ref(),
                repository.allow_merge_commits,
                repository.allow_rebase,
                repository.allow_squash_merge
            ),
            expected,
            "{name}"
        );
        assert_eq!(h.calls, 0, "{name}: setup inspection is no domain call");
    }
}

#[test]
fn admin_reads_a_public_repository_from_setup() {
    let mut h = harness();
    h.send(&by_id());
    assert_eq!(h.decoded(&by_id()).document, expected_repository(2, b"acme", b"http://127.0.0.1:3000/acme/widgets"));
    assert_eq!(h.calls, 0);
}
#[test]
fn admin_reads_another_owners_private_repository() {
    let mut h = harness();
    h.service.setup_mut().private(u64::from(OTHER)).unwrap();
    let request = Request::RepositoryById { id: u64::from(OTHER) };
    h.send(&request);
    assert_eq!(h.decoded(&request).document, expected_repository(3, b"bob", b"http://127.0.0.1:3000/bob/widgets"));
    assert_eq!(h.calls, 0);
}
#[test]
fn unknown_or_wider_repository_ids_answer_not_found() {
    for id in [99, u64::from(u32::MAX).saturating_add(1)] {
        let mut h = harness();
        h.send(&Request::RepositoryById { id });
        assert_eq!(h.status(), 404);
        assert_eq!(h.calls, 0);
    }
}
#[test]
fn plain_setup_account_reads_a_public_repository() {
    let mut h = plain(&[b"all"]);
    ask_plain(&mut h, b"/api/v1/repositories/2");
    assert_eq!(h.decoded(&by_id()).document, expected_repository(2, b"acme", b"http://127.0.0.1:3000/acme/widgets"));
    assert_eq!(h.calls, 0);
}
#[test]
fn unreadable_private_repository_by_id_is_not_found() {
    let mut h = plain(&[b"all"]);
    h.service.setup_mut().private(u64::from(REPOSITORY)).unwrap();
    ask_plain(&mut h, b"/api/v1/repositories/2");
    assert_eq!(h.status(), 404);
    assert_eq!(h.calls, 0);
}
#[test]
fn collaborator_reads_a_private_repository_by_id() {
    let mut h = plain(&[b"all"]);
    h.service.setup_mut().private(u64::from(REPOSITORY)).unwrap();
    grant(&mut h, b"bob", Grant::Read);
    ask_plain(&mut h, b"/api/v1/repositories/2");
    assert_eq!(h.status(), 200);
    assert_eq!(h.calls, 0);
}
#[test]
fn owner_reads_a_private_repository_by_id() {
    let mut h = plain(&[b"all"]);
    h.service.setup_mut().private(u64::from(OTHER)).unwrap();
    ask_plain(&mut h, b"/api/v1/repositories/3");
    assert_eq!(h.status(), 200);
}
#[test]
fn config_token_repository_read_reaches_the_domain() {
    let mut h = Harness::new(fixtures::config());
    h.send(&by_id());
    assert_eq!(h.status(), 200);
    assert_eq!(h.calls, 1);
}
#[test]
fn config_token_permission_read_reaches_the_domain() {
    let mut h = Harness::new(fixtures::config());
    h.send(&Request::Permission { at: fixtures::at(), login: boxed(b"person") });
    assert_eq!(h.status(), 200);
    assert_eq!(h.calls, 1);
}
#[test]
fn permission_reads_back_a_collaborator_put() {
    let mut h = harness();
    h.send(&Request::AddCollaborator { at: repo(), login: boxed(b"bob"), permission: Grant::Write });
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Write);
    assert_eq!(h.calls, 0);
}
#[test]
fn permission_reads_back_a_downgraded_collaborator_put() {
    let mut h = harness();
    for permission in [Grant::Write, Grant::Read] {
        h.send(&Request::AddCollaborator { at: repo(), login: boxed(b"bob"), permission });
    }
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Read);
}
#[test]
fn public_non_collaborator_has_no_setup_permission() {
    let mut h = harness();
    assert_eq!(
        read_permission(&mut h, b"bob"),
        d::CollaboratorPermission {
            permission: d::Permission::None,
            user: d::User { id: BOB, login: boxed(b"bob"), is_admin: false, full_name: boxed(b"") },
        }
    );
}
#[test]
fn repository_owner_permission_is_owner() {
    let mut h = harness();
    let request = Request::Permission { at: other(), login: boxed(b"bob") };
    h.send(&request);
    let Document::Permission(value) = h.decoded(&request).document else { panic!("permission document") };
    assert_eq!(value.permission, d::Permission::Owner);
}
#[test]
fn organization_owner_permission_is_owner_above_a_grant() {
    let mut h = harness();
    h.service.setup_mut().org(b"acme", b"bob");
    grant(&mut h, b"bob", Grant::Read);
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Owner);
}
fn team(h: &mut Harness, access: d::Access) {
    h.service.setup_mut().org(b"acme", b"alice");
    let id = h.service.setup_mut().add_team(
        b"acme",
        d::TeamSpec {
            name: boxed(b"widgets"),
            permission: access,
            includes_all_repositories: true,
            ..fixtures::team_spec()
        },
    );
    h.service.setup_mut().member(id, BOB);
}
#[test]
fn organization_team_permission_is_read_from_setup() {
    let mut h = harness();
    team(&mut h, d::Access::Write);
    grant(&mut h, b"bob", Grant::Read);
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Write);
}
#[test]
fn direct_grant_above_a_team_permission_wins() {
    let mut h = harness();
    team(&mut h, d::Access::Read);
    grant(&mut h, b"bob", Grant::Admin);
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Admin);
}
#[test]
fn organization_team_reads_a_private_repository() {
    let mut h = plain(&[b"all"]);
    team(&mut h, d::Access::Read);
    h.service.setup_mut().private(u64::from(REPOSITORY)).unwrap();
    ask_plain(&mut h, b"/api/v1/repositories/2");
    assert_eq!(h.status(), 200);
}
#[test]
fn permission_of_an_unknown_login_is_not_found() {
    let mut h = harness();
    h.send(&permission(b"widgets"));
    assert_eq!(h.status(), 404);
}
#[test]
fn permission_of_an_unknown_repository_is_not_found() {
    let mut h = harness();
    h.send(&Request::Permission {
        at: skein_forge::forgejo::request::Repo { owner: boxed(b"alice"), ..repo() },
        login: boxed(b"bob"),
    });
    assert_eq!(h.status(), 404);
}
#[test]
fn plain_account_cannot_query_another_logins_permission() {
    let mut h = plain(&[b"all"]);
    ask_plain(&mut h, b"/api/v1/repos/acme/widgets/collaborators/alice/permission");
    assert_eq!(h.status(), 403);
    assert_eq!(h.body().as_ref(), br#"{"message":"Only admins can query all permissions, repo admins can query all repo permissions, collaborators can query only their own"}"#);
}
#[test]
fn plain_account_can_query_its_own_permission() {
    let mut h = plain(&[b"all"]);
    grant(&mut h, b"bob", Grant::Write);
    ask_plain(&mut h, b"/api/v1/repos/acme/widgets/collaborators/bob/permission");
    let Document::Permission(value) = h.decoded(&permission(b"bob")).document else { panic!("permission document") };
    assert_eq!(value.permission, d::Permission::Write);
}
#[test]
fn setup_reads_without_repository_scope_are_forbidden() {
    let mut h = plain(&[b"read:user"]);
    for target in [b"/api/v1/repositories/2".as_slice(), b"/api/v1/repos/acme/widgets/collaborators/bob/permission"] {
        ask_plain(&mut h, target);
        assert_eq!(h.status(), 403);
        assert_eq!(h.calls, 0);
    }
}
#[test]
fn read_repository_scope_admits_both_setup_reads() {
    let mut h = plain(&[b"read:repository"]);
    for target in [b"/api/v1/repositories/2".as_slice(), b"/api/v1/repos/acme/widgets/collaborators/bob/permission"] {
        ask_plain(&mut h, target);
        assert_eq!(h.status(), 200);
        assert_eq!(h.calls, 0);
    }
}
#[test]
fn scenario_grant_is_visible_to_permission_reads() {
    let mut h = harness();
    for (value, expected) in [(Grant::Admin, d::Permission::Admin), (Grant::Read, d::Permission::Read)] {
        grant(&mut h, b"bob", value);
        assert_eq!(read_permission(&mut h, b"bob").permission, expected);
    }
}
#[test]
fn config_user_permission_needs_no_setup_account() {
    let mut h = harness();
    h.service.setup_mut().delete_account(b"bob");
    grant(&mut h, b"bob", Grant::Write);
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Write);
}

#[test]
fn permission_user_can_be_a_setup_only_account() {
    let mut h = harness();
    h.service.setup_mut().account(b"acme", false, b"widgets");
    assert_eq!(
        read_permission(&mut h, b"acme").user,
        d::User { id: 3, login: boxed(b"acme"), is_admin: false, full_name: boxed(b"") }
    );
}

fn full_grants() -> Harness {
    let mut h = harness();
    for repository in 0..fixtures::SETUP_ROWS {
        h.service.setup_mut().grant(repository, b"bob", Grant::Read).unwrap();
    }
    h
}

#[test]
fn full_grant_table_still_replaces_an_existing_grant() {
    let mut h = full_grants();
    grant(&mut h, b"bob", Grant::Write);
    assert_eq!(read_permission(&mut h, b"bob").permission, d::Permission::Write);
}

#[test]
fn full_grant_table_refuses_a_new_scenario_grant() {
    let mut h = full_grants();
    assert_eq!(h.service.setup_mut().grant(u32::MAX, b"bob", Grant::Write), Err(crate::Error::Limits));
    assert_eq!(h.service.setup().collaborator(u32::MAX, b"bob"), None);
}

#[test]
fn scenario_grant_refuses_a_login_past_its_bound() {
    let mut h = harness();
    let length = usize::try_from(h.env.limits.documents.string_bytes).unwrap().saturating_add(1);
    let login = scenario_login(length);
    assert_eq!(h.service.setup_mut().grant(REPOSITORY, &login, Grant::Read), Err(crate::Error::Limits));
    assert_eq!(h.service.setup().collaborator(REPOSITORY, &login), None);
}

fn scenario_login(length: usize) -> Box<[u8]> {
    let mut login = skein_lib::bytes::zeroed(length);
    login.fill(b'a');
    login
}
#[test]
fn scenario_grant_accepts_a_login_at_its_bound() {
    let mut h = harness();
    let login = scenario_login(usize::try_from(h.env.limits.documents.string_bytes).unwrap());
    assert_eq!(h.service.setup_mut().grant(REPOSITORY, &login, Grant::Read), Ok(()));
    assert_eq!(h.service.setup().collaborator(REPOSITORY, &login), Some(Grant::Read));
}

fn created_bot(h: &mut Harness) -> d::User {
    let request = Request::CreateUser {
        login: boxed(b"reconciler"),
        email: boxed(b"reconciler@example.test"),
        full_name: boxed(b"Reconciler Bot"),
        password: Secret::new(boxed(b"fixture-password")),
        must_change_password: false,
    };
    h.send(&request);
    let Document::User(user) = h.decoded(&request).document else { panic!("created user") };
    user
}

#[test]
fn authenticated_user_names_a_created_account_by_its_login() {
    let mut h = harness();
    let bot = created_bot(&mut h);
    h.service.setup_mut().token(bot.id, b"reads", &[b"read:user"], Secret::new(boxed(READ_TOKEN)));
    ask_plain(&mut h, b"/api/v1/user");
    assert_eq!(h.decoded(&Request::User).document, Document::User(bot));
}

#[test]
fn user_search_names_a_created_account_by_its_login() {
    let mut h = harness();
    let bot = created_bot(&mut h);
    let request = Request::UserSearch { uid: bot.id, limit: None };
    h.send(&request);
    assert_eq!(&*h.decoded(&request).rows, &[Row::User(d::User { full_name: boxed(b""), ..bot })]);
}

#[test]
fn collaborators_name_an_added_created_account_by_its_login() {
    let mut h = harness();
    let bot = created_bot(&mut h);
    fake::grant(&mut h.domain, b"acme/widgets", 1, a::Permission::Admin);
    h.send(&Request::AddCollaborator { at: repo(), login: bot.login.clone(), permission: Grant::Write });
    assert_eq!(h.status(), 204);
    let request = Request::Collaborators { at: repo(), page: fixtures::PAGE };
    h.send(&request);
    let rows = h.decoded(&request).rows;
    assert!(rows.contains(&Row::User(d::User { full_name: boxed(b""), ..bot })), "{rows:?}");
}

#[test]
fn permission_reads_a_created_accounts_write_grant_by_its_login() {
    let mut h = harness();
    let bot = created_bot(&mut h);
    h.send(&Request::AddCollaborator { at: repo(), login: bot.login.clone(), permission: Grant::Write });
    assert_eq!(
        read_permission(&mut h, &bot.login),
        d::CollaboratorPermission { permission: d::Permission::Write, user: d::User { full_name: boxed(b""), ..bot } }
    );
}
