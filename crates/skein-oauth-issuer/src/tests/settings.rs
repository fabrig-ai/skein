use super::*;
#[test]
fn settings_refuse_each_invalid_entity() {
    for (label, field, value, error) in [
        ("http outside loopback", 0_u8, b"http://issuer.example:80".as_slice(), Error::Address),
        ("empty host", 0, b"https:///path", Error::Address),
        ("userinfo", 0, b"https://user@issuer.example", Error::Address),
        ("fragment", 0, b"https://issuer.example#part", Error::Address),
        ("identifier query", 0, b"https://issuer.example?a=b", Error::Address),
        ("identifier trailing slash", 0, b"https://issuer.example/", Error::Address),
        ("loopback missing port", 0, b"http://127.0.0.1", Error::Address),
        ("loopback zero port", 0, b"http://127.0.0.1:0", Error::Address),
        ("loopback suffix", 0, b"http://localhost.evil:80", Error::Address),
        ("bad port", 0, b"https://issuer.example:65536", Error::Address),
        ("space", 1, b"https://issuer.example/a b", Error::Address),
        ("endpoint fragment", 2, b"https://issuer.example/token#x", Error::Address),
        ("userinfo endpoint", 3, b"ftp://issuer.example", Error::Address),
        ("key set endpoint", 4, b"https://", Error::Address),
        ("client id empty", 5, b"", Error::Client),
        ("client id space", 5, b"a b", Error::Client),
        ("client id control", 5, b"a\n", Error::Client),
        ("client id non-ASCII", 5, b"\xff", Error::Client),
        ("client secret empty", 6, b"", Error::Client),
        ("redirect off URL rules", 7, b"http://app.example:80", Error::Address),
    ] {
        let mut settings = settings();
        match field {
            0 => settings.issuer = boxed(value),
            1 => settings.authorization_endpoint = boxed(value),
            2 => settings.token_endpoint = boxed(value),
            3 => settings.userinfo_endpoint = boxed(value),
            4 => settings.jwks_uri = boxed(value),
            5 => settings.clients.get_mut(0).expect("client").id = boxed(value),
            6 => settings.clients.get_mut(0).expect("client").secret = secret(value),
            7 => settings.clients.get_mut(0).expect("client").redirect_uri = boxed(value),
            _ => unreachable!("table field"),
        }
        assert_eq!(Issuer::new(settings, limits()).expect_err(label), error, "{label}");
    }
}
#[test]
fn clients_are_present_unique_and_bounded_and_the_key_is_checked() {
    let mut empty = settings();
    empty.clients.clear();
    assert_eq!(Issuer::new(empty, limits()).expect_err("no client"), Error::Client);
    let mut duplicate = settings();
    duplicate
        .clients
        .push(Client { id: boxed(b"client"), secret: secret(b"other"), redirect_uri: boxed(REDIRECT) })
        .expect("slot");
    assert_eq!(Issuer::new(duplicate, limits()).expect_err("duplicate"), Error::Client);
    let mut cap = limits();
    cap.clients = 0;
    assert_eq!(Issuer::new(settings(), cap).expect_err("too many clients"), Error::Limits);
    let mut bad = settings();
    bad.key = secret(b"not a key");
    assert_eq!(Issuer::new(bad, limits()).expect_err("bad key"), Error::Key);
}
#[test]
fn endpoint_rules_accept_https_and_each_loopback_spelling() {
    for url in [
        b"https://issuer.example".as_slice(),
        b"https://issuer.example/path",
        b"http://127.0.0.1:8123",
        b"http://[::1]:8123",
        b"http://localhost:8123",
    ] {
        let mut settings = settings();
        settings.issuer = boxed(url);
        assert!(Issuer::new(settings, limits()).is_ok(), "{url:?}");
    }
}

#[test]
fn valid_accepts_test_limits() {
    assert!(valid(&limits()));
}

#[test]
fn valid_refuses_an_answer_past_u32() {
    let huge = Limits { claim_bytes: u32::MAX, ..limits() };
    assert!(!valid(&huge));
    assert_eq!(Issuer::new(settings(), huge).expect_err("answer length"), Error::Limits);
}

#[test]
fn valid_refuses_discovery_past_u32() {
    let huge = Limits { uri_bytes: 150_000_000, ..limits() };
    assert!(!valid(&huge));
    assert_eq!(Issuer::new(settings(), huge).expect_err("discovery length"), Error::Limits);
}

#[test]
fn valid_refuses_form_slots_past_u32() {
    let huge = Limits { request_bytes: u32::MAX, ..limits() };
    assert!(!valid(&huge));
    assert_eq!(Issuer::new(settings(), huge).expect_err("form slots"), Error::Limits);
}
