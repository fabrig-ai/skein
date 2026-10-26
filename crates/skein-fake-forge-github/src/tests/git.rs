use super::fixtures::{self as f, Harness, boxed};
use crate::{
    GitCall, Minted,
    installations::{Kind, Reach, Scope},
    service::{Outcome, Request},
};
use skein_fake_forge_domain::{self as domain, api};
use skein_fake_forge_protocol::git::{self, Call};
use skein_forge::github::request::Method;
use skein_lib::{
    Duration, List, ReplyTo, Secret, Token, Wall,
    base64::{self, Alphabet},
};

fn basic(login: &[u8], token: &[u8]) -> Secret {
    let value = f::parts(&[login, b":", token]);
    let length = base64::encoded_len(Alphabet::Standard, u32::try_from(value.len()).expect("bounded basic header"))
        .expect("bounded basic header");
    let mut encoded = skein_lib::Writer::new(usize::try_from(length).expect("bounded basic header"));
    base64::encode(Alphabet::Standard, &value, &mut encoded).expect("fixture basic credential");
    Secret::new(f::parts(&[b"Basic ", &encoded.finish()]))
}
fn call(h: &mut Harness, call: &Call, authorization: Option<&Secret>) -> crate::service::Response {
    let encoded = git::client::encode(call, &crate::git::limits(&h.limits)).expect("fixture git call");
    #[expect(clippy::manual_map, reason = "step code spells Option transformations with exhaustive matches")]
    let authorization = match authorization {
        Some(secret) => Some(secret.expose()),
        None => None,
    };
    h.ask(Method::Post, &encoded.target, authorization, &encoded.body)
}
fn clone() -> Call {
    Call::Clone { repository: boxed(b"acme/widgets") }
}
fn push(commit: u64) -> Call {
    Call::Push { repository: boxed(b"acme/widgets"), branch: boxed(b"trunk"), commit, expected: Some(1) }
}
fn head(h: &Harness) -> Result<api::Answer, api::Error> {
    h.domain.inspect(&h.env.limits, b"acme/widgets", &api::Read::Branch { branch: boxed(b"trunk") })
}
fn commit(h: &mut Harness) -> u64 {
    domain::grant(&mut h.domain, b"acme/widgets", f::BOT, api::Permission::Write);
    domain::commit_tree(&mut h.domain, &h.env.limits, h.first, None, Box::new([]), b"next").expect("fixture commit")
}
#[test]
fn a_minted_token_clones_and_pushes_on_the_private_route() {
    let mut h = Harness::new();
    let mut expected = List::with_capacity(4);
    for (contents, mint) in [(true, 1), (false, 2)] {
        let token = h.mint(1, contents).token;
        assert_eq!(h.service.installations().minted(), mint);
        let authorization = basic(b"x-access-token", token.expose());
        let parent = h.first;
        let next = commit(&mut h);
        assert_eq!(call(&mut h, &clone(), Some(&authorization)).status, 200);
        let push = Call::Push {
            repository: boxed(b"acme/widgets"),
            branch: boxed(b"trunk"),
            commit: next,
            expected: Some(parent),
        };
        assert_eq!(call(&mut h, &push, Some(&authorization)).status, 200);
        assert_eq!(head(&h), Ok(api::Answer::Commit(next)));
        h.first = next;
        for writes in [false, true] {
            expected.push(GitCall { minted: Some(Minted { installation: 1, mint }), writes, status: 200 }).unwrap();
        }
    }
    assert!(h.service.git_calls().iter().eq(expected.iter()));
}
struct Row {
    authorization: Option<Secret>,
    repository: &'static [u8],
    status: u16,
    minted: Option<Minted>,
}

#[test]
fn the_private_route_refuses_each_failing_credential() {
    let limits = crate::Limits {
        installations: crate::installations::Limits { tokens: 8, ..f::limits().installations },
        ..f::limits()
    };
    let mut config = f::config();
    config.tokens = List::with_capacity(1);
    config.tokens.push((Secret::new(boxed(b"person-token")), 31, None)).unwrap();
    let mut h = Harness::with(limits, config);
    let next = commit(&mut h);
    let now = h.now;
    let valid = h.service.installations_mut().mint(1, Scope::Contents, now).unwrap().0;
    let revoked = h.service.installations_mut().mint(1, Scope::Full, now).unwrap().0;
    assert!(h.service.installations_mut().revoke(revoked.expose()));
    let expired = h.service.installations_mut().mint(1, Scope::Full, Wall::EPOCH).unwrap().0;
    // The fixture clock is before an hour; move to the expiry boundary.
    h.now = Wall::from_nanos(crate::installations::TOKEN_LIFE.as_nanos());
    assert!(h.service.installations_mut().suspend(3, false));
    let suspended = h.service.installations_mut().mint(3, Scope::Full, h.now).unwrap().0;
    assert!(h.service.installations_mut().suspend(3, true));
    let deleted = h.service.installations_mut().mint(2, Scope::Full, h.now).unwrap().0;
    assert!(h.service.installations_mut().remove(2));
    let other = h.service.installations_mut().install(23, b"example", Kind::User, Reach::All, h.now).unwrap();
    let foreign = h.service.installations_mut().mint(other, Scope::Full, h.now).unwrap().0;
    assert!(h.service.installations_mut().set_reach(1, f::listed(&[2])));
    let issued = Some(Minted { installation: 1, mint: 1 });
    let rows = [
        Row { authorization: None, repository: b"acme/widgets", status: 401, minted: None },
        Row { authorization: Some(f::bearer(valid.expose())), repository: b"acme/widgets", status: 401, minted: None },
        Row {
            authorization: Some(basic(b"someone", valid.expose())),
            repository: b"acme/widgets",
            status: 403,
            minted: issued,
        },
        Row {
            authorization: Some(basic(b"x-access-token", b"person-token")),
            repository: b"acme/widgets",
            status: 403,
            minted: None,
        },
        Row {
            authorization: Some(basic(b"x-access-token", b"unknown")),
            repository: b"acme/widgets",
            status: 403,
            minted: None,
        },
        Row {
            authorization: Some(basic(b"x-access-token", revoked.expose())),
            repository: b"acme/widgets",
            status: 403,
            minted: None,
        },
        Row {
            authorization: Some(basic(b"x-access-token", expired.expose())),
            repository: b"acme/widgets",
            status: 403,
            minted: Some(Minted { installation: 1, mint: 3 }),
        },
        Row {
            authorization: Some(basic(b"x-access-token", suspended.expose())),
            repository: b"acme/widgets",
            status: 403,
            minted: Some(Minted { installation: 3, mint: 4 }),
        },
        Row {
            authorization: Some(basic(b"x-access-token", deleted.expose())),
            repository: b"acme/widgets",
            status: 403,
            minted: None,
        },
        Row {
            authorization: Some(basic(b"x-access-token", foreign.expose())),
            repository: b"acme/widgets",
            status: 404,
            minted: Some(Minted { installation: other, mint: 6 }),
        },
        Row {
            authorization: Some(basic(b"x-access-token", valid.expose())),
            repository: b"acme/tools",
            status: 404,
            minted: issued,
        },
    ];
    for row in rows {
        let before = h.calls;
        let request = Call::Push {
            repository: boxed(row.repository),
            branch: boxed(b"trunk"),
            commit: next,
            expected: Some(h.first),
        };
        assert_eq!(call(&mut h, &request, row.authorization.as_ref()).status, row.status);
        assert_eq!(h.calls, before, "refusal never reaches the domain");
        assert_eq!(head(&h), Ok(api::Answer::Commit(h.first)));
        assert_eq!(
            h.service.git_calls().iter().last(),
            Some(&GitCall { minted: row.minted, writes: true, status: row.status })
        );
    }
}
#[test]
fn a_tokens_life_is_the_scenarios() {
    let life = Duration::from_secs(2);
    let mut h = Harness::with(f::limits(), crate::Config { token_life: life, ..f::config() });
    let start = h.now.as_nanos();
    let token = h.mint(1, true);
    assert_eq!(token.expires.as_nanos(), start.checked_add(life.as_nanos()).unwrap());
    let authorization = basic(b"x-access-token", token.token.expose());
    for (offset, status) in [(life.as_nanos().checked_sub(1).unwrap(), 200), (life.as_nanos(), 403)] {
        h.now = Wall::from_nanos(start.checked_add(offset).unwrap());
        assert_eq!(call(&mut h, &clone(), Some(&authorization)).status, status);
    }
}
#[test]
fn the_git_record_keeps_the_newest_calls() {
    let mut h = Harness::with(crate::Limits { git_records: 2, ..f::limits() }, f::config());
    let token = h.mint(1, false).token;
    let authorization = basic(b"x-access-token", token.expose());
    assert_eq!(call(&mut h, &clone(), None).status, 401);
    let first = h.first;
    assert_eq!(call(&mut h, &push(first), Some(&authorization)).status, 403);
    assert_eq!(call(&mut h, &clone(), Some(&authorization)).status, 200);
    let expected = [
        GitCall { minted: Some(Minted { installation: 1, mint: 1 }), writes: true, status: 403 },
        GitCall { minted: Some(Minted { installation: 1, mint: 1 }), writes: false, status: 200 },
    ];
    assert!(h.service.git_calls().iter().eq(expected.iter()));
}
#[test]
fn a_protected_branch_refuses_a_tokens_push_as_the_domain_does() {
    let protection = api::Protection {
        linear: false,
        code_owners: false,
        bypassing_users: Box::new([]),
        queue: false,
        branch: boxed(b"trunk"),
        contexts: Box::new([]),
        approvals: 0,
        dismiss_stale: false,
    };
    let mut h = Harness::with_protection(f::limits(), f::config(), f::domain_config(), Some(protection));
    let next = commit(&mut h);
    let token = h.mint(1, true).token;
    let authorization = basic(b"x-access-token", token.expose());
    assert_eq!(call(&mut h, &push(next), Some(&authorization)).status, 403);
    assert_eq!(head(&h), Ok(api::Answer::Commit(h.first)));
    assert_eq!(h.service.git_calls().iter().last().unwrap().status, 403);
}
#[test]
fn private_git_does_not_require_rest_headers() {
    let mut h = Harness::new();
    let token = h.mint(1, true).token;
    let authorization = basic(b"x-access-token", token.expose());
    let encoded = git::client::encode(&clone(), &crate::git::limits(&h.limits)).unwrap();
    let outcome = h.service.request(
        &Request {
            validator: None,
            method: Method::Post,
            target: &encoded.target,
            body: &encoded.body,
            authorization: Some(authorization.expose()),
            version: None,
            agent: false,
        },
        h.now,
        ReplyTo::new(Token::new(77)),
    );
    let Outcome::Call(pending, event) = outcome else { panic!("git domain call") };
    let result = h.drive(event);
    let Outcome::Reply(response) = h.service.finish(pending, result, ReplyTo::new(Token::new(77)), &h.domain) else {
        panic!("git reply")
    };
    assert_eq!(response.status, 200);
}
#[test]
fn zero_token_life_and_zero_git_records_are_refused() {
    assert_eq!(
        crate::Service::new(crate::Config { token_life: Duration::ZERO, ..f::config() }, &f::limits(), 1).err(),
        Some(crate::Problem::Config)
    );
    assert_eq!(
        crate::Service::new(f::config(), &crate::Limits { git_records: 0, ..f::limits() }, 1).err(),
        Some(crate::Problem::Limits)
    );
}
