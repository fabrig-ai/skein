use super::fixtures as f;
use crate::{
    installations::{Grant, Reach, Scope},
    service,
};
use skein_forge::github::{
    document::{Document, Kind},
    request::Method,
};

#[test]
fn github_fake_install_creates_the_installation() {
    let mut h = f::converted_with_installations();
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1"]), b"");
    assert_eq!(answer.status, 302);
    assert_eq!(
        answer.location.as_deref(),
        Some(b"https://setup.test/install?installation_id=4&setup_action=install&state=s1".as_slice())
    );
    let row = h.service.installations().get(4).expect("new installation");
    assert_eq!(
        (row.account, row.login.as_ref(), row.kind),
        (31, b"alice".as_slice(), crate::installations::Kind::User)
    );
    assert_eq!(row.created, h.now);
    match row.reach {
        Reach::All => {}
        Reach::Listed(_) => panic!("default reach is All"),
    }
    let response = h.app(Method::Get, b"/app/installations/4", b"");
    assert_eq!(response.status, 200);
    let (_, Document::Installation(row)) = h.decode(Kind::Installation, &response) else {
        panic!("installation document");
    };
    assert_eq!((row.id, row.account, row.login.as_ref()), (4, 31, b"alice".as_slice()));
    assert_eq!((h.calls, h.hooks), (0, 0));
}

#[test]
fn github_fake_install_reach_and_update() {
    let mut h = f::converted_with_installations();
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1&account=acme&repository_ids=2,4"]), b"");
    assert_eq!(answer.status, 302);
    assert_eq!(
        answer.location.as_deref(),
        Some(b"https://setup.test/install?installation_id=1&setup_action=update&state=s1".as_slice())
    );
    let grant = Grant { installation: 1, scope: Scope::Full };
    assert!(h.service.installations().reaches(&grant, 2), "listed repository granted");
    assert!(!h.service.installations().reaches(&grant, 6), "other repository excluded");
    for (query, status) in [
        (b"s1&account=unknown".as_slice(), 422),
        (b"s1&repository_ids=99", 422),
        (b"s1&repository_ids=2,2", 422),
        (b"s1&repository_ids=2,4,6", 422),
        (b"s1&repository_ids=2,", 422),
        (b"s1&repository_ids=", 422),
        (b"s1&repository_ids=not-a-number", 422),
        (b"s1&repository_ids=4294967296", 422),
        (b"s1&repositories=all&repository_ids=2", 422),
        (b"s1&repositories=other", 422),
        (b"s/1", 422),
        (b"", 422),
    ] {
        let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, query]), b"");
        f::page_refusal(&h, answer, status, b"Validation Failed");
        assert_eq!(h.service.installations().iter().count(), 3);
        assert!(!h.service.installations().reaches(&grant, 6), "refused page preserves reach");
    }
    let answer = h.page(Method::Get, b"/apps/unknown/installations/new?state=s1", b"");
    f::page_refusal(&h, answer, 404, b"Not Found");
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"._~-AZ09&account=acme&repositories=all"]), b"");
    assert_eq!(answer.status, 302);
    assert!(h.service.installations().reaches(&grant, 6), "update can grant All");
    assert!(answer.location.expect("setup redirect").ends_with(b"&state=._~-AZ09"), "state echoed");
    let mut h = f::pages();
    let answer = h.page(Method::Get, b"/apps/test-app/installations/new?state=s1&account=alice", b"");
    f::page_refusal(&h, answer, 422, b"Validation Failed");
    assert_eq!(h.service.installations().iter().count(), 3);
}

#[test]
fn github_fake_install_store_is_bounded() {
    let mut h = f::converted_with_installations();
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1"]), b"");
    assert_eq!(answer.status, 302);
    // Remove alice and fill her slot with a distinct scenario-independent installation.
    assert!(h.service.installations_mut().remove(4), "remove alice");
    assert_eq!(
        h.service.installations_mut().install(99, b"acme", crate::installations::Kind::Organization, Reach::All, h.now),
        Ok(5)
    );
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1"]), b"");
    f::page_refusal(&h, answer, 503, b"Server Error");
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1&account=acme"]), b"");
    assert_eq!(answer.status, 302, "full store still updates");
    assert!(answer.location.expect("setup redirect").contains(&b'1'), "existing id retained");
    assert!(service::valid(&h.limits, &f::config()), "test limits are representable");
    let mut limits = h.limits;
    limits.pages.states = 0;
    assert!(!service::valid(&limits, &f::config()));
    assert_eq!(service::Service::new(f::config(), &limits, 1).err(), Some(service::Problem::Limits));
    limits = h.limits;
    limits.pages.manifest_bytes = 0;
    assert!(!service::valid(&limits, &f::config()));
    assert_eq!(service::Service::new(f::config(), &limits, 1).err(), Some(service::Problem::Limits));
}

#[test]
fn github_fake_install_response_bound_preserves_the_store() {
    let mut h = f::converted_with_installations();
    h.service.limits.response = 128;
    let state = [b'a'; 100];
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, &state]), b"");
    assert_eq!(answer.status, 500);
    assert_eq!(h.service.installations().iter().count(), 3, "failed redirect does not install");
    let grant = Grant { installation: 1, scope: Scope::Full };
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, &state, b"&account=acme&repository_ids=2"]), b"");
    assert_eq!(answer.status, 500);
    assert!(h.service.installations().reaches(&grant, 4), "failed redirect preserves reach");
}

#[test]
fn github_fake_install_follows_the_whole_setup() {
    let mut h = f::pages();
    let input = f::manifest(f::NAME, f::REDIRECT, f::SETUP);
    let answer = h.page(Method::Post, b"/organizations/acme/settings/apps/new?state=s1", &f::form(&input));
    let link = f::link(&answer);
    assert!(link.starts_with(b"https://setup.test/convert?code="), "follow create link");
    let response = f::convert(&mut h, &f::code(&answer));
    let app = f::conversion(&h, &response);
    assert_eq!((app.owner.as_ref(), app.owner_id), (b"acme".as_slice(), 22));
    h.jwt = f::authorization_for(app.id, crate::keys::KEYS.first().expect("first key"), 1000, 1600);
    let answer = h.page(Method::Get, &f::parts(&[f::INSTALL, b"s1&repository_ids=2"]), b"");
    assert_eq!(answer.status, 302);
    assert_eq!(
        answer.location.as_deref(),
        Some(b"https://setup.test/install?installation_id=4&setup_action=install&state=s1".as_slice())
    );
    let token = h.mint(4, false);
    assert_eq!(token.expires, super::wall(4800));
    assert_eq!(
        h.service.installations().token(token.token.expose(), h.now),
        Ok(&Grant { installation: 4, scope: Scope::Full })
    );
    assert_eq!((h.calls, h.hooks), (0, 0), "setup makes no domain calls or hooks");
}

#[test]
fn github_fake_seeded_apps_registered_setup_url_accepts_an_existing_installation() {
    let mut h = f::pages();
    h.service.set_setup_url(Some(f::boxed(b"http://temper.test/forges/2/github/setup"))).expect("registered callback");
    let answer = h.page(Method::Get, b"/apps/test-app/installations/new?state=person&account=acme", b"");
    assert_eq!(answer.status, 302);
    assert_eq!(
        answer.location.as_deref(),
        Some(b"http://temper.test/forges/2/github/setup?installation_id=1&setup_action=update&state=person".as_slice())
    );
    assert_eq!(h.service.installations().iter().count(), 3, "existing installation reused");
    assert_eq!(h.service.set_setup_url(Some(f::boxed(b"relative/callback"))), Err(service::Problem::Config));
    assert_eq!(h.service.set_setup_url(Some(f::boxed(&[b'x'; 4097]))), Err(service::Problem::Config));
    h.service.set_setup_url(None).expect("callback optional");
    let answer = h.page(Method::Get, b"/apps/test-app/installations/new?state=person&account=acme", b"");
    assert_eq!(answer.status, 422);
}
