use super::{fixtures as f, read_support as r};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    forgejo::document::Body,
    github::{
        document::{Kind, Row},
        request::Method,
    },
};
use skein_lib::{Duration, List, Secret};

const TARGET: &[u8] = b"/repos/acme/widgets/issues/comments/1";
fn harness() -> f::Harness {
    let mut config = f::config();
    let mut accounts = List::with_capacity(3);
    for account in config.accounts.into_boxed() {
        accounts.push(account).unwrap();
    }
    accounts.push(crate::service::Account { login: f::boxed(b"bob"), id: 32, organization: false }).unwrap();
    config.accounts = accounts;
    config.tokens = List::with_capacity(2);
    for (token, user) in [(b"invented-author".as_slice(), 31), (b"invented-other", 32)] {
        config.tokens.push((Secret::new(f::boxed(token)), user, None)).unwrap();
    }
    let mut domain = r::domain_config();
    domain.limits.users = 4;
    let mut h = f::Harness::with_domain(f::limits(), config, domain);
    for user in [31, 32] {
        fake::grant(&mut h.domain, b"acme/widgets", user, a::Permission::Read);
    }
    let number = r::issue(&mut h, b"comment fixture");
    let result = r::call_as(&mut h, 31, a::Op::Write(a::Write::Comment { number, body: f::boxed(b"before") }));
    assert_eq!(result, a::Answer::Commented(1));
    h.env.now = h.env.now.saturating_add(Duration::from_secs(2));
    h
}
#[test]
fn github_fake_comment_edits_follow_domain_authorship_and_permission_and_advance_updated_at() {
    for (who, user, permission, status) in [
        (b"invented-author".as_slice(), 31, a::Permission::Read, 403_u16),
        (b"invented-other", 32, a::Permission::Read, 403),
        (b"invented-other", 32, a::Permission::Write, 403),
        (b"installation", f::BOT, a::Permission::Read, 404),
        (b"installation", f::BOT, a::Permission::Write, 200),
    ] {
        let mut h = harness();
        fake::grant(&mut h.domain, b"acme/widgets", user, permission);
        let token = if who == b"installation" { r::token(&mut h) } else { f::bearer(who) };
        let response = h.ask(Method::Patch, TARGET, Some(token.expose()), br#"{"body":"after"}"#);
        assert_eq!(response.status, status);
        let a::Answer::Comment { comment, .. } = r::call(&mut h, a::Op::Read(a::Read::Comment { id: 1 })) else {
            panic!("comment")
        };
        if status == 200 {
            let array = f::parts(&[b"[", &response.body, b"]"]);
            let (rows, _) = f::decode_body(Kind::CommentRows, &array, &h.limits.github);
            let Row::Comment(row) = rows.get(0).unwrap() else { panic!("comment response") };
            assert_eq!((row.id, row.author), (1, 31));
            assert_eq!(row.body, Body::Kept(f::boxed(b"after")));
            assert!(row.updated_at > row.created_at);
            assert_eq!(comment.body.as_ref(), b"after");
            assert!(comment.edited.unwrap() > comment.created);
        } else {
            assert_eq!(comment.body.as_ref(), b"before");
            assert_eq!(comment.edited, None);
        }
    }
}
#[test]
fn github_fake_comment_edit_refuses_extra_duplicate_empty_and_non_string_bodies() {
    let mut h = harness();
    let token = r::token(&mut h);
    for body in [
        br#"{"body":"after","unknown":1}"#.as_slice(),
        br#"{"body":"first","body":"second"}"#,
        br#"{"body":3}"#,
        br#"{"body":""}"#,
        b"{}",
        b"",
        br#"{"body":"after"}false"#,
    ] {
        let response = h.ask(Method::Patch, TARGET, Some(token.expose()), body);
        f::refusal(&h, &response, 422, b"Validation Failed");
        let a::Answer::Comment { comment, .. } = r::call(&mut h, a::Op::Read(a::Read::Comment { id: 1 })) else {
            panic!("comment")
        };
        assert_eq!(comment.body.as_ref(), b"before");
    }
    assert_eq!(h.ask(Method::Patch, TARGET, None, b"{}").status, 401);
    let limited = f::bearer(h.mint(1, true).token.expose());
    assert_eq!(h.ask(Method::Patch, TARGET, Some(limited.expose()), br#"{"body":"after"}"#).status, 403);
}
