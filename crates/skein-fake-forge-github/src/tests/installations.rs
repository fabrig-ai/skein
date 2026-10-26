use super::{store, wall};
use crate::installations::{
    Denied, Grant, Install, Installations, Kind, LOGIN_BYTES, Limits, Mint, Reach, Scope, TOKEN_LIFE,
};
use skein_lib::{List, Wall};

#[test]
fn github_fake_installations() {
    let mut rows = store(42);
    assert_eq!(rows.install(2, b"org", Kind::Organization, Reach::All, wall(1001)), Ok(2));
    {
        let mut listed = rows.iter();
        let first = listed.next().expect("first installation");
        assert_eq!((first.id, first.account, first.kind, first.created), (1, 1, Kind::User, wall(1000)));
        assert!(first.login.as_ref() == b"person", "login retained");
        assert_eq!(listed.next().expect("second installation").id, 2);
        assert!(listed.next().is_none(), "two installations in id order");
    }
    assert_eq!(rows.install(3, b"third", Kind::User, Reach::All, wall(1000)), Err(Install::Full));
    let (first_token, _) = rows.mint(1, Scope::Full, wall(1000)).expect("first token");
    let (second_token, _) = rows.mint(2, Scope::Contents, wall(1000)).expect("second token");
    assert!(rows.suspend(1, true), "suspend existing installation");
    assert!(rows.get(1).expect("first installation").suspended, "suspension retained");
    assert_eq!(rows.mint(1, Scope::Full, wall(1000)), Err(Mint::Suspended));
    assert_eq!(rows.token(first_token.expose(), wall(1000)), Ok(&Grant { installation: 1, scope: Scope::Full }));
    assert!(rows.suspend(1, false), "resume existing installation");
    assert!(!rows.get(1).expect("first installation").suspended, "suspension cleared");
    assert!(rows.remove(1), "remove first installation");
    assert!(rows.get(1).is_none(), "removed installation absent");
    assert_eq!(rows.token(first_token.expose(), wall(1000)), Err(Denied::Unknown));
    assert_eq!(rows.token(second_token.expose(), wall(1000)), Ok(&Grant { installation: 2, scope: Scope::Contents }));
    assert!(!rows.remove(1), "absent removal reports false");
    assert!(!rows.suspend(9, true), "absent suspension reports false");
    assert_eq!(rows.install(3, b"third", Kind::User, Reach::All, wall(1002)), Ok(3));
    assert_eq!(rows.iter().next().expect("remaining row").id, 2);
    assert_eq!(rows.iter().nth(1).expect("new row").id, 3);
    assert!(!rows.reaches(&Grant { installation: 1, scope: Scope::Full }, 1), "removed grant reaches nothing");

    let mut rows = Installations::new(&Limits { installations: 1, tokens: 0, listed: 1 }, 0);
    for (label, login) in [("empty", b"".as_slice()), ("too long", &[b'x'; LOGIN_BYTES.saturating_add(1)])] {
        assert_eq!(rows.install(1, login, Kind::User, Reach::All, wall(0)), Err(Install::Login), "{label}");
    }
    let mut repositories = List::with_capacity(2);
    repositories.push(1).expect("first repository");
    repositories.push(2).expect("second repository");
    assert_eq!(rows.install(1, b"person", Kind::User, Reach::Listed(repositories), wall(0)), Err(Install::Full));
    assert_eq!(rows.install(1, &[b'x'; LOGIN_BYTES], Kind::User, Reach::All, wall(0)), Ok(1));
    assert_eq!(rows.mint(1, Scope::Full, wall(0)), Err(Mint::Full));
}

#[test]
fn github_fake_installations_set_reach() {
    let mut rows = store(42);
    let grant = Grant { installation: 1, scope: Scope::Full };
    assert!(rows.set_reach(1, super::fixtures::listed(&[2, 4])), "replace within listed bound");
    assert!(rows.reaches(&grant, 2), "new reach includes listed id");
    assert!(!rows.reaches(&grant, 3), "new reach excludes unlisted id");
    assert!(!rows.set_reach(1, super::fixtures::listed(&[2, 4, 6])), "over listed bound refused");
    assert!(rows.reaches(&grant, 2), "refusal preserves reach");
    assert!(!rows.set_reach(99, Reach::All), "absent id refused");
    assert!(rows.set_reach(1, Reach::All), "replace by All");
    assert!(rows.reaches(&grant, 3), "All reaches other repositories");
    assert!(rows.set_reach(1, super::fixtures::listed(&[])), "empty listed reach admitted");
    assert!(!rows.reaches(&grant, 2), "empty reach grants nothing");
}

#[test]
fn github_fake_tokens() {
    let mut rows = store(42);
    let mut replay = store(42);
    let mut other = store(43);
    let (full, expiry) = rows.mint(1, Scope::Full, wall(1000)).expect("full token");
    let (contents, contents_expiry) = rows.mint(1, Scope::Contents, wall(1001)).expect("contents token");
    assert_eq!(expiry.as_nanos().checked_sub(wall(1000).as_nanos()), Some(TOKEN_LIFE.as_nanos()));
    assert_eq!(contents_expiry, wall(4601));
    for token in [&full, &contents] {
        assert_eq!(token.len(), 40);
        assert!(token.expose().starts_with(b"ghs_"), "token prefix");
        for byte in token.expose().get(4..).expect("token suffix") {
            assert!(byte.is_ascii_alphanumeric(), "token alphabet");
        }
    }
    assert_eq!(replay.mint(1, Scope::Full, wall(1000)).expect("replay").0, full);
    assert_eq!(replay.mint(1, Scope::Contents, wall(1001)).expect("replay second").0, contents);
    assert!(other.mint(1, Scope::Full, wall(1000)).expect("another seed").0 != full, "seed changes token text");
    assert_eq!(rows.token(full.expose(), wall(1000)), Ok(&Grant { installation: 1, scope: Scope::Full }));
    assert_eq!(rows.token(contents.expose(), wall(1001)), Ok(&Grant { installation: 1, scope: Scope::Contents }));
    assert_eq!(
        rows.token(full.expose(), Wall::from_nanos(expiry.as_nanos().checked_sub(1).expect("before expiry"))),
        Ok(&Grant { installation: 1, scope: Scope::Full })
    );
    assert_eq!(rows.token(full.expose(), expiry), Err(Denied::Expired));
    assert_eq!(rows.token(b"unknown", wall(1000)), Err(Denied::Unknown));
    assert_eq!(rows.mint(1, Scope::Full, wall(1002)), Err(Mint::Full));
    let (replacement, _) = rows.mint(1, Scope::Full, expiry).expect("expired token reclaimed");
    assert_eq!(rows.token(full.expose(), expiry), Err(Denied::Unknown));
    assert_eq!(rows.token(contents.expose(), expiry), Ok(&Grant { installation: 1, scope: Scope::Contents }));
    assert!(rows.revoke(contents.expose()), "revoke known token");
    assert!(!rows.revoke(contents.expose()), "revoke absent token");
    assert_eq!(rows.token(contents.expose(), expiry), Err(Denied::Unknown));
    assert!(rows.revoke(replacement.expose()), "remove last token");
    assert!(rows.suspend(1, true), "suspend installation");
    assert_eq!(rows.mint(1, Scope::Contents, wall(1000)), Err(Mint::Suspended));
    assert_eq!(rows.mint(99, Scope::Full, wall(1000)), Err(Mint::Missing));
    assert!(rows.suspend(1, false), "resume installation");
    assert_eq!(rows.mint(1, Scope::Full, Wall::from_nanos(u64::MAX)), Err(Mint::Full));
    let (fractional, fractional_expiry) = rows.mint(1, Scope::Full, Wall::from_nanos(123)).expect("fractional clock");
    assert_eq!(fractional_expiry.as_nanos().checked_sub(123), Some(TOKEN_LIFE.as_nanos()));
    assert!(rows.revoke(fractional.expose()), "revoke fractional token");
    let mut repositories = List::with_capacity(8);
    repositories.push(3).expect("listed repository");
    repositories.push(5).expect("listed repository");
    assert_eq!(rows.install(2, b"org", Kind::Organization, Reach::Listed(repositories), wall(1000)), Ok(2));
    for scope in [Scope::Full, Scope::Contents] {
        assert!(rows.reaches(&Grant { installation: 1, scope }, 99), "All reaches any repository");
        assert!(rows.reaches(&Grant { installation: 2, scope }, 3), "Listed reaches its repository");
        assert!(!rows.reaches(&Grant { installation: 2, scope }, 4), "Listed excludes other repositories");
    }
    drop(rows.mint(1, Scope::Full, wall(1000)).expect("token retained for debug check"));
    let debug = alloc::fmt::format(format_args!("{rows:?}"));
    assert!(!debug.contains("ghs_"), "debug does not expose token text");
}
