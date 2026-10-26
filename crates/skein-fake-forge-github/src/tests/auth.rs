use super::{fixtures as f, read_support as r, wall};
use crate::{
    installations::Scope,
    service::{Account, Outcome, Request},
};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api,
    github::{
        self,
        document::{Document, Kind},
        request::{Method, Request as Route},
    },
};
use skein_lib::{List, ReplyTo, Secret, Token};

fn harness() -> f::Harness {
    let mut config = f::config();
    let mut accounts = List::with_capacity(3);
    for account in config.accounts.into_boxed() {
        accounts.push(account).unwrap();
    }
    config.accounts = accounts;
    config.accounts.push(Account { login: f::boxed(b"bob"), id: 32, organization: false }).expect("account room");
    config.app.permissions = Box::new([github::document::Permission {
        name: f::boxed(b"statuses"),
        access: github::document::Access::Write,
    }]);
    config.tokens = List::with_capacity(2);
    for (value, user) in [(b"invented-alice".as_slice(), 31), (b"invented-bob", 32)] {
        config.tokens.push((Secret::new(f::boxed(value)), user, None)).expect("person room");
    }
    let mut domain = r::domain_config();
    domain.limits.users = 4;
    let mut h = f::Harness::with_domain(f::limits(), config, domain);
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Admin);
    fake::grant(&mut h.domain, b"acme/tools", 32, a::Permission::Read);
    h
}
fn person(alice: bool) -> Secret {
    f::bearer(if alice { b"invented-alice" } else { b"invented-bob" })
}
#[test]
fn github_fake_person_tokens_reach_their_users_and_not_the_apps_routes() {
    let mut h = harness();
    for (alice, id, login, reachable, denied) in [
        (true, 31, b"alice".as_slice(), b"/repos/acme/widgets".as_slice(), b"/repos/acme/tools".as_slice()),
        (false, 32, b"bob", b"/repos/acme/tools", b"/repos/acme/widgets"),
    ] {
        let token = person(alice);
        let response = h.ask(Method::Get, b"/user", Some(token.expose()), b"");
        let (_, Document::User(user)) = h.decode(Kind::User, &response) else { panic!("person identity") };
        assert_eq!((user.id, user.login.as_ref()), (id, login));
        assert_eq!(h.ask(Method::Get, reachable, Some(token.expose()), b"").status, 200);
        assert_eq!(h.ask(Method::Get, denied, Some(token.expose()), b"").status, 404);
        let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), f::VIEWER);
        let (_, Document::Viewer(viewer)) = h.decode(Kind::Viewer, &response) else { panic!("person viewer") };
        assert_eq!((viewer.id, viewer.login.as_ref()), (id, login));
        for (method, target) in [
            (Method::Get, b"/app".as_slice()),
            (Method::Get, f::INSTALLATIONS),
            (Method::Get, b"/app/hook/config"),
            (Method::Patch, b"/app/hook/config"),
            (Method::Get, b"/app/installations/1"),
            (Method::Post, b"/app/installations/1/access_tokens"),
            (Method::Delete, b"/app/installations/1"),
        ] {
            let calls = h.calls;
            let response = h.ask(method, target, Some(token.expose()), b"");
            f::refusal(&h, &response, 401, b"Bad credentials");
            assert_eq!(h.calls, calls);
        }
    }
    let token = person(true);
    assert_eq!(
        h.ask(Method::Get, b"/repos/acme/widgets/rules/branches/trunk?per_page=100&page=1", Some(token.expose()), b"")
            .status,
        200
    );
    let response = h.ask(
        Method::Post,
        b"/repos/acme/widgets/issues",
        Some(token.expose()),
        br#"{"title":"person's issue","body":"body"}"#,
    );
    assert_eq!(response.status, 201);
    let a::Answer::Item { item, .. } = r::call(&mut h, a::Op::Read(a::Read::Item { number: 1, after: 0 })) else {
        panic!("created issue")
    };
    assert_eq!(item.author, 31);
    let (number, _) = r::pull(&mut h);
    let request = Route::Review {
        owner: f::boxed(b"acme"),
        name: f::boxed(b"widgets"),
        number,
        verdict: api::Verdict::Approve,
        body: f::boxed(b"review"),
    };
    let encoded = github::request::encode(&request, &h.limits.github).unwrap();
    assert_eq!(
        h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap()).status,
        200
    );
    let a::Answer::Pull(pull) = r::call(&mut h, a::Op::Read(a::Read::Pull { number })) else { panic!("reviews") };
    assert_eq!(pull.reviews.first().unwrap().author, 31);
    let minted = h.mint(1, false);
    let installation = f::bearer(minted.token.expose());
    for method in [Method::Delete, Method::Put] {
        let response = h.ask(method, b"/user/installations/1/repositories/2", Some(token.expose()), b"");
        assert_eq!(response.status, 204);
        assert!(response.body.is_empty());
        assert_eq!(
            h.ask(Method::Get, b"/repositories/2", Some(installation.expose()), b"").status,
            if method == Method::Put { 200 } else { 404 }
        );
    }
    assert!(h.service.installations_mut().set_reach(1, f::listed(&[2])));
    let response = h.ask(Method::Delete, b"/user/installations/1/repositories/2", Some(token.expose()), b"");
    f::refusal(&h, &response, 422, crate::texts::REMOVE_LAST_REPOSITORY);
    // bob cannot change the reach of a repository alice administers.
    assert_eq!(
        h.ask(Method::Delete, b"/user/installations/1/repositories/2", Some(person(false).expose()), b"").status,
        404
    );
}
#[test]
fn github_fake_user_on_an_installation_token_is_403() {
    let mut h = harness();
    for contents in [false, true] {
        let token = f::bearer(h.mint(1, contents).token.expose());
        let calls = h.calls;
        let response = h.ask(Method::Get, b"/user", Some(token.expose()), b"");
        f::refusal(&h, &response, 403, crate::texts::INTEGRATION);
        assert_eq!(h.calls, calls);
    }
}
#[test]
fn github_fake_revoked_installation_token_is_401() {
    let mut h = harness();
    for contents in [false, true] {
        let token = f::bearer(h.mint(1, contents).token.expose());
        let other = f::bearer(h.mint(1, contents).token.expose());
        let response = h.ask(Method::Delete, b"/installation/token", Some(token.expose()), b"");
        assert_eq!(response.status, 204);
        assert!(response.body.is_empty());
        for (method, target) in [(Method::Get, b"/repositories/2".as_slice()), (Method::Delete, b"/installation/token")]
        {
            let response = h.ask(method, target, Some(token.expose()), b"");
            f::refusal(&h, &response, 401, b"Bad credentials");
        }
        assert_eq!(h.ask(Method::Get, b"/repositories/2", Some(other.expose()), b"").status, 200);
    }
    assert_eq!(h.ask(Method::Delete, b"/installation/token", Some(person(true).expose()), b"").status, 401);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    Contents,
    Full,
    App,
    Public,
}
#[expect(clippy::too_many_lines, reason = "one table lists every routed request and its credential class")]
fn routes() -> Box<[(Route, Class)]> {
    use Class::{App, Contents, Full, Public};
    fn owner() -> Box<[u8]> {
        f::boxed(b"acme")
    }
    fn name() -> Box<[u8]> {
        f::boxed(b"widgets")
    }
    let head = r::object(1);
    Box::new([
        (Route::App, App),
        (Route::HookConfig, App),
        (Route::SetHook { url: f::boxed(b"https://hook.test/changed") }, App),
        (Route::Installations { page: 1 }, App),
        (Route::Installation { id: 1 }, App),
        (Route::Mint { installation: 1, scope: github::request::Scope::Full }, App),
        (Route::Uninstall { id: 1 }, App),
        (Route::BotUser { slug: f::boxed(b"test-app") }, Public),
        (Route::Conversion { code: f::boxed(b"unknown") }, Public),
        (Route::Reachable { page: 1, per_page: 2 }, Contents),
        (Route::Viewer, Full),
        (Route::Repository { id: 2 }, Contents),
        (Route::Locate { owner: owner(), name: name() }, Contents),
        (Route::Branch { owner: owner(), name: name(), branch: f::boxed(b"trunk") }, Contents),
        (Route::Branches { owner: owner(), name: name(), page: 1, per_page: 2 }, Contents),
        (Route::Raw { owner: owner(), name: name(), commit: head, path: f::boxed(b"one") }, Contents),
        (Route::Compare { owner: owner(), name: name(), before: head, after: head, per_page: 2 }, Contents),
        (Route::CreateBranch { owner: owner(), name: name(), branch: f::boxed(b"feature"), commit: head }, Contents),
        (Route::DeleteBranch { owner: owner(), name: name(), branch: f::boxed(b"feature") }, Contents),
        (Route::Issues { owner: owner(), name: name(), since: wall(0), page: 1, per_page: 2 }, Full),
        (Route::Issue { owner: owner(), name: name(), number: 1 }, Full),
        (Route::Comments { owner: owner(), name: name(), number: 1, page: 1, per_page: 2 }, Full),
        (Route::Pull { owner: owner(), name: name(), number: 1 }, Full),
        (
            Route::PullsFor {
                owner: owner(),
                name: name(),
                head: f::boxed(b"feature"),
                base: f::boxed(b"trunk"),
                open: true,
            },
            Full,
        ),
        (Route::PullGraphql { owner: owner(), name: name(), number: 1 }, Full),
        (Route::PullFiles { owner: owner(), name: name(), number: 1, page: 1, per_page: 2 }, Full),
        (Route::Enqueue { id: f::boxed(b"PR_2_1"), head }, Full),
        (Route::Reviews { owner: owner(), name: name(), number: 1, page: 1, per_page: 2 }, Full),
        (Route::ReviewComments { owner: owner(), name: name(), number: 1, review: 1, page: 1, per_page: 2 }, Full),
        (Route::CombinedStatus { owner: owner(), name: name(), commit: head, page: 1, per_page: 2 }, Full),
        (Route::CheckRuns { owner: owner(), name: name(), commit: head, page: 1, per_page: 2 }, Full),
        (Route::Job { owner: owner(), name: name(), id: 1 }, Full),
        (Route::JobLog { owner: owner(), name: name(), id: 1 }, Full),
        (Route::RulesForBranch { owner: owner(), name: name(), branch: f::boxed(b"trunk"), page: 1 }, Full),
        (Route::Ruleset { owner: owner(), name: name(), id: 17 }, Full),
        (Route::ClassicProtection { owner: owner(), name: name(), branch: f::boxed(b"trunk") }, Full),
        (Route::Collaborators { owner: owner(), name: name(), page: 1, per_page: 2 }, Full),
        (Route::Permission { owner: owner(), name: name(), login: f::boxed(b"alice") }, Full),
        (Route::User { id: 31 }, Full),
        (
            Route::Merge {
                owner: owner(),
                name: name(),
                number: 1,
                head,
                method: github::plans::tables::Method::Squash,
            },
            Full,
        ),
        (
            Route::OpenPull {
                owner: owner(),
                name: name(),
                title: f::boxed(b"feature"),
                body: f::boxed(b"body"),
                head: f::boxed(b"feature"),
                base: f::boxed(b"trunk"),
            },
            Full,
        ),
        (Route::Update { owner: owner(), name: name(), number: 1, head }, Full),
        (
            Route::Review {
                owner: owner(),
                name: name(),
                number: 1,
                verdict: api::Verdict::Approve,
                body: f::boxed(b"review"),
            },
            Full,
        ),
        (
            Route::Reviewers {
                owner: owner(),
                name: name(),
                number: 1,
                add: true,
                logins: Box::new([f::boxed(b"alice")]),
            },
            Full,
        ),
        (
            Route::Reviewers {
                owner: owner(),
                name: name(),
                number: 1,
                add: false,
                logins: Box::new([f::boxed(b"alice")]),
            },
            Full,
        ),
        (Route::Reopen { owner: owner(), name: name(), number: 1 }, Full),
        (Route::Close { owner: owner(), name: name(), number: 1 }, Full),
        (Route::Edit { owner: owner(), name: name(), number: 1, title: Some(f::boxed(b"edited")), body: None }, Full),
        (Route::Post { owner: owner(), name: name(), number: 1, body: f::boxed(b"comment") }, Full),
        (
            Route::CreateIssue { owner: owner(), name: name(), title: f::boxed(b"issue"), body: f::boxed(b"body") },
            Contents,
        ),
        (
            Route::Status {
                owner: owner(),
                name: name(),
                commit: head,
                context: f::boxed(b"ci"),
                check: api::Check::Passed,
            },
            Full,
        ),
    ])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Credential {
    Full,
    Contents,
    Expired,
    Revoked,
    Person,
    Jwt,
}
fn expected(class: Class, credential: Credential, target: &[u8]) -> Option<u16> {
    if skein_lib::bytes::find(target, b"/statuses/").is_some() {
        match credential {
            Credential::Full | Credential::Contents | Credential::Person => Some(403),
            Credential::Expired | Credential::Revoked | Credential::Jwt => Some(401),
        }
    } else if target == b"/user" {
        match credential {
            Credential::Full | Credential::Contents => Some(403),
            Credential::Expired | Credential::Revoked | Credential::Jwt => Some(401),
            Credential::Person => None,
        }
    } else if target == b"/installation/token" {
        match credential {
            Credential::Full | Credential::Contents => None,
            Credential::Expired | Credential::Revoked | Credential::Jwt | Credential::Person => Some(401),
        }
    } else if target.starts_with(b"/user/installations/") {
        match credential {
            Credential::Full | Credential::Contents => Some(403),
            Credential::Expired | Credential::Revoked | Credential::Jwt => Some(401),
            Credential::Person => None,
        }
    } else {
        match class {
            Class::Public => None,
            Class::App => match credential {
                Credential::Jwt => None,
                Credential::Full
                | Credential::Contents
                | Credential::Expired
                | Credential::Revoked
                | Credential::Person => Some(401),
            },
            Class::Contents | Class::Full => match credential {
                Credential::Expired | Credential::Revoked | Credential::Jwt => Some(401),
                Credential::Contents => match class {
                    Class::Full => Some(if target == b"/graphql" { 200 } else { 403 }),
                    Class::Contents => None,
                    Class::App | Class::Public => unreachable!(),
                },
                Credential::Full | Credential::Person => None,
            },
        }
    }
}
fn matrix_request(h: &mut f::Harness, method: Method, target: &[u8], body: &[u8], class: Class) {
    let full = f::bearer(h.mint(1, false).token.expose());
    let contents = f::bearer(h.mint(1, true).token.expose());
    let (expired, _) = h.service.installations_mut().mint(1, Scope::Full, wall(0)).unwrap();
    let expired = f::bearer(expired.expose());
    let (revoked, _) = h.service.installations_mut().mint(1, Scope::Full, h.now).unwrap();
    assert!(h.service.installations_mut().revoke(revoked.expose()));
    let revoked = f::bearer(revoked.expose());
    h.now = wall(3600);
    let jwt = f::authorization(super::app().key, 3300, 3900);
    for (credential, token) in [
        (Credential::Full, full),
        (Credential::Contents, contents),
        (Credential::Expired, expired),
        (Credential::Revoked, revoked),
        (Credential::Person, person(true)),
        (Credential::Jwt, jwt),
    ] {
        let outcome = h.service.request(
            &Request {
                validator: None,
                method,
                target,
                body,
                authorization: Some(token.expose()),
                agent: true,
                version: Some(github::pin::API_VERSION.as_bytes()),
            },
            h.now,
            ReplyTo::new(Token::new(77)),
        );
        let outcome = match outcome {
            Outcome::Admit(admission) => {
                h.service.admit(admission, &mut h.domain, &h.env, ReplyTo::new(Token::new(77)))
            }
            Outcome::Call(_, _) | Outcome::Reply(_) => outcome,
        };
        let expected = expected(class, credential, target);
        if let Some(status) = expected {
            let Outcome::Reply(response) = outcome else {
                panic!("{credential:?} must be refused before domain for {method:?}, target length {}", target.len())
            };
            assert_eq!(response.status, status, "{credential:?} {method:?}, target length {}", target.len());
            if status == 200 {
                let (_, Document::Graphql(error)) = h.decode(Kind::Viewer, &response) else {
                    panic!("GraphQL refusal document")
                };
                assert_eq!(error.message.as_ref(), crate::texts::INTEGRATION);
            } else {
                f::refusal(
                    h,
                    &response,
                    status,
                    if status == 401 {
                        b"Bad credentials"
                    } else if credential == Credential::Person {
                        crate::texts::PERSON
                    } else {
                        crate::texts::INTEGRATION
                    },
                );
            }
        } else {
            match outcome {
                Outcome::Admit(_) => panic!("one admission per request"),
                Outcome::Call(_, fake::Event::Call { user, .. }) => {
                    assert_eq!(user, if credential == Credential::Person { 31 } else { f::BOT });
                }
                Outcome::Reply(response) => {
                    // Installation listing is restricted to installation tokens; conversion
                    // codes and enqueue node ids may be absent from this entrance fixture.
                    let expected_person_listing =
                        target.starts_with(b"/installation/repositories?") && credential == Credential::Person;
                    assert!(
                        response.status < 300 || expected_person_listing || class == Class::Public,
                        "{credential:?} unexpectedly refused with {} for {method:?}, target length {}",
                        response.status,
                        target.len()
                    );
                }
            }
        }
    }
}
#[test]
fn github_fake_contents_only_token_scope_matrix() {
    for (method, target) in [
        (Method::Get, b"/user".as_slice()),
        (Method::Delete, b"/installation/token"),
        (Method::Put, b"/user/installations/1/repositories/2"),
        (Method::Delete, b"/user/installations/1/repositories/2"),
    ] {
        matrix_request(&mut harness(), method, target, b"", Class::Full);
    }
    for (request, class) in routes() {
        let mut h = harness();
        let encoded = github::request::encode(&request, &h.limits.github).unwrap();
        matrix_request(&mut h, encoded.method, &encoded.target, encoded.body.as_deref().unwrap_or(b""), class);
    }
}
#[test]
fn github_fake_serves_only_the_dialects_routes() {
    let mut h = harness();
    let token = person(true);
    for (method, target) in [
        (Method::Get, b"/".as_slice()),
        (Method::Get, b"/api/v3/repos/acme/widgets"),
        (Method::Get, b"/user/repos"),
        (Method::Get, b"/orgs/acme/members"),
        (Method::Get, b"/repos/acme/widgets/actions/workflows"),
        (Method::Post, b"/repos/acme/widgets/check-runs"),
        (Method::Put, b"/repos/acme/widgets/contents/file"),
        (Method::Post, b"/repos/acme/widgets/git/commits"),
        (Method::Delete, b"/repos/acme/widgets"),
        (Method::Get, b"/graphql"),
        (Method::Get, b"/repos/acme/widgets?unrecognized=true"),
    ] {
        let calls = h.calls;
        let response = h.ask(method, target, Some(token.expose()), b"");
        f::refusal(&h, &response, 404, b"Not Found");
        assert_eq!(h.calls, calls);
    }
    for body in [
        br#"{"query":"query { viewer { login } }"}"#.as_slice(),
        br#"{"query":"query { repository(owner: \"acme\", name: \"widgets\") { id } }"}"#,
        br#"{"query":"mutation { deleteIssue(input: {id: \"unknown\"}) { clientMutationId } }"}"#,
    ] {
        let calls = h.calls;
        let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), body);
        let (_, Document::Graphql(error)) = h.decode(Kind::Viewer, &response) else { panic!("unsupported document") };
        assert_eq!(error.message.as_ref(), b"Unsupported query");
        assert_eq!(h.calls, calls);
    }
}

#[test]
fn github_fake_all_repositories_stays_within_the_installations_account() {
    let mut h = harness();
    for contents in [false, true] {
        let token = f::bearer(h.mint(2, contents).token.expose());
        let calls = h.calls;
        for target in [b"/repos/acme/widgets".as_slice(), b"/repositories/2"] {
            assert_eq!(h.ask(Method::Get, target, Some(token.expose()), b"").status, 404);
        }
        assert_eq!(h.calls, calls);
        let response = h.ask(Method::Get, f::REACHABLE, Some(token.expose()), b"");
        let (rows, _) = h.decode(Kind::ReachableRows, &response);
        assert!(rows.is_empty());
    }
}

#[test]
fn github_fake_installation_repository_removal_refusals_preserve_reach() {
    use crate::installations::Reach;
    for (reach, status, still_reachable, all) in [
        (Reach::All, 422_u16, true, true),
        (f::listed(&[2]), 422, true, false),
        (f::listed(&[2, 4]), 204, false, false),
    ] {
        let mut h = harness();
        assert!(h.service.installations_mut().set_reach(1, reach));
        let token = person(true);
        let response = h.ask(Method::Delete, b"/user/installations/1/repositories/2", Some(token.expose()), b"");
        if status == 422 {
            f::refusal(
                &h,
                &response,
                422,
                if all { crate::texts::REMOVE_ALL_REPOSITORIES } else { crate::texts::REMOVE_LAST_REPOSITORY },
            );
        } else {
            assert_eq!(response.status, 204);
        }
        let installation = f::bearer(h.mint(1, false).token.expose());
        assert_eq!(
            h.ask(Method::Get, b"/repositories/2", Some(installation.expose()), b"").status,
            if still_reachable { 200 } else { 404 }
        );
        assert_eq!(h.ask(Method::Put, b"/user/installations/1/repositories/2", Some(token.expose()), b"").status, 204);
        if status == 422 {
            let reach = &h.service.installations().get(1).unwrap().reach;
            match reach {
                Reach::All => assert!(all),
                Reach::Listed(ids) => {
                    assert!(!all);
                    assert_eq!(ids.as_slice(), &[2]);
                }
            }
        }
    }
}
