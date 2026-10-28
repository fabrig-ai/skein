//! `OpenID` driver routes and subject terminals (oauth.md, 6.2 and 6.3).
use super::*;
fn openid_owner() -> private::Owner {
    let mut profile = limits();
    profile.client.nonce_bytes = 32;
    profile.client.keys = 1;
    profile.client.key_set_bytes = 512;
    profile.client.metadata_bytes = 512;
    let mut account = refresh_account(0, ([127, 0, 0, 1], 31000).into());
    if let Account::SignIn { registration, origins, .. } = &mut account {
        registration.scope = bytes::copy_of(b"openid");
        registration.client_secret = Some(bytes::copy_of(b"secret"));
        registration.redirect_uri = bytes::copy_of(b"https://service.example/callback");
        registration.terms = Some(skein_oauth::Terms {
            openid: Some(skein_oauth::OpenId { issuer: bytes::copy_of(b"http://127.0.0.1:31001"), key_set: None }),
            ..terms()
        });
        for (uri, port) in [(b"http://127.0.0.1:31001".as_slice(), 31001), (b"http://127.0.0.1:31002", 31002)] {
            let origin = Origin {
                uri: bytes::copy_of(uri),
                endpoint: Endpoint { address: ([127, 0, 0, 1], port).into(), transport: Transport::Plaintext },
            };
            if origins.capacity() == 0 {
                *origins = List::with_capacity(2);
            }
            origins.push(origin).expect("origins");
        }
    }
    private::Owner::configured([account], &profile)
}
const METADATA: &[u8] = br#"{"issuer":"http://127.0.0.1:31001","authorization_endpoint":"http://127.0.0.1:31000/authorize","token_endpoint":"http://127.0.0.1:31000/token","jwks_uri":"http://127.0.0.1:31002/keys"}"#;
#[test]
fn the_metadata_and_key_set_go_as_get_on_their_origin() {
    let mut owner = openid_owner();
    owner.ask(Request::SignIn { account: 0, subject: None });
    let (identity, head, address) = owner.sent_request();
    assert!(head.starts_with(b"GET /.well-known/openid-configuration HTTP/1.1\r\n"));
    assert_eq!(address.port(), 31001);
    owner.answer_http(identity, METADATA);
    let (_, head, address) = owner.sent_request();
    assert!(head.starts_with(b"GET /keys HTTP/1.1\r\n"));
    assert_eq!(address.port(), 31002);
}
#[test]
fn a_key_set_on_an_origin_not_given_fails_as_unverified() {
    let mut owner = openid_owner();
    // Admission kept both origins; an unknown URL is discovered only in metadata.
    owner.ask(Request::SignIn { account: 0, subject: None });
    let identity = owner.sent();
    let metadata = br#"{"issuer":"http://127.0.0.1:31001","authorization_endpoint":"http://127.0.0.1:31000/authorize","token_endpoint":"http://127.0.0.1:31000/token","jwks_uri":"http://127.0.0.1:31003/keys"}"#;
    owner.answer_http(identity, metadata);
    for _ in 0_u32..16 {
        owner.fire();
    }
    assert_eq!(
        failure_of(owner.above.pop()),
        Some(Failed {
            account: 0,
            ends: Ends::SignIn,
            failure: Failure::Exchange(skein_oauth::Failure::Unverified { check: skein_oauth::Check::Keys })
        })
    );
    assert!(owner.io.is_empty());
}
#[test]
fn a_sign_in_of_another_subject_keeps_nothing() {
    let mut owner = scope_owner(REQUIRED_SCOPES, false);
    owner.redirected_as(b"", Some(bytes::copy_of(EXPECTED_SUBJECT)));
    assert_event(owner.above.pop(), Event::Taken { account: 0 });
    let identity = owner.sent();
    let mut events = owner.finish_tokens(identity, private::BODY);
    assert_eq!(
        failure_of(events.pop()),
        Some(Failed { account: 0, ends: Ends::SignIn, failure: Failure::AnotherSubject })
    );
    assert!(events.is_empty() && owner.files.is_empty());
}
#[test]
fn signed_in_names_the_records_subject() {
    let mut owner = scope_owner(REQUIRED_SCOPES, false);
    owner.redirected(b"");
    assert_event(owner.above.pop(), Event::Taken { account: 0 });
    let identity = owner.sent();
    let mut events = owner.finish_tokens(identity, private::BODY);
    let observed =
        if let Some(Event::SignedIn { subject, .. }) = events.pop() { Some(subject.is_none()) } else { None };
    assert_eq!(observed, Some(true));
}

#[test]
fn handed_in_accounts_refuse_sign_in_and_redirect_without_child_work() {
    let (mut component, env, mut above) = setup();
    let mut below = Queue::uncapped();
    component.down(&env, Request::SignIn { account: 0, subject: None }, &mut above, &mut below, &mut Queue::uncapped());
    assert_refused(above.pop(), 0, Asked::SignIn, Refusal::Source);
    component.down(
        &env,
        Request::Redirected { account: 0, uri: bytes::copy_of(b"http://localhost:31234/callback?state=a&code=b") },
        &mut above,
        &mut below,
        &mut Queue::uncapped(),
    );
    assert_refused(above.pop(), 0, Asked::Redirected, Refusal::Source);
    assert!(below.is_empty());
}

#[test]
fn redirect_uri_is_checked_before_bounded_query_decoding() {
    let limits = limits();
    let registered = b"http://localhost:31234/callback";
    assert!(
        redirect::parse(b"http://localhost:31234/callback?state=a%2Bb&code=c%20d", registered, &limits.client)
            .is_some()
    );
    for uri in [
        b"http://127.0.0.1:31234/callback?state=a&code=b".as_slice(),
        b"http://localhost.:31234/callback?state=a&code=b",
        b"http://localhost:31234/wrong?state=a&code=b",
        b"http://localhost:31234/callback?state=a&state=b&code=c",
        b"http://localhost:31234/callback?state=%xx&code=b",
        b"http://localhost:31234/callback?state=a&code=b#fragment",
    ] {
        assert!(redirect::parse(uri, registered, &limits.client).is_none());
    }
}

#[test]
fn public_sign_in_admission_respects_listener_and_account_bounds() {
    let mut profile = limits();
    profile.listeners = 0;
    let address = core::net::SocketAddr::new(core::net::IpAddr::V4(core::net::Ipv4Addr::LOCALHOST), 31000);
    let mut configured = List::with_capacity(2);
    assert!(configured.push(refresh_account(0, address)).is_ok());
    assert!(configured.push(refresh_account(1, address)).is_ok());
    let mut component = Component::new(configured, &profile, crypto(17)).expect("component");
    let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: profile };
    let mut above = Queue::uncapped();
    let mut below = Queue::uncapped();
    component.down(&env, Request::SignIn { account: 0, subject: None }, &mut above, &mut below, &mut Queue::uncapped());
    assert_refused(above.pop(), 0, Asked::SignIn, Refusal::Full { bound: 0 });
    assert!(below.is_empty());
    env.limits.listeners = 1;
    let mut configured = List::with_capacity(2);
    assert!(configured.push(refresh_account(0, address)).is_ok());
    assert!(configured.push(refresh_account(1, address)).is_ok());
    component = Component::new(configured, &env.limits, crypto(17)).expect("one listener component");
    component.down(&env, Request::SignIn { account: 0, subject: None }, &mut above, &mut below, &mut Queue::uncapped());
    assert!(above.is_empty());
    assert!(below.pop().is_some());
    component.down(&env, Request::SignIn { account: 0, subject: None }, &mut above, &mut below, &mut Queue::uncapped());
    assert_refused(above.pop(), 0, Asked::SignIn, Refusal::Busy);
    component.down(&env, Request::SignIn { account: 1, subject: None }, &mut above, &mut below, &mut Queue::uncapped());
    assert_refused(above.pop(), 1, Asked::SignIn, Refusal::Full { bound: 1 });
    assert!(below.is_empty());
}
