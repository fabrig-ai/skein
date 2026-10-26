use super::{app, wall};
use crate::app::{HEADER, Refused, check};
use crate::keys::KEYS;
use skein_forge::github::jwt;
use skein_lib::{Secret, Wall, Writer, base64};
use skein_tls::sign_rs256;

fn signed(claims: &[u8], key: &[u8]) -> Secret {
    let encoded =
        base64::encoded_len(base64::Alphabet::Url, u32::try_from(claims.len()).expect("fixture claims length"))
            .expect("encoded claims length");
    let length = HEADER
        .len()
        .checked_add(1)
        .expect("separator")
        .checked_add(usize::try_from(encoded).expect("encoded length"))
        .expect("signing input length");
    let mut input = Writer::new(length);
    input.put(HEADER).expect("header");
    input.put(b".").expect("separator");
    base64::encode(base64::Alphabet::Url, claims, &mut input).expect("fixture claims fit JWT");
    let input = input.finish();
    jwt::token(&input, &sign_rs256(key, &input).expect("fixture key signs")).expect("fixture token fits")
}

#[test]
fn github_fake_jwt_accepted() {
    let app = app();
    let input = jwt::signing_input(app.id, wall(1000), wall(1600)).expect("ten minute life");
    assert!(input.starts_with(HEADER) && input.get(HEADER.len()) == Some(&b'.'), "fake and dialect header agree");
    let token = jwt::token(&input, &sign_rs256(app.key, &input).expect("fixture signature")).expect("fixture token");
    let just_before = Wall::from_nanos(wall(1600).as_nanos().checked_sub(1).expect("before expiry"));
    for (label, now, expected) in [
        ("at issuance", wall(1000), Ok(())),
        ("nanosecond before expiry", just_before, Ok(())),
        ("60 seconds early", wall(940), Ok(())),
        ("61 seconds early", wall(939), Err(Refused::Early)),
    ] {
        assert_eq!(check(token.expose(), &app, now), expected, "{label}");
    }
    for (label, claims) in [
        ("integer issuer", b"{\"iat\":1000,\"exp\":1600,\"iss\":7}".as_slice()),
        ("reordered and escaped", b"{ \"iss\":\"\\u0037\", \"exp\":1600, \"iat\":1000 }"),
        ("extra nested field", b"{\"extra\":[{\"iss\":8}],\"iat\":1000,\"exp\":1600,\"iss\":7}"),
    ] {
        let token = signed(claims, app.key);
        assert_eq!(check(token.expose(), &app, wall(1000)), Ok(()), "{label}");
    }
    // Debug redacts the key as well as Secret's webhook bytes.
    let debug = alloc::fmt::format(format_args!("{app:?}"));
    assert!(!debug.contains("test-webhook"), "webhook absent from debug text");
    assert!(debug.contains("key_bytes"), "key debug contains only its length");
}

#[test]
fn github_fake_jwt_refused() {
    let app = app();
    let valid = signed(b"{\"iat\":1000,\"exp\":1600,\"iss\":7}", app.key);
    let other_key = signed(b"{\"iat\":1000,\"exp\":1600,\"iss\":7}", KEYS.get(1).expect("second test key"));
    let mut changed = skein_lib::bytes::copy_of(valid.expose());
    // First claims byte changes while the signature stays the same.
    *changed.get_mut(HEADER.len().checked_add(1).expect("claims offset")).expect("claims byte") = b'f';
    let mut other_header = skein_lib::bytes::copy_of(valid.expose());
    *other_header.first_mut().expect("header byte") = b'f';
    let oversized = [b'x'; jwt::JWT_BYTES.saturating_add(1)];
    for (label, token, now, expected) in [
        ("no dot", b"bad".as_slice(), wall(1000), Refused::Malformed),
        ("bad base64", b"eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.e30.*", wall(1000), Refused::Malformed),
        ("another header", other_header.as_ref(), wall(1000), Refused::Header),
        ("another key", other_key.expose(), wall(1000), Refused::Signature),
        ("changed claims", changed.as_ref(), wall(1000), Refused::Signature),
        ("at expiry", valid.expose(), wall(1600), Refused::Expired),
        ("past JWT_BYTES", oversized.as_slice(), wall(1000), Refused::Malformed),
    ] {
        assert_eq!(check(token, &app, now), Err(expected), "{label}");
    }
    for (label, claims, expected) in [
        ("another issuer", b"{\"iat\":1000,\"exp\":1600,\"iss\":8}".as_slice(), Refused::Issuer),
        ("601 seconds", b"{\"iat\":1000,\"exp\":1601,\"iss\":7}", Refused::TooLong),
        ("zero life", b"{\"iat\":1000,\"exp\":1000,\"iss\":7}", Refused::TooLong),
        ("not JSON", b"not JSON", Refused::Malformed),
        ("missing exp", b"{\"iat\":1000,\"iss\":7}", Refused::Malformed),
    ] {
        let token = signed(claims, app.key);
        assert_eq!(check(token.expose(), &app, wall(1000)), Err(expected), "{label}");
    }
    for (label, issued, expires) in [("601 seconds", 1000_u64, 1601_u64), ("zero life", 1000, 1000)] {
        assert_eq!(jwt::signing_input(app.id, wall(issued), wall(expires)), None, "{label}: hand signing required");
    }
}

#[test]
fn github_fake_jwt_claim_shapes() {
    let app = app();
    for (label, claims, expected) in [
        ("array", b"[1000,1600,7]".as_slice(), Refused::Malformed),
        ("duplicate exp", b"{\"iat\":1000,\"exp\":1600,\"exp\":1500,\"iss\":7}", Refused::Malformed),
        ("nested expiry", b"{\"iat\":1000,\"exp\":{},\"iss\":7}", Refused::Malformed),
        ("boolean expiry", b"{\"iat\":1000,\"exp\":true,\"iss\":7}", Refused::Malformed),
    ] {
        let token = signed(claims, app.key);
        assert_eq!(check(token.expose(), &app, wall(1000)), Err(expected), "{label}");
    }
    // Malformed claims with a forged signature must stop at Signature.
    let invalid_claims = signed(b"not JSON", KEYS.get(1).expect("second test key"));
    assert_eq!(check(invalid_claims.expose(), &app, wall(1000)), Err(Refused::Signature));
}

#[test]
fn github_fake_jwt_claim_values() {
    let app = app();
    for (label, claims, expected) in [
        ("negative life", b"{\"iat\":1000,\"exp\":999,\"iss\":7}".as_slice(), Refused::TooLong),
        ("fractional issuance", b"{\"iat\":1000.0,\"exp\":1600,\"iss\":7}", Refused::Malformed),
        ("string expiry", b"{\"iat\":1000,\"exp\":\"1600\",\"iss\":7}", Refused::Malformed),
        ("integer overflow", b"{\"iat\":1000,\"exp\":18446744073709551616,\"iss\":7}", Refused::Malformed),
    ] {
        let token = signed(claims, app.key);
        assert_eq!(check(token.expose(), &app, wall(1000)), Err(expected), "{label}");
    }
}

#[test]
fn github_fake_cached_signature_checks_claims_and_changed_bytes() {
    let mut app = app();
    let token = signed(b"{\"iat\":1000,\"exp\":1600,\"iss\":7}", app.key);
    let mut verified = crate::app::Verified::empty();
    assert_eq!(verified.check(token.expose(), &app, wall(1000)), Ok(()));
    for (now, expected) in [
        (wall(1000), Ok(())),
        (wall(1599), Ok(())),
        (wall(1600), Err(Refused::Expired)),
        (wall(939), Err(Refused::Early)),
    ] {
        assert_eq!(verified.check(token.expose(), &app, now), expected);
    }
    app.id = 8;
    assert_eq!(verified.check(token.expose(), &app, wall(1000)), Err(Refused::Issuer));
    app.id = 7;
    let mut changed = skein_lib::bytes::copy_of(token.expose());
    *changed.get_mut(HEADER.len().checked_add(1).expect("claims offset")).expect("claims byte") = b'f';
    assert_eq!(verified.check(&changed, &app, wall(1000)), Err(Refused::Signature));
    let mut changed_signature = skein_lib::bytes::copy_of(token.expose());
    let at = changed_signature.len().checked_sub(2).expect("signature byte");
    let byte = changed_signature.get_mut(at).expect("signature byte");
    *byte = if *byte == b'A' { b'B' } else { b'A' };
    assert_eq!(verified.check(&changed_signature, &app, wall(1000)), Err(Refused::Signature));
    let debug = alloc::fmt::format(format_args!("{verified:?}"));
    assert!(skein_lib::bytes::find(debug.as_bytes(), token.expose()).is_none());
    app.key = KEYS.get(1).expect("second key");
    assert_eq!(verified.check(token.expose(), &app, wall(1000)), Err(Refused::Signature));
    let next = signed(b"{\"iat\":1000,\"exp\":1600,\"iss\":7}", app.key);
    assert_eq!(verified.check(next.expose(), &app, wall(1000)), Ok(()));
    assert_eq!(verified.check(token.expose(), &app, wall(1000)), Err(Refused::Signature));
}

#[test]
fn github_fake_cached_claims_keep_refusing_invalid_values() {
    let app = app();
    for (label, claims, now, expected) in [
        ("another issuer", b"{\"iat\":1000,\"exp\":1600,\"iss\":8}".as_slice(), wall(1000), Refused::Issuer),
        ("601 seconds", b"{\"iat\":1000,\"exp\":1601,\"iss\":7}", wall(1000), Refused::TooLong),
        ("zero life", b"{\"iat\":1000,\"exp\":1000,\"iss\":7}", wall(1000), Refused::TooLong),
        ("negative life", b"{\"iat\":1000,\"exp\":999,\"iss\":7}", wall(1000), Refused::TooLong),
        ("at expiry", b"{\"iat\":1000,\"exp\":1600,\"iss\":7}", wall(1600), Refused::Expired),
        ("61 seconds early", b"{\"iat\":1000,\"exp\":1600,\"iss\":7}", wall(939), Refused::Early),
    ] {
        let token = signed(claims, app.key);
        let mut verified = crate::app::Verified::empty();
        assert_eq!(verified.check(token.expose(), &app, now), Err(expected), "{label}: miss");
        assert_eq!(verified.check(token.expose(), &app, now), Err(expected), "{label}: hit");
    }
}
