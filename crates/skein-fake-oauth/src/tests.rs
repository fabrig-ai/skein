fn copies(ids: &[&[u8]]) -> Box<[Box<[u8]>]> {
    let mut out = List::with_capacity(u32::try_from(ids.len()).expect("fixture length"));
    for id in ids {
        out.push(bytes::copy_of(id)).expect("capacity");
    }
    out.into_boxed()
}
use super::*;

fn limits() -> Limits {
    Limits {
        document: DocumentLimits {
            document_bytes: 4096,
            string_bytes: 1024,
            token_bytes: 256,
            client_bytes: 64,
            detail_bytes: 64,
            record_bytes: 2048,
            depth: 8,
            tokens: 128,
            issued_client_bytes: 64,
            granted_bytes: 64,
            subject_bytes: 64,
            id_token_bytes: 1024,
        },
        uri_bytes: 256,
        request_bytes: 2048,
        codes: 8,
        rotations: 8,
        plans: 8,
        carried: 8,
    }
}
fn config() -> Config {
    Config {
        authorization_url: bytes::copy_of(b"https://issuer.example/authorize"),
        token_endpoint: bytes::copy_of(b"https://issuer.example/token"),
        client_id: bytes::copy_of(b"client"),
        client_secret: Some(bytes::copy_of(b"secret")),
        redirect_uri: bytes::copy_of(b"http://127.0.0.1:1234/callback"),
        refresh_token: bytes::copy_of(b"seed"),
        terms: None,
    }
}
fn terms(ids: &[&[u8]]) -> Terms {
    Terms {
        issued: copies(ids),
        omit_issued: false,
        revocation_endpoint: Some(bytes::copy_of(b"https://issuer.example/revoke")),
        varying_port: false,
        openid: None,
    }
}
fn issuer(terms: Terms) -> Issuer {
    let mut config = config();
    config.terms = Some(terms);
    Issuer::new(config, limits()).expect("issuer")
}
fn step(issuer: &mut Issuer, event: Event) -> Option<Request> {
    let mut out = Queue::uncapped();
    issuer.step(event, &mut out);
    out.pop()
}
fn authorize(issuer: &mut Issuer) -> Option<Request> {
    authorization(issuer, b"http://127.0.0.1:1234/callback", b"")
}
fn authorization(issuer: &mut Issuer, redirect: &[u8], extra: &[u8]) -> Option<Request> {
    let url = joined(&[
        b"https://issuer.example/authorize?response_type=code&client_id=client&redirect_uri=",
        redirect,
        b"&state=0123456789abcdef",
        extra,
    ]);
    step(issuer, Event::Authorize { url, now: Time::ZERO })
}
fn plan(status: u16, delay: u64) -> Plan {
    Plan {
        status,
        body: Body::Raw(bytes::copy_of(b"{}")),
        delay: Duration::from_secs(delay),
        retry_after: Duration::ZERO,
    }
}
fn post(issuer: &mut Issuer, endpoint: &[u8], body: &[u8]) -> Option<Request> {
    step(
        issuer,
        Event::Post {
            request: HttpRequest {
                method: skein_oauth::Method::Post,
                id: 1,
                endpoint: bytes::copy_of(endpoint),
                content_type: b"application/x-www-form-urlencoded",
                body: bytes::copy_of(body),
                deadline: Time::ZERO.saturating_add(Duration::from_secs(10)),
            },
            now: Time::ZERO,
            wall: Wall::EPOCH,
        },
    )
}
fn token_post(issuer: &mut Issuer, body: &[u8]) -> Option<Request> {
    post(issuer, b"https://issuer.example/token", body)
}
fn revoke(issuer: &mut Issuer) -> Option<Request> {
    post(
        issuer,
        b"https://issuer.example/revoke",
        b"token=seed&token_type_hint=refresh_token&client_id=client&client_secret=secret",
    )
}
fn refresh(issuer: &mut Issuer) -> Option<Request> {
    token_post(issuer, b"grant_type=refresh_token&refresh_token=seed&client_id=client&client_secret=secret")
}
fn status(request: Option<Request>) -> u16 {
    let Some(Request::Http(answer)) = request else { panic!("HTTP answer") };
    answer.status
}
#[test]
fn an_issued_registration_redirects_with_each_issued_id() {
    let mut issuer = issuer(terms(&[b"client-1", b"client-2"]));
    for expected in [b"client-1".as_slice(), b"client-2", b"client-2"] {
        let Some(Request::Redirect { client_id, .. }) = authorize(&mut issuer) else { panic!("redirect expected") };
        assert_eq!(client_id.as_deref(), Some(expected));
    }
}
#[test]
fn an_omitted_issue_leaves_the_redirect_without_a_client_id() {
    let mut terms = terms(&[b"client-1"]);
    terms.omit_issued = true;
    let Some(Request::Redirect { client_id, .. }) = authorize(&mut issuer(terms)) else { panic!("redirect") };
    assert_eq!(client_id, None);
}
#[test]
fn a_later_request_without_the_latest_issued_id_is_refused() {
    for (endpoint, body) in [
        (b"https://issuer.example/token".as_slice(), b"grant_type=authorization_code&code=0000000000000001&redirect_uri=http://127.0.0.1:1234/callback&client_id=client&client_secret=secret".as_slice()),
        (b"https://issuer.example/token", b"grant_type=refresh_token&refresh_token=seed&client_id=client&client_secret=secret"),
        (b"https://issuer.example/revoke", b"token=seed&token_type_hint=refresh_token&client_id=client&client_secret=secret"),
    ] {
        let mut issuer = issuer(terms(&[b"client-1"])); drop(authorize(&mut issuer));
        let Some(Request::Http(answer)) = post(&mut issuer, endpoint, body) else { panic!("answer") };
        assert_eq!(answer.status, 400);
        assert!(find(&answer.body, b"invalid_grant").is_some());
    }
}
#[test]
fn carried_names_each_added_name_in_order() {
    let mut issuer = issuer(terms(&[]));
    drop(authorization(
        &mut issuer,
        b"http://127.0.0.1:1234/callback",
        b"&added=a&resource=b&nonce=c&id_token_hint=d&login_hint=e",
    ));
    let names = issuer.carried();
    assert_eq!(names.len(), 5);
    for (name, expected) in
        names.iter().zip([b"added".as_slice(), b"resource", b"nonce", b"id_token_hint", b"login_hint"])
    {
        assert_eq!(name.as_ref(), expected);
    }
}
#[test]
fn carried_names_stop_at_their_bound() {
    const NAMES: u32 = 2; // Two scripted added fields fit; the third exceeds the bound.
    for (extra, accepted) in [(b"&a=1&b=2".as_slice(), true), (b"&a=1&b=2&c=3", false)] {
        let mut limits = limits();
        limits.carried = NAMES;
        let mut issuer = Issuer::new(config(), limits).expect("issuer");
        let answer = authorization(&mut issuer, b"http://127.0.0.1:1234/callback", extra);
        assert_eq!(is_redirect(answer), accepted);
    }
}
#[test]
fn a_varying_port_redirect_is_held_for_its_exchange() {
    for (configured, bound) in [
        (b"http://127.0.0.1:0/callback".as_slice(), b"http://127.0.0.1:5678/callback".as_slice()),
        (b"http://[::1]:0/callback", b"http://[::1]:5678/callback"),
    ] {
        let mut config = config();
        config.redirect_uri = bytes::copy_of(configured);
        let mut terms = terms(&[]);
        terms.varying_port = true;
        config.terms = Some(terms);
        let mut issuer = Issuer::new(config, limits()).expect("issuer");
        issuer.queue(plan(200, 0)).expect("plan");
        let Some(Request::Redirect { uri, code, .. }) = authorization(&mut issuer, bound, b"") else {
            panic!("redirect")
        };
        assert_eq!(uri.as_ref(), bound);
        let body = joined(&[
            b"grant_type=authorization_code&client_id=client&client_secret=secret&code=",
            &code,
            b"&redirect_uri=",
            bound,
        ]);
        assert_eq!(status(token_post(&mut issuer, &body)), 200);
    }
}
#[test]
fn a_varying_port_is_refused_off_loopback_or_at_exchange() {
    for uri in [b"http://localhost:5678/callback".as_slice(), b"http://127.0.0.1:5678/other"] {
        let mut terms = terms(&[]);
        terms.varying_port = true;
        let mut issuer = issuer(terms);
        assert!(is_refused(authorization(&mut issuer, uri, b"")));
    }
    let mut terms = terms(&[]);
    terms.varying_port = true;
    let mut issuer = issuer(terms);
    drop(authorization(&mut issuer, b"http://127.0.0.1:5678/callback", b""));
    assert_eq!(status(token_post(&mut issuer, b"grant_type=authorization_code&client_id=client&client_secret=secret&code=0000000000000001&redirect_uri=http://127.0.0.1:9999/callback")), 400);
}
#[test]
fn a_confirmed_revocation_spends_the_refresh_token() {
    let mut issuer = issuer(terms(&[]));
    issuer.queue(plan(200, 0)).expect("plan");
    drop(revoke(&mut issuer));
    assert_eq!(status(refresh(&mut issuer)), 400);
}
#[test]
fn an_unconfirmed_revocation_leaves_the_refresh_token() {
    for (status_code, delay) in [(400, 0), (503, 0), (200, 20)] {
        let mut issuer = issuer(terms(&[]));
        issuer.queue(plan(status_code, delay)).expect("plan");
        drop(revoke(&mut issuer));
        drop(step(&mut issuer, Event::LoseResponse));
        issuer.queue(plan(200, 0)).expect("plan");
        assert_eq!(status(refresh(&mut issuer)), 200);
    }
}
#[test]
fn granted_lists_queued_tokens_in_answer_order() {
    const GRANTS: usize = 3; // Three successful token plans below.
    let mut issuer = issuer(terms(&[]));
    for access in [b"access-1".as_slice(), b"access-2", b"access-3"] {
        issuer.queue(plan(400, 0)).expect("error");
        issuer
            .queue(Plan {
                status: 200,
                body: Body::Token(TokenResponse {
                    access_token: bytes::copy_of(access),
                    refresh_token: None,
                    expires_in: 30,
                    scope: None,
                    id_token: None,
                    earliest_refresh_at: None,
                }),
                delay: Duration::ZERO,
                retry_after: Duration::ZERO,
            })
            .expect("token");
    }
    let tokens = issuer.granted();
    assert_eq!(tokens.len(), GRANTS);
    for (index, token) in tokens.iter().enumerate() {
        assert!(
            token == &bytes::copy_of([b"access-1".as_slice(), b"access-2", b"access-3"].get(index).expect("index"))
        );
    }
}

fn is_redirect(answer: Option<Request>) -> bool {
    match answer {
        Some(Request::Redirect { .. }) => true,
        Some(Request::Http(_) | Request::Refused) | None => false,
    }
}
fn is_refused(answer: Option<Request>) -> bool {
    match answer {
        Some(Request::Refused) => true,
        Some(Request::Http(_) | Request::Redirect { .. }) | None => false,
    }
}

fn joined(parts: &[&[u8]]) -> Box<[u8]> {
    let mut size = 0_usize;
    for part in parts {
        size = size.checked_add(part.len()).expect("fixture fits");
    }
    let mut out = Writer::new(size);
    for part in parts {
        out.put(part).expect("measured");
    }
    out.finish()
}

use skein_lib::base64::{Alphabet, decode};
use skein_lib::bytes::{find, find_from};
use skein_tls::verify_rs256;

fn openid_terms(fault: Option<Fault>) -> Terms {
    let mut terms = terms(&[]);
    terms.openid = Some(OpenId {
        issuer: bytes::copy_of(b"https://issuer.example"),
        key_set_url: bytes::copy_of(b"https://issuer.example/keys"),
        subject: bytes::copy_of(b"subject"),
        lifetime: Duration::from_secs(60),
        header_kid: true,
        fault,
    });
    terms
}
fn token_plan(scripted: Option<&[u8]>) -> Plan {
    Plan {
        status: 200,
        body: Body::Token(TokenResponse {
            access_token: bytes::copy_of(b"access"),
            refresh_token: None,
            expires_in: 60,
            scope: None,
            id_token: copy_optional(scripted),
            earliest_refresh_at: None,
        }),
        delay: Duration::ZERO,
        retry_after: Duration::ZERO,
    }
}
fn exchange(issuer: &mut Issuer) -> Box<[u8]> {
    let Some(Request::Redirect { code, .. }) =
        authorization(issuer, b"http://127.0.0.1:1234/callback", b"&nonce=sign-in-nonce")
    else {
        panic!("redirect")
    };
    exchange_code(issuer, &code)
}
fn exchange_code(issuer: &mut Issuer, code: &[u8]) -> Box<[u8]> {
    let body = joined(&[b"grant_type=authorization_code&client_id=client&client_secret=secret&redirect_uri=http://127.0.0.1:1234/callback&code=", code]);
    let Some(Request::Http(answer)) = step(
        issuer,
        Event::Post {
            request: HttpRequest {
                method: skein_oauth::Method::Post,
                id: 1,
                endpoint: bytes::copy_of(b"https://issuer.example/token"),
                content_type: b"application/x-www-form-urlencoded",
                body,
                deadline: Time::ZERO,
            },
            now: Time::ZERO,
            wall: Wall::from_nanos(100_000_000_000),
        },
    ) else {
        panic!("token answer")
    };
    answer.body
}
fn id_token(issuer: &mut Issuer) -> Box<[u8]> {
    issuer.queue(token_plan(None)).expect("plan");
    member(&exchange(issuer), b"id_token", 0).expect("ID token")
}
fn member(body: &[u8], name: &[u8], occurrence: usize) -> Option<Box<[u8]>> {
    let json = Json::from_bytes(body, &limits().document).expect("JSON fixture");
    let mut found = 0_usize;
    for pair in json.as_tokens().windows(2) {
        if let Some(Token::Key(key)) = pair.first()
            && key.as_ref() == name
        {
            if found == occurrence {
                return match pair.get(1) {
                    Some(Token::String(value) | Token::Number(value)) => Some(value.clone()),
                    Some(Token::True) => Some(bytes::copy_of(b"true")),
                    Some(Token::False) => Some(bytes::copy_of(b"false")),
                    Some(
                        Token::ObjectStart
                        | Token::ObjectEnd
                        | Token::ArrayStart
                        | Token::ArrayEnd
                        | Token::Key(_)
                        | Token::Null,
                    )
                    | None => None,
                };
            }
            found = found.checked_add(1).expect("fixture count");
        }
    }
    None
}
fn segment(token: &[u8], part: usize) -> Box<[u8]> {
    let first = find(token, b".").expect("header");
    let second = find_from(token, b".", first.checked_add(1).expect("in range")).expect("payload");
    let text = match part {
        0 => token.get(..first).expect("header"),
        1 => token.get(first.checked_add(1).expect("in range")..second).expect("payload"),
        _ => token.get(second.checked_add(1).expect("in range")..).expect("signature"),
    };
    decode(Alphabet::Url, text, limits().document.id_token_bytes).expect("base64url")
}
fn get(issuer: &mut Issuer, endpoint: &[u8]) -> HttpResponse {
    let Some(Request::Http(answer)) =
        step(issuer, Event::Get { id: 7, endpoint: bytes::copy_of(endpoint), now: Time::ZERO, wall: Wall::EPOCH })
    else {
        panic!("GET answer")
    };
    answer
}
#[test]
fn the_signature_verifies_against_the_published_key() {
    let token = id_token(&mut issuer(openid_terms(None)));
    let first = find(&token, b".").expect("a header");
    let cut = find_from(&token, b".", first.checked_add(1).expect("in range")).expect("three parts");
    let (input, dotted) = token.split_at_checked(cut).expect("in range");
    let signature = decode(Alphabet::Url, dotted.strip_prefix(b".").expect("dot"), 1024).expect("signature");
    let n = decode(Alphabet::Url, key::MODULUS.as_bytes(), 1024).expect("modulus");
    let e = decode(Alphabet::Url, b"AQAB", 1024).expect("exponent");
    assert!(verify_rs256(&n, &e, input, &signature));
    assert!(!verify_rs256(&n, &e, b"other", &signature));
}
#[test]
fn signing_the_same_claims_twice_gives_equal_bytes() {
    let terms = openid_terms(None);
    let openid = terms.openid.as_ref().expect("OpenID");
    let account = Account { subject: openid.subject.clone(), email: None, name: None };
    let first =
        openid::token(openid, &limits(), b"client", &account, Some(b"sign-in-nonce"), Wall::EPOCH).expect("token");
    let second =
        openid::token(openid, &limits(), b"client", &account, Some(b"sign-in-nonce"), Wall::EPOCH).expect("token");
    assert!(first == second);
}
#[test]
fn the_id_token_names_each_claim() {
    let token = id_token(&mut issuer(openid_terms(None)));
    let claims = segment(&token, 1);
    for (name, expected) in [
        (b"iss".as_slice(), b"https://issuer.example".as_slice()),
        (b"aud", b"client"),
        (b"sub", b"subject"),
        (b"exp", b"160"),
    ] {
        assert_eq!(member(&claims, name, 0).as_deref(), Some(expected));
    }
    assert!(member(&claims, b"nonce", 0).is_some());
}
#[derive(Clone, Copy)]
enum Source {
    Header,
    Claims,
    Metadata,
    Keys,
    Answer,
}
fn fault_document(fault: Fault, source: Source) -> Box<[u8]> {
    let mut issuer = issuer(openid_terms(Some(fault)));
    match source {
        Source::Header => {
            openid::header(openid::configured(&issuer.config).expect("OpenID"), &limits()).expect("header")
        }
        Source::Claims => openid::claims(
            openid::configured(&issuer.config).expect("OpenID"),
            &limits(),
            b"client",
            issuer.account.as_ref().expect("account"),
            Some(b"sign-in-nonce"),
            Wall::from_nanos(100_000_000_000),
        )
        .expect("claims"),
        Source::Metadata => get(&mut issuer, b"https://issuer.example/.well-known/openid-configuration").body,
        Source::Keys => get(&mut issuer, b"https://issuer.example/keys").body,
        Source::Answer => {
            issuer.queue(token_plan(None)).expect("plan");
            exchange(&mut issuer)
        }
    }
}
type FaultRow<'a> = (Fault, Source, Option<&'a [u8]>, usize, Option<&'a [u8]>);
#[test]
fn each_fault_changes_its_document_or_claim() {
    let rows: &[FaultRow<'_>] = &[
        (Fault::UnknownKid, Source::Header, Some(b"kid"), 0, Some(b"unknown")),
        (Fault::AnotherKey, Source::Keys, Some(b"n"), 0, Some(b"vpPDmF4kBBqSO-YVej0H6gZ7tUpVAo-h7OdVVem1crN-Gqjqn76PfCjcptgwGJcE6BHbT-Ul_Flx422gJND8PD1bN9IVXwH_OWTBMayIJ0VMZx6BC1rpMdKv2fZ2M5i7f8K2N1xMukrqbQDSBdvlAQRyS3P42bxYAr01JuAN-jGfkof3TM8ppVc02xktHteKgUWMGrt8rM1ZqeOG3d7CnjsYbM-6REOixJy2ZAj2ogBt5d1YQLuGEdzEUY-HkhJ_a5zYftIH73FjqdJhh8xoSsDNvfkvTD9FfsQ6o7UzpfzucQwuLDlmJvWIf69dAjqEEzQWKuXNw2XeIxrOzjg_tQ")),
        (Fault::AlgNone, Source::Header, Some(b"alg"), 0, Some(b"none")),
        (Fault::AlgHs256, Source::Header, Some(b"alg"), 0, Some(b"HS256")),
        (Fault::TwoKeysNoKid, Source::Header, Some(b"kid"), 0, None),
        (Fault::TwoKeysNoKid, Source::Keys, Some(b"kid"), 1, Some(b"second")),
        (Fault::Issuer, Source::Claims, Some(b"iss"), 0, Some(b"https://other.example")),
        (Fault::Audience, Source::Claims, Some(b"aud"), 0, Some(b"other-client")),
        (Fault::Azp, Source::Claims, Some(b"azp"), 0, Some(b"other-client")),
        (Fault::TwoAudiences, Source::Claims, Some(b"azp"), 0, Some(b"client")),
        (Fault::Expired(Duration::from_secs(10)), Source::Claims, Some(b"exp"), 0, Some(b"90")),
        (Fault::Nonce, Source::Claims, Some(b"nonce"), 0, Some(b"other-nonce")),
        (Fault::NoSubject, Source::Claims, Some(b"sub"), 0, None),
        (Fault::NoIdToken, Source::Answer, Some(b"id_token"), 0, None),
        (Fault::LongModulus, Source::Keys, Some(b"n"), 0, Some(b"vpPDmF4kBBqSO-YVej0H6gZ7tUpVAo-h7OdVVem1crN-Gqjqn76PfCjcptgwGJcE6BHbT-Ul_Flx422gJND8PD1bN9IVXwH_OWTBMayIJ0VMZx6BC1rpMdKv2fZ2M5i7f8K2N1xMukrqbQDSBdvlAQRyS3P42bxYAr01JuAN-jGfkof3TM8ppVc02xktHteKgUWMGrt8rM1ZqeOG3d7CnjsYbM-6REOixJy2ZAj2ogBt5d1YQLuGEdzEUY-HkhJ_a5zYftIH73FjqdJhh8xoSsDNvfkvTD9FfsQ6o7UzpfzucQwuLDlmJvWIf69dAjqEEzQWKuXNw2XeIxrOzjg_t76Tw5heJAQakjvmFXo9B-oGe7VKVQKPoeznVVXptXKzfhqo6p--j3wo3KbYMBiXBOgR20_lJfxZceNtoCTQ_Dw9WzfSFV8B_zlkwTGsiCdFTGcegQta6THSr9n2djOYu3_CtjdcTLpK6m0A0gXb5QEEcktz-Nm8WAK9NSbgDfoxn5KH90zPKaVXNNsZLR7XioFFjBq7fKzNWanjht3ewp47GGzPukRDosSctmQI9qIAbeXdWEC7hhHcxFGPh5ISf2uc2H7SB-9xY6nSYYfMaErAzb35L0w_RX7EOqO1M6X87nEMLiw5Zib1iH-vXQI6hBM0FirlzcNl3iMazs44P7c")),
        (Fault::UseEnc, Source::Keys, Some(b"use"), 0, Some(b"enc")),
        (Fault::ExtraEcKey, Source::Keys, Some(b"kty"), 1, Some(b"EC")),
        (Fault::UnreadableMetadata, Source::Metadata, None, 0, Some(b"not JSON")),
        (Fault::UnreadableKeySet, Source::Keys, None, 0, Some(b"not JSON")),
        (Fault::MetadataIssuer, Source::Metadata, Some(b"issuer"), 0, Some(b"https://other.example")),
    ];
    for &(fault, source, name, occurrence, expected) in rows {
        let document = fault_document(fault, source);
        let value = match name {
            Some(name) => member(&document, name, occurrence),
            None => Some(document),
        };
        assert_eq!(value.as_deref().cmp(&expected), core::cmp::Ordering::Equal, "{fault:?}");
    }
}
#[test]
fn the_documents_name_the_issuer_and_its_key() {
    let mut issuer = issuer(openid_terms(None));
    let metadata = get(&mut issuer, b"https://issuer.example/.well-known/openid-configuration").body;
    for (name, expected) in [
        (b"issuer".as_slice(), b"https://issuer.example".as_slice()),
        (b"authorization_endpoint", b"https://issuer.example/authorize"),
        (b"token_endpoint", b"https://issuer.example/token"),
        (b"jwks_uri", b"https://issuer.example/keys"),
    ] {
        assert_eq!(member(&metadata, name, 0).as_deref(), Some(expected));
    }
    let keys = get(&mut issuer, b"https://issuer.example/keys").body;
    for (name, expected) in [
        (b"kty".as_slice(), b"RSA".as_slice()),
        (b"kid", b"skein-test"),
        (b"use", b"sig"),
        (b"alg", b"RS256"),
        (b"e", b"AQAB"),
        (b"n", key::MODULUS.as_bytes()),
    ] {
        assert_eq!(member(&keys, name, 0).as_deref(), Some(expected));
    }
}
#[test]
fn a_get_for_another_endpoint_is_not_found() {
    let mut issuer = issuer(openid_terms(None));
    assert_eq!(get(&mut issuer, b"https://issuer.example/other").status, 404);
}
#[test]
fn a_scripted_id_token_is_answered_unchanged() {
    let mut issuer = issuer(openid_terms(None));
    issuer.queue(token_plan(Some(b"scripted.id.token"))).expect("plan");
    let body = exchange(&mut issuer);
    assert!(member(&body, b"id_token", 0).as_deref() == Some(b"scripted.id.token"));
}

fn account(subject: &[u8], verified: bool) -> Account {
    Account {
        subject: bytes::copy_of(subject),
        email: Some(Email { address: bytes::copy_of(b"member@example.test"), verified }),
        name: Some(bytes::copy_of(b"Member")),
    }
}
#[test]
fn the_default_account_has_no_optional_claims() {
    let claims = segment(&id_token(&mut issuer(openid_terms(None))), 1);
    for name in [b"email".as_slice(), b"email_verified", b"name"] {
        assert_eq!(member(&claims, name, 0), None);
    }
}
#[test]
fn the_account_supplies_its_email_verification_and_name() {
    let claims = account_claims(account(b"member", true));
    for (name, expected) in [
        (b"sub".as_slice(), b"member".as_slice()),
        (b"email", b"member@example.test"),
        (b"email_verified", b"true"),
        (b"name", b"Member"),
    ] {
        assert_eq!(member(&claims, name, 0).as_deref(), Some(expected));
    }
    assert!(has_verification(&claims, &Token::True));
}
#[test]
fn an_unverified_email_has_a_false_json_boolean() {
    let claims = account_claims(account(b"member", false));
    assert!(has_verification(&claims, &Token::False));
}
#[test]
fn a_code_keeps_the_account_at_its_authorization() {
    let mut issuer = issuer(openid_terms(None));
    issuer.sign_in_as(account(b"first", true)).expect("first account");
    let Some(Request::Redirect { code, .. }) = authorize(&mut issuer) else { panic!("redirect") };
    issuer.sign_in_as(account(b"second", false)).expect("second account");
    issuer.queue(token_plan(None)).expect("plan");
    let first = segment(&member(&exchange_code(&mut issuer, &code), b"id_token", 0).expect("token"), 1);
    let second = segment(&id_token(&mut issuer), 1);
    assert_eq!(member(&first, b"sub", 0).as_deref(), Some(b"first".as_slice()));
    assert_eq!(member(&first, b"email_verified", 0).as_deref(), Some(b"true".as_slice()));
    assert_eq!(member(&second, b"sub", 0).as_deref(), Some(b"second".as_slice()));
    assert_eq!(member(&second, b"email_verified", 0).as_deref(), Some(b"false".as_slice()));
}
fn sized_accounts(size: usize) -> [Account; 3] {
    let subject = Account { subject: bytes::zeroed(size), email: None, name: None };
    let email = Account {
        subject: bytes::copy_of(b"subject"),
        email: Some(Email { address: bytes::zeroed(size), verified: true }),
        name: None,
    };
    let name = Account { subject: bytes::copy_of(b"subject"), email: None, name: Some(bytes::zeroed(size)) };
    [subject, email, name]
}
#[test]
fn each_account_string_accepts_its_document_bound() {
    let cap = usize::try_from(limits().document.string_bytes).expect("u32 fits");
    let mut issuer = issuer(openid_terms(None));
    for account in sized_accounts(cap) {
        assert_eq!(issuer.sign_in_as(account), Ok(()));
    }
}
#[test]
fn each_account_string_refuses_one_past_its_bound() {
    let past = usize::try_from(limits().document.string_bytes).expect("u32 fits").checked_add(1).expect("one past");
    let mut issuer = issuer(openid_terms(None));
    for account in sized_accounts(past) {
        assert_eq!(issuer.sign_in_as(account), Err(Error::Limits));
    }
}
#[test]
fn an_empty_subject_is_refused() {
    let mut issuer = issuer(openid_terms(None));
    assert_eq!(issuer.sign_in_as(account(b"", true)), Err(Error::Limits));
}
#[test]
fn an_account_requires_openid_terms() {
    for config in [config(), Config { terms: Some(terms(&[])), ..config() }] {
        let mut issuer = Issuer::new(config, limits()).expect("issuer");
        assert_eq!(issuer.sign_in_as(account(b"member", true)), Err(Error::Plan));
    }
}

fn account_claims(account: Account) -> Box<[u8]> {
    let mut issuer = issuer(openid_terms(None));
    issuer.sign_in_as(account).expect("account");
    segment(&id_token(&mut issuer), 1)
}
fn has_verification(claims: &[u8], expected: &Token) -> bool {
    let json = Json::from_bytes(claims, &limits().document).expect("claims");
    for pair in json.as_tokens().windows(2) {
        if pair.first() == Some(&Token::Key(bytes::copy_of(b"email_verified"))) && pair.get(1) == Some(expected) {
            return true;
        }
    }
    false
}

#[test]
fn issuer_refuses_unrepresentable_limits() {
    let good = limits();
    assert!(valid(&good));
    for bad in [
        Limits { codes: 0, ..good },
        Limits { rotations: 0, ..good },
        Limits { carried: u32::MAX, ..good },
        Limits { document: DocumentLimits { document_bytes: u32::MAX, ..good.document }, ..good },
        Limits { document: DocumentLimits { token_bytes: u32::MAX, ..good.document }, ..good },
    ] {
        assert!(!valid(&bad));
        assert_eq!(Issuer::new(config(), bad).err(), Some(Error::Limits));
    }
}
