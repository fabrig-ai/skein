use super::fixtures::{self as f, Harness, bearer, refusal};
use crate::service::{Outcome, Pending, Request};
use skein_fake_forge_domain::{
    self as fake,
    api::{self as a, Answer, Error, What},
};
use skein_forge::github::{
    self,
    document::{Document, Kind, Row},
    request::Method,
};
use skein_lib::{ReplyTo, Token};

#[test]
fn github_fake_repository_reads_inside_reach() {
    let mut h = Harness::new();
    h.branch(b"feature/one");
    for contents in [false, true] {
        let token = bearer(h.mint(1, contents).token.expose());
        let by_id = h.ask(Method::Get, b"/repositories/2", Some(token.expose()), b"");
        let by_name = h.ask(Method::Get, b"/repos/acme/widgets", Some(token.expose()), b"");
        assert_eq!(by_id.status, 200);
        assert_eq!(by_id.body, by_name.body);
        let (_, Document::Repository(row)) = h.decode(Kind::Repository, &by_id) else {
            panic!("repository document");
        };
        assert_eq!(row.id, 2);
        assert_eq!(row.owner.as_ref(), b"acme");
        assert_eq!(row.owner_id, 22);
        assert!(row.organization, "table owns owner kind");
        assert_eq!(row.name.as_ref(), b"widgets");
        assert_eq!(row.default_branch.as_ref(), b"trunk");
        assert!(!row.merge && row.squash && row.rebase, "domain settings");
        assert_eq!(row.html_url.as_ref(), b"https://github.com/acme/widgets");
        for target in [b"/repos/acme/widgets/branches/trunk".as_slice(), b"/repos/acme/widgets/branches/feature/one"] {
            let response = h.ask(Method::Get, target, Some(token.expose()), b"");
            assert_eq!(response.status, 200);
            let (_, Document::Branch(branch)) = h.decode(Kind::Branch, &response) else {
                panic!("branch document");
            };
            let expected = skein_fake_forge_protocol::routes::object(h.first).bytes;
            assert_eq!(branch.commit, expected);
            assert_eq!(Some(branch.name.as_ref()), target.strip_prefix(b"/repos/acme/widgets/branches/"));
        }
        let response = h.ask(Method::Get, b"/repos/acme/widgets/branches/missing", Some(token.expose()), b"");
        refusal(&h, &response, 404, b"Not Found");
        let response = h.ask(Method::Get, b"/repositories/4", Some(token.expose()), b"");
        let (_, Document::Repository(row)) = h.decode(Kind::Repository, &response) else {
            panic!("repository document");
        };
        assert!(!row.organization, "user-owned repository");
    }
    assert_eq!(h.calls, 12, "each repository read is exactly one domain call");
    assert_eq!(h.hooks, 0, "read story's hook count is zero");
}

#[test]
fn github_fake_repository_reads_outside_reach_are_404() {
    let mut h = Harness::new();
    // A fresh bounded installation reaches only widgets.
    assert!(h.service.installations_mut().remove(2), "make room in bounded store");
    let id = h
        .service
        .installations_mut()
        .install(22, b"acme", crate::installations::Kind::Organization, f::listed(&[2]), h.now)
        .expect("listed installation");
    let token = bearer(h.mint(id, false).token.expose());
    for target in [
        b"/repositories/4".as_slice(),
        b"/repos/acme/tools",
        b"/repos/acme/tools/branches/trunk",
        b"/repos/acme/tools/branches?per_page=2&page=1",
        b"/repositories/999",
        b"/repositories/0",
        b"/repositories/4294967296",
        b"/repos/acme/unknown",
    ] {
        let outcome = h.service.request(
            &Request {
                validator: None,
                method: Method::Get,
                target,
                authorization: Some(token.expose()),
                agent: true,
                version: Some(github::pin::API_VERSION.as_bytes()),
                body: b"",
            },
            h.now,
            ReplyTo::new(Token::new(77)),
        );
        let outcome = admitted(&mut h, outcome);
        let Outcome::Reply(response) = outcome else {
            panic!("outside reach never calls domain");
        };
        refusal(&h, &response, 404, b"Not Found");
    }
    assert_eq!(h.ask(Method::Get, b"/repositories/2", Some(token.expose()), b"").status, 200);
    assert_eq!(h.calls, 1);
    assert_eq!(h.hooks, 0);
}

#[test]
fn github_fake_branches_page_with_link() {
    let mut h = Harness::new();
    h.branch(b"alpha");
    h.branch(b"feature/one");
    let token = bearer(h.mint(1, false).token.expose());
    for (target, names, linked) in [
        (
            b"/repos/acme/widgets/branches?per_page=100&page=1".as_slice(),
            &[b"alpha".as_slice(), b"feature/one"][..],
            true,
        ),
        (b"/repos/acme/widgets/branches?per_page=100&page=2", &[b"trunk".as_slice()][..], false),
        (b"/repos/acme/widgets/branches?per_page=1&page=1", &[b"alpha".as_slice()][..], true),
        (b"/repos/acme/widgets/branches?per_page=1&page=2", &[b"feature/one".as_slice()][..], true),
        (b"/repos/acme/widgets/branches?per_page=100&page=99", &[][..], false),
        (b"/repos/acme/widgets/branches?per_page=4294967295&page=4294967295", &[][..], false),
    ] {
        let response = h.ask(Method::Get, target, Some(token.expose()), b"");
        assert_eq!(response.status, 200);
        let (rows, document) = h.decode(Kind::BranchRows, &response);
        assert_eq!(document, Document::Rows);
        assert_eq!(rows.len(), u32::try_from(names.len()).expect("fixture rows"));
        for (row, name) in rows.iter().zip(names) {
            let Row::Branch(actual) = row else {
                panic!("branch row");
            };
            assert_eq!(actual.as_ref(), *name);
        }
        assert_eq!(response.link.is_some(), linked);
        if let Some(link) = response.link {
            assert!(skein_lib::bytes::find(&link, b"rel=\"next\"").is_some(), "next relation");
            assert!(skein_lib::bytes::find(&link, b"rel=\"last\"").is_some(), "last relation");
            if target == b"/repos/acme/widgets/branches?per_page=1&page=1" {
                assert_eq!(link.as_ref(), b"<http://127.0.0.1:3000/repos/acme/widgets/branches?per_page=1&page=2>; rel=\"next\", <http://127.0.0.1:3000/repos/acme/widgets/branches?per_page=1&page=3>; rel=\"last\"");
            }
        }
    }
    assert_eq!(h.hooks, 0);
}

fn pending(h: &mut Harness, target: &[u8], token: &[u8]) -> Pending {
    let outcome = h.service.request(
        &Request {
            validator: None,
            method: Method::Get,
            target,
            authorization: Some(token),
            agent: true,
            version: Some(github::pin::API_VERSION.as_bytes()),
            body: b"",
        },
        h.now,
        ReplyTo::new(Token::new(77)),
    );
    let outcome = match outcome {
        Outcome::Admit(admission) => h.service.admit(admission, &mut h.domain, &h.env, ReplyTo::new(Token::new(77))),
        other @ (Outcome::Call(..) | Outcome::Reply(_)) => other,
    };
    let Outcome::Call(pending, fake::Event::Call { reply_to, user, repository, op }) = outcome else {
        panic!("repository calls domain");
    };
    assert_eq!(reply_to.into_token(), Token::new(77));
    assert_eq!(user, f::BOT);
    assert_eq!(repository.as_ref(), b"acme/widgets");
    let a::Op::Read(a::Read::Settings) = op else {
        panic!("repository settings read");
    };
    pending
}
#[test]
fn github_fake_domain_errors_map_to_statuses() {
    let mut h = Harness::new();
    let token = bearer(h.mint(1, false).token.expose());
    for (error, status) in [
        (Error::Unavailable, 503),
        (Error::Timeout, 504),
        (Error::Missing(What::Repository), 404),
        (Error::Forbidden, 404),
        (Error::RateLimited { reset: skein_lib::Time::ZERO }, 500),
        (Error::TooLarge, 500),
        (Error::Full, 500),
        (Error::Exists, 500),
        (Error::Circular, 500),
        (Error::NothingToMerge, 500),
        (Error::Empty, 500),
        (Error::Closed, 500),
        (Error::Stale, 500),
        (Error::Conflict, 500),
        (Error::Protected, 500),
        (Error::Unreachable, 500),
        (Error::Refused, 500),
    ] {
        let pending = pending(&mut h, b"/repositories/2", token.expose());
        let Outcome::Reply(response) = h.service.finish(pending, Err(error), ReplyTo::new(Token::new(77)), &h.domain)
        else {
            panic!("read settles")
        };
        refusal(&h, &response, status, if status == 404 { b"Not Found" } else { b"Server Error" });
    }
    let pending = pending(&mut h, b"/repos/acme/widgets", token.expose());
    let Outcome::Reply(response) = h.service.finish(pending, Ok(Answer::Done), ReplyTo::new(Token::new(77)), &h.domain)
    else {
        panic!("read settles")
    };
    refusal(&h, &response, 500, b"Server Error");
}

#[test]
fn github_fake_read_continuations_reject_wrong_answers() {
    let mut h = Harness::new();
    let token = bearer(h.mint(1, false).token.expose());
    for target in [b"/repos/acme/widgets/branches/trunk".as_slice(), b"/repos/acme/widgets/branches?per_page=2&page=1"]
    {
        let outcome = h.service.request(
            &Request {
                validator: None,
                method: Method::Get,
                target,
                authorization: Some(token.expose()),
                agent: true,
                version: Some(github::pin::API_VERSION.as_bytes()),
                body: b"",
            },
            h.now,
            ReplyTo::new(Token::new(77)),
        );
        let outcome = admitted(&mut h, outcome);
        let Outcome::Call(pending, _) = outcome else {
            panic!("read calls domain");
        };
        let Outcome::Reply(response) =
            h.service.finish(pending, Ok(Answer::Done), ReplyTo::new(Token::new(77)), &h.domain)
        else {
            panic!("read settles")
        };
        refusal(&h, &response, 500, b"Server Error");
    }
}

#[test]
fn github_fake_table_row_missing_from_domain_is_404() {
    let mut h = Harness::new();
    assert!(h.service.installations_mut().set_reach(1, crate::installations::Reach::All));
    let token = bearer(h.mint(1, false).token.expose());
    let response = h.ask(Method::Get, b"/repositories/6", Some(token.expose()), b"");
    refusal(&h, &response, 404, b"Not Found");
    assert_eq!(h.calls, 1, "table row is checked against domain at call time");
}

fn admitted(h: &mut Harness, outcome: Outcome) -> Outcome {
    match outcome {
        Outcome::Admit(admission) => h.service.admit(admission, &mut h.domain, &h.env, ReplyTo::new(Token::new(77))),
        other @ (Outcome::Call(..) | Outcome::Reply(_)) => other,
    }
}
