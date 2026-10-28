use super::*;
use crate::tests::fixtures;
use skein_lib::Wall;

#[test]
fn abort_upgrades_a_graceful_close_already_sent_to_io() {
    use crate::host::connection::Connection;
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: crate::tests::host::limits() };
    let forge = Forge {
        kind: Kind::Forgejo(fixtures::given()),
        address: skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()),
        transport: Transport::Plaintext { name: None },
    };
    let mut c = Connection::new(
        Token::new(1),
        &forge,
        b"kit",
        &skein_forge::forgejo::request::Request::User.into(),
        &fixtures::secret(fixtures::ADMIN),
        &env.limits,
    )
    .expect("connection");
    c.socket = Some(Token::new(9));
    c.fail();
    let mut io = Queue::with_capacity(4);
    assert!(c.pump(&env, &mut io));
    assert_eq!(io.pop(), Some(skein_io::Request::Close { entity: Token::new(9) }));
    c.abort();
    assert!(c.pump(&env, &mut io));
    assert_eq!(io.pop(), Some(skein_io::Request::Abort { entity: Token::new(9) }));
    assert!(!c.pump(&env, &mut io));
}
#[test]
fn a_listing_past_the_priced_page_is_refused_at_open() {
    use skein_forge::forgejo::request::{Page, Request};
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: crate::tests::host::limits() };
    let forge = Forge {
        kind: Kind::Forgejo(fixtures::given()),
        address: skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()),
        transport: Transport::Plaintext { name: None },
    };
    let send = |limit| Send {
        call: Request::Tokens { login: b"kit".as_slice().into(), page: Page { page: Some(1), limit: Some(limit) } }
            .into(),
        auth: crate::call::Auth::Token(fixtures::secret(fixtures::ADMIN)),
    };
    let inner = env.limits.inner;
    open(Token::new(1), &forge, b"kit", &send(inner), &env).expect("a page at inner is priced");
    assert_eq!(open(Token::new(1), &forge, b"kit", &send(inner + 1), &env).err(), Some(Refused::Unexpected));
}

#[test]
fn creation_heap_admission_prices_every_file_and_its_content() {
    let limits = crate::tests::host::limits();
    let mut settings = crate::tests::host::settings(&[1]);
    let first = worst_case(&limits, &settings).expect("settings admitted");
    let forge = settings.forges.get_mut(0).expect("forge");
    let Kind::Forgejo(given) = &mut forge.kind else { unreachable!() };
    given.limits.files = 2;
    given.limits.file_bytes = 8192;
    let more = worst_case(&limits, &settings).expect("larger files admitted");
    // An additional full 8192-byte file plus both paths and file records is retained.
    assert!(more >= first + 8192);
}

#[test]
fn valid_accepts_test_limits() {
    let l = crate::tests::host::limits();
    let settings = crate::tests::host::settings(&[1]);
    assert_eq!(limits::check(&l, &settings), Ok(()));
}

fn refuses_limits(l: Limits, settings: Settings) {
    assert!(limits::check(&l, &settings).is_err());
    assert!(Host::new(&l, settings, crate::tests::host::crypto()).is_err());
}

#[test]
fn valid_refuses_a_json_wire_length_past_u32() {
    let l = Limits { body_bytes_in: u32::MAX, ..crate::tests::host::limits() };
    refuses_limits(l, crate::tests::host::settings(&[1]));
}

#[test]
fn valid_refuses_an_unrepresentable_http_head() {
    let l = Limits { head_bytes: 1, ..crate::tests::host::limits() };
    refuses_limits(l, crate::tests::host::settings(&[1]));
}

#[test]
fn valid_refuses_unrepresentable_tls_intake() {
    let mut l = crate::tests::host::limits();
    l.tls.read = u32::MAX;
    refuses_limits(l, crate::tests::host::settings(&[1]));
}

#[test]
fn valid_refuses_a_key_spelling_past_u32() {
    let mut settings = crate::tests::host::github_settings();
    for index in 0..settings.forges.len() {
        let forge = settings.forges.get_mut(index).unwrap();
        if let Kind::GitHub(given) = &mut forge.kind {
            given.limits.key_bytes = u32::MAX;
        }
    }
    refuses_limits(crate::tests::host::limits(), settings);
}

#[test]
fn valid_ignores_heap_run_plans() {
    let l = Limits { runs: u32::MAX, ..crate::tests::host::limits() };
    let mut settings = crate::tests::host::settings(&[1]);
    let Kind::Forgejo(given) = &mut settings.forges.get_mut(0).unwrap().kind else { unreachable!() };
    given.limits.files = u32::MAX;
    given.limits.file_bytes = u32::MAX;
    assert_eq!(limits::check(&l, &settings), Ok(()));
}

#[test]
fn valid_refuses_installation_slots_past_u32() {
    let mut settings = crate::tests::host::github_settings();
    for index in 0..settings.forges.len() {
        let forge = settings.forges.get_mut(index).unwrap();
        if let Kind::GitHub(given) = &mut forge.kind {
            given.limits.pages = u32::MAX;
        }
    }
    refuses_limits(crate::tests::host::limits(), settings);
}
