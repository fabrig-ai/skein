use super::{fixtures as f, read_support as r};
use crate::service::CiSetup;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api,
    github::{
        document::{Access, Document, Kind, Permission},
        request::Method,
    },
};
use skein_lib::{List, Secret};

const OTHER_REPOSITORY: u32 = 4; // tools in the shared fixture
const REPOSITORY: u32 = 2; // widgets in the shared fixture
const ACTIONS: &[u8] = b"/repos/acme/widgets/actions/permissions";
const WORKFLOWS: &[u8] = b"/repos/acme/widgets/actions/workflows?per_page=1";
fn harness() -> f::Harness {
    let mut config = f::config();
    config.app.permissions = Box::new([
        Permission { name: f::boxed(b"administration"), access: Access::Read },
        Permission { name: f::boxed(b"actions"), access: Access::Read },
    ]);
    config.tokens = List::with_capacity(1);
    config.tokens.push((Secret::new(f::boxed(b"person")), 31, None)).unwrap();
    let mut h = f::Harness::with_domain(f::limits(), config, r::domain_config());
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Admin);
    h
}
#[test]
fn github_fake_ci_setup_controls_are_independent_and_repository_bounded() {
    let mut h = harness();
    let token = r::token(&mut h);
    assert_eq!(
        r::read(&mut h, &token, api::Read::CiSetup),
        Ok(api::Answer::CiSetup(api::CiSetup { actions: Some(true), workflows: Some(true) }))
    );
    for actions in [Some(true), Some(false), None] {
        for workflows in [Some(true), Some(false), None] {
            assert!(h.service.set_ci_setup(REPOSITORY, CiSetup { actions, workflows }));
            assert_eq!(
                r::read(&mut h, &token, api::Read::CiSetup),
                Ok(api::Answer::CiSetup(api::CiSetup { actions, workflows }))
            );
        }
    }
    assert!(!h.service.set_ci_setup(0, CiSetup::DEFAULT));
    assert_eq!(h.service.repositories.get(&OTHER_REPOSITORY).unwrap().ci_setup, CiSetup::DEFAULT);
}
#[test]
fn github_fake_ci_setup_unavailable_is_a_refusal() {
    let mut h = harness();
    let token = r::token(&mut h);
    assert!(h.service.set_ci_setup(REPOSITORY, CiSetup { actions: None, workflows: None }));
    for target in [ACTIONS, WORKFLOWS] {
        let response = h.ask(Method::Get, target, Some(token.expose()), b"");
        f::refusal(&h, &response, 403, crate::texts::INTEGRATION);
    }
    assert!(h.service.set_ci_setup(REPOSITORY, CiSetup { actions: Some(false), workflows: Some(false) }));
    let response = h.ask(Method::Get, WORKFLOWS, Some(token.expose()), b"");
    assert_eq!(response.status, 200);
    let (rows, document) = h.decode(Kind::Workflows, &response);
    assert_eq!(rows.len(), 0);
    assert_eq!(document, Document::Workflows { total: 0, count: 0 });
}
#[test]
fn github_fake_ci_setup_checks_scope_reach_and_existing_grants() {
    let mut h = harness();
    let full = r::token(&mut h);
    let contents = f::bearer(h.mint(1, true).token.expose());
    for target in [ACTIONS, WORKFLOWS] {
        assert_eq!(h.ask(Method::Get, target, None, b"").status, 401);
        assert_eq!(h.ask(Method::Get, target, Some(contents.expose()), b"").status, 403);
        assert_eq!(h.ask(Method::Get, target, Some(full.expose()), b"").status, 200);
    }
    assert!(h.service.installations_mut().set_reach(1, f::listed(&[OTHER_REPOSITORY])));
    for target in [ACTIONS, WORKFLOWS] {
        assert_eq!(h.ask(Method::Get, target, Some(full.expose()), b"").status, 404);
    }
    assert!(h.service.installations_mut().set_reach(1, f::listed(&[REPOSITORY])));
    h.service.app.permissions = Box::new([]);
    for target in [ACTIONS, WORKFLOWS] {
        assert_eq!(h.ask(Method::Get, target, Some(full.expose()), b"").status, 403);
    }
    let person = f::bearer(b"person");
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Read);
    assert_eq!(h.ask(Method::Get, ACTIONS, Some(person.expose()), b"").status, 403);
    assert_eq!(h.ask(Method::Get, WORKFLOWS, Some(person.expose()), b"").status, 200);
}
