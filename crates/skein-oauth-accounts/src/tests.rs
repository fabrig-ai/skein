use super::*;
use skein_lib::{Duration, Env, List, Queue, Time, Wall, bytes};
use skein_oauth::{ClientLimits, SavedToken};

pub(crate) fn limits() -> Limits {
    Limits {
        accounts: 2,
        file_stall: Duration::from_secs(1),
        lock_wait: Duration::from_secs(1),
        exchanges: 2,
        listeners: 1,
        server: skein_http::server::Limits {
            head: 2048,
            headers: 16,
            body: 1024,
            read: 256,
            response: 2048,
            send: 256,
        },
        http: skein_http::client::Limits { request: 2048, head: 2048, headers: 16, read: 256, send: 256 },
        tls: skein_tls::client::Limits { read: 2048, send: 2048, records: 131_072 },
        io: skein_io::Limits {
            sockets: 4,
            refusals: 1,
            intake: 32_768,
            receive: 1024,
            output: 32_768,
            sends: 4,
            accepts: 1,
            backlog: 2,
            close_timeout: Duration::from_secs(1),
            names: 0,
            retry: Duration::from_millis(1),
        },
        refresh_lead: Duration::from_secs(10),
        client: ClientLimits {
            document: skein_oauth::Limits {
                document_bytes: 1024,
                string_bytes: 256,
                token_bytes: 64,
                client_bytes: 64,
                detail_bytes: 64,
                record_bytes: 512,
                depth: 8,
                tokens: 64,
                issued_client_bytes: 0,
                granted_bytes: 64,
                subject_bytes: 64,
                id_token_bytes: 64,
            },
            uri_bytes: 256,
            scope_bytes: 64,
            state_bytes: 64,
            code_bytes: 64,
            url_bytes: 1024,
            request_bytes: 1024,
            parameter_bytes: 0,
            resource_bytes: 0,
            nonce_bytes: 0,
            metadata_bytes: 0,
            key_set_bytes: 0,
            keys: 0,
            modulus_bits: 2048,
            skew: Duration::from_secs(0),
            sign_in_time: Duration::from_secs(120),
            request_time: Duration::from_secs(10),
            backoff_base: Duration::from_secs(2),
            backoff_ceiling: Duration::from_secs(8),
            max_attempts: 3,
        },
    }
}
fn setup() -> (Component, Env<Limits>, Queue<Event>) {
    let limits = limits();
    let mut accounts = List::with_capacity(2);
    match accounts.push(Account::HandedIn) {
        Ok(()) => {}
        Err(_) => unreachable!("account slot"),
    }
    match accounts.push(Account::HandedIn) {
        Ok(()) => {}
        Err(_) => unreachable!("account slot"),
    }
    let component = Component::new(accounts, &limits, crypto(1)).expect("component");
    (component, Env { now: Time::ZERO, wall: Wall::EPOCH, limits }, Queue::uncapped())
}
fn tests_record(generation: u64) -> SavedToken {
    SavedToken {
        key: 0,
        generation,
        access_token: bytes::copy_of(b"access"),
        refresh_token: None,
        metadata: None,
        expires_at: Wall::from_nanos(30_000_000_000),
        issued_client_id: None,
        granted_scopes: None,
        subject: None,
        id_token: None,
        earliest_refresh_at: None,
    }
}
fn hand_in(component: &mut Component, env: &Env<Limits>, out: &mut Queue<Event>, generation: u64) {
    component.down(
        env,
        Request::HandIn { account: 0, record: tests_record(generation) },
        out,
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
    );
}
fn assert_event(event: Option<Event>, expected: Event) {
    assert!(event == Some(expected), "the request has its expected typed outcome");
}
fn assert_granted(event: Option<Event>, generation: u64, seconds: u64) {
    assert_event(
        event,
        Event::Granted {
            account: 0,
            token: bytes::copy_of(b"access"),
            generation,
            valid: Duration::from_secs(seconds),
        },
    );
}
fn assert_expired(event: Option<Event>) {
    assert_event(event, Event::Failed { account: 0, ends: Ends::Grant, failure: Failure::Expired });
}
fn assert_refused(event: Option<Event>, account: u32, asked: Asked, why: Refusal) {
    assert_eq!(refusal_of(event), Some((account, asked, why)));
}

#[test]
fn a_handed_in_record_is_lent_while_it_is_valid() {
    let (mut component, env, mut out) = setup();
    hand_in(&mut component, &env, &mut out, 1);
    assert!(out.is_empty());
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_granted(out.pop(), 1, 30);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_refused(out.pop(), 0, Asked::Grant, Refusal::Held);
    component.down(&env, Request::Release { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_granted(out.pop(), 1, 30);
}

#[test]
fn an_access_only_record_is_announced_expiring_once_and_fails_at_expiry() {
    let (mut component, mut env, mut out) = setup();
    hand_in(&mut component, &env, &mut out, 1);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    drop(out.pop());
    assert_eq!(component.next_deadline(), Some(Time::from_nanos(20_000_000_000)));
    env.now = Time::from_nanos(20_000_000_000);
    // Wall jumps never extend a running grant's deadline.
    env.wall = Wall::from_nanos(100_000_000_000);
    component.fire(&env, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_event(out.pop(), Event::Expiring { account: 0, generation: 1 });
    component.fire(&env, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert!(out.is_empty());
    assert_eq!(component.next_deadline(), Some(Time::from_nanos(30_000_000_000)));
    env.now = Time::from_nanos(30_000_000_000);
    component.fire(&env, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_expired(out.pop());
    component.fire(&env, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert!(out.is_empty());
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_expired(out.pop());
}

#[test]
fn a_rejected_record_fails_until_a_newer_one_is_handed_in() {
    let (mut component, mut env, mut out) = setup();
    hand_in(&mut component, &env, &mut out, 1);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    drop(out.pop());
    component.down(
        &env,
        Request::Rejected { account: 0, generation: 0 },
        &mut out,
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
    );
    assert!(out.is_empty());
    component.down(
        &env,
        Request::Rejected { account: 0, generation: 1 },
        &mut out,
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
    );
    assert_expired(out.pop());
    hand_in(&mut component, &env, &mut out, 1);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_expired(out.pop());
    env.now = Time::from_nanos(1_000_000_000);
    env.wall = Wall::from_nanos(1_000_000_000);
    hand_in(&mut component, &env, &mut out, 2);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_granted(out.pop(), 2, 29);
}

#[test]
fn a_newer_record_announces_the_new_generation_while_held() {
    let (mut component, env, mut out) = setup();
    hand_in(&mut component, &env, &mut out, 1);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    drop(out.pop());
    hand_in(&mut component, &env, &mut out, 0);
    assert!(out.is_empty());
    hand_in(&mut component, &env, &mut out, 2);
    assert_granted(out.pop(), 2, 30);
}

#[test]
fn a_record_with_a_refresh_token_or_past_its_bounds_is_refused() {
    let (mut component, env, mut out) = setup();
    let mut record = tests_record(1);
    record.refresh_token = Some(bytes::copy_of(b"refresh"));
    component.down(
        &env,
        Request::HandIn { account: 0, record },
        &mut out,
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
    );
    assert_refused(out.pop(), 0, Asked::HandIn, Refusal::NotAccessOnly);
    let mut record = tests_record(1);
    record.access_token = bytes::copy_of(&[b'x'; 65]);
    component.down(
        &env,
        Request::HandIn { account: 0, record },
        &mut out,
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
    );
    assert_refused(out.pop(), 0, Asked::HandIn, Refusal::Record(skein_oauth::DecodeError::TooLarge));
}

#[test]
fn close_and_abort_settle_once_clear_deadlines_and_refuse_new_work_as_closed() {
    for close in [Request::Close, Request::Abort] {
        let (mut component, env, mut out) = setup();
        hand_in(&mut component, &env, &mut out, 1);
        component.down(&env, close, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
        assert_event(out.pop(), Event::Closed);
        assert_eq!(component.next_deadline(), None);
        component.down(&env, Request::Close, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
        component.down(&env, Request::Abort, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
        component.fire(&env, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
        assert!(out.is_empty());
        component.down(&env, Request::Grant { account: 9 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
        assert_refused(out.pop(), 9, Asked::Grant, Refusal::Closed);
        component.down(
            &env,
            Request::SignIn { account: 9, subject: None },
            &mut out,
            &mut Queue::uncapped(),
            &mut Queue::uncapped(),
        );
        assert_refused(out.pop(), 9, Asked::SignIn, Refusal::Closed);
        component.down(
            &env,
            Request::Redirected { account: 9, uri: bytes::copy_of(b"late") },
            &mut out,
            &mut Queue::uncapped(),
            &mut Queue::uncapped(),
        );
        assert_refused(out.pop(), 9, Asked::Redirected, Refusal::Closed);

        hand_in(&mut component, &env, &mut out, 2);
        assert_refused(out.pop(), 0, Asked::HandIn, Refusal::Closed);
    }
}

#[test]
fn a_missing_account_and_missing_record_have_typed_terminals() {
    let (mut component, env, mut out) = setup();
    component.down(&env, Request::Grant { account: 3 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_refused(out.pop(), 3, Asked::Grant, Refusal::Account);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_expired(out.pop());
}

#[test]
fn a_released_record_still_announces_its_lead_without_an_expiry_failure() {
    let (mut component, mut env, mut out) = setup();
    hand_in(&mut component, &env, &mut out, 1);
    component.down(&env, Request::Grant { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    drop(out.pop());
    component.down(&env, Request::Release { account: 0 }, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    env.now = Time::from_nanos(20_000_000_000);
    component.fire(&env, &mut out, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_event(out.pop(), Event::Expiring { account: 0, generation: 1 });
    assert_eq!(component.next_deadline(), None);
}

pub(crate) fn refresh_account(account: u32, address: core::net::SocketAddr) -> Account {
    let mut record = tests_record(0);
    record.key = account;
    record.refresh_token = Some(bytes::copy_of(b"refresh"));
    record.expires_at = Wall::from_nanos(5_000_000_000);
    Account::SignIn {
        origins: List::with_capacity(0),
        shared: false,
        registration: skein_oauth::Registration {
            authorization_url: bytes::copy_of(b"http://127.0.0.1:31000/authorize"),
            token_endpoint: bytes::copy_of(b"http://127.0.0.1:31000/token"),
            client_id: bytes::copy_of(b"client"),
            redirect_uri: bytes::copy_of(b"http://localhost:31234/callback"),
            scope: bytes::copy_of(b"read"),
            wire: skein_oauth::WireFormat::Form,
            client_secret: None,
            pkce_for_confidential: false,
            metadata_claim: None,
            terms: None,
        },
        endpoint: Endpoint { address, transport: Transport::Plaintext },
        keeper: Keeper::Owner { kept: Some(record) },
    }
}

#[test]
fn a_refresh_refused_as_sign_in_again_drops_an_owner_kept_record() {
    let profile = limits();
    let mut configured = List::with_capacity(1);
    assert!(configured.push(refresh_account(0, ([127, 0, 0, 1], 31000).into())).is_ok());
    let mut component = Component::new(configured, &profile, crypto(1)).expect("owner-kept account");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: profile };
    let mut above = Queue::uncapped();
    let mut io = Queue::uncapped();
    let mut files = Queue::uncapped();
    component.down(&env, Request::Grant { account: 0 }, &mut above, &mut io, &mut files);
    component::refuse_refresh(&mut component, &env, &mut above, &mut io);
    assert_event(
        above.pop(),
        Event::Failed { account: 0, ends: Ends::Grant, failure: Failure::Exchange(skein_oauth::Failure::Refused) },
    );
    component.down(&env, Request::Grant { account: 0 }, &mut above, &mut io, &mut files);
    assert_expired(above.pop());
    assert!(above.is_empty() && io.is_empty() && files.is_empty(), "the dead record starts no new refresh");
}

#[test]
fn an_exchange_bound_refuses_a_second_account_before_admitting_its_grant() {
    let mut limits = limits();
    limits.exchanges = 1;
    let mut configured = List::with_capacity(2);
    assert!(
        configured.push(refresh_account(0, core::net::SocketAddr::from(([127, 0, 0, 1], 31000)))).is_ok(),
        "first account"
    );
    assert!(
        configured.push(refresh_account(1, core::net::SocketAddr::from(([127, 0, 0, 1], 31000)))).is_ok(),
        "second account"
    );
    let mut component = Component::new(configured, &limits, crypto(1)).expect("configured component");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut above = Queue::uncapped();
    let mut io = Queue::uncapped();
    component.down(&env, Request::Grant { account: 0 }, &mut above, &mut io, &mut Queue::uncapped());
    assert!(above.is_empty() && io.is_empty(), "grant waits for refresh child work");
    component.down(&env, Request::Grant { account: 1 }, &mut above, &mut io, &mut Queue::uncapped());
    assert_refused(above.pop(), 1, Asked::Grant, Refusal::Full { bound: 1 });
    component.down(&env, Request::Abort, &mut above, &mut io, &mut Queue::uncapped());
    component.fire(&env, &mut above, &mut io, &mut Queue::uncapped());
    assert_event(
        above.pop(),
        Event::Failed { account: 0, ends: Ends::Grant, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) },
    );
    assert_event(above.pop(), Event::Closed);
    assert!(io.is_empty(), "unstarted exchange has no socket to abort");
    component.down(
        &env,
        Request::Kept { account: 0, generation: 1, keeping: Keeping::Kept },
        &mut above,
        &mut io,
        &mut Queue::uncapped(),
    );
    assert!(above.is_empty() && io.is_empty(), "stale keeper terminal after abort is inert");
    assert!(!component.has_work() && component.next_deadline().is_none(), "abort settled every child");
}

#[test]
fn a_plaintext_endpoint_off_loopback_is_refused_at_startup() {
    let mut configured = List::with_capacity(1);
    assert!(
        configured.push(refresh_account(0, core::net::SocketAddr::from(([192, 0, 2, 1], 31000)))).is_ok(),
        "account"
    );
    assert_eq!(Component::new(configured, &limits(), crypto(1)).err(), Some(Unusable::Plaintext { account: 0 }));
}

#[path = "private_tests.rs"]
mod private;

pub(crate) fn crypto(n: u8) -> skein_lib::CryptoRng {
    let seed = skein_lib::Secret::new(bytes::copy_of(&[n; 32]));
    skein_lib::CryptoRng::new(&seed, 0).expect("a 32-byte fixture seed")
}

#[test]
fn two_sign_ins_draw_different_states_and_verifiers() {
    let mut random = crypto(0);
    let state = component::draw(&mut random, 24);
    let verifier = component::draw(&mut random, 32);
    assert_ne!(state, component::draw(&mut random, 24));
    assert_ne!(verifier, component::draw(&mut random, 32));
}

#[test]
fn a_verifier_is_43_url_safe_characters() {
    let verifier = component::draw(&mut crypto(0), 32);
    assert_eq!(verifier.len(), 43);
    for byte in verifier.as_ref() {
        assert!(byte.is_ascii_alphanumeric() || b"-_".contains(byte));
    }
}

fn issued_terms() -> skein_oauth::Terms {
    skein_oauth::Terms { issued_client_id: true, ..terms() }
}

#[test]
fn a_name_no_live_account_holds_is_refused() {
    const MISSING: u32 = 7;
    let (mut component, env, mut above) = setup();
    for (request, asked) in [
        (Request::SignIn { account: MISSING, subject: None }, Asked::SignIn),
        (
            Request::Redirected { account: MISSING, uri: bytes::copy_of(b"https://service.example/callback") },
            Asked::Redirected,
        ),
        (Request::HandIn { account: MISSING, record: tests_record(0) }, Asked::HandIn),
    ] {
        component.down(&env, request, &mut above, &mut Queue::uncapped(), &mut Queue::uncapped());
        assert_refused(above.pop(), MISSING, asked, Refusal::Account);
    }
}

#[test]
fn an_owner_record_without_an_issued_id_is_held_as_none() {
    let mut profile = limits();
    profile.client.document.issued_client_bytes = profile.client.document.client_bytes;
    let mut account = refresh_account(0, ([127, 0, 0, 1], 31000).into());
    match &mut account {
        Account::SignIn { registration, .. } => registration.terms = Some(issued_terms()),
        Account::HandedIn => unreachable!("refresh fixture"),
    }
    let mut configured = List::with_capacity(1);
    assert!(configured.push(account).is_ok());
    let mut component = Component::new(configured, &profile, crypto(1)).expect("issued registration");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: profile };
    let mut above = Queue::uncapped();
    component.down(&env, Request::Grant { account: 0 }, &mut above, &mut Queue::uncapped(), &mut Queue::uncapped());
    assert_expired(above.pop());
}

#[path = "accounts_tests.rs"]
mod accounts;

fn refusal_of(event: Option<Event>) -> Option<(u32, Asked, Refusal)> {
    match event {
        Some(Event::Refused { account, asked, why }) => Some((account, asked, why)),
        Some(
            Event::Visit { .. }
            | Event::Taken { .. }
            | Event::SignedIn { .. }
            | Event::Lock { .. }
            | Event::Unlock { .. }
            | Event::SignedOut { .. }
            | Event::Granted { .. }
            | Event::Expiring { .. }
            | Event::Keep { .. }
            | Event::Failed { .. }
            | Event::Removed { .. }
            | Event::Closed,
        )
        | None => None,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Failed {
    account: u32,
    ends: Ends,
    failure: Failure,
}
fn failure_of(event: Option<Event>) -> Option<Failed> {
    match event {
        Some(Event::Failed { account, ends, failure }) => Some(Failed { account, ends, failure }),
        Some(
            Event::Visit { .. }
            | Event::Taken { .. }
            | Event::SignedIn { .. }
            | Event::Lock { .. }
            | Event::Unlock { .. }
            | Event::SignedOut { .. }
            | Event::Granted { .. }
            | Event::Expiring { .. }
            | Event::Keep { .. }
            | Event::Refused { .. }
            | Event::Removed { .. }
            | Event::Closed,
        )
        | None => None,
    }
}

#[path = "ports_tests.rs"]
mod ports;

#[path = "sign_out_tests.rs"]
mod sign_out;

fn scope_owner(granted: &[u8], kept: bool) -> private::Owner {
    let mut account = refresh_account(0, ([127, 0, 0, 1], 31000).into());
    if let Account::SignIn { registration, keeper, .. } = &mut account {
        let mut terms = issued_terms();
        terms.issued_client_id = false;
        terms.required_scopes = Some(bytes::copy_of(REQUIRED_SCOPES));
        registration.terms = Some(terms);
        registration.scope = bytes::copy_of(REQUIRED_SCOPES);
        registration.client_secret = Some(bytes::copy_of(b"secret"));
        registration.redirect_uri = bytes::copy_of(b"https://service.example/callback");
        let mut record = tests_record(0);
        record.refresh_token = Some(bytes::copy_of(b"refresh"));
        record.granted_scopes = Some(bytes::copy_of(granted));
        *keeper = Keeper::Owner { kept: kept.then_some(record) };
    }
    private::Owner::configured([account], &limits())
}
fn scope_body(scopes: &[u8]) -> Box<[u8]> {
    let parts =
        [br#"{"access_token":"new","refresh_token":"next","expires_in":30,"scope":""#.as_slice(), scopes, br#""}"#];
    let mut length = 0_usize;
    for part in parts {
        length = length.checked_add(part.len()).expect("body fixture length");
    }
    let mut body = skein_lib::Writer::new(length);
    for part in parts {
        body.put(part).expect("body fixture room");
    }
    body.finish()
}
#[test]
fn signed_in_names_the_required_scopes_not_granted() {
    for (granted, missing) in [(REQUIRED_SCOPES, b"".as_slice()), (b"read", b"write"), (b"", REQUIRED_SCOPES)] {
        let mut owner = scope_owner(granted, false);
        owner.redirected(b"");
        assert_event(owner.above.pop(), Event::Taken { account: 0 });
        let identity = owner.sent();
        let mut events = owner.finish_tokens(identity, &scope_body(granted));
        let observed = if let Some(Event::SignedIn { generation, missing, .. }) = events.pop() {
            Some((generation, missing))
        } else {
            None
        };
        assert!(events.is_empty());
        assert_eq!(observed, Some((1, bytes::copy_of(missing))), "granted scopes {granted:?}");
    }
}
fn scope_grant(refresh: bool) -> (Option<Failure>, bool) {
    let mut owner = scope_owner(if refresh { REQUIRED_SCOPES } else { b"read" }, true);
    owner.ask(Request::Grant { account: 0 });
    let mut events = if refresh {
        drop(owner.above.pop());
        owner.ask(Request::Rejected { account: 0, generation: 0 });
        let identity = owner.sent();
        owner.finish_tokens(identity, &scope_body(b"read"))
    } else {
        core::mem::replace(&mut owner.above, Queue::uncapped())
    };
    let failure =
        if let Some(Event::Failed { ends: Ends::Grant, failure, .. }) = events.pop() { Some(failure) } else { None };
    (failure, events.is_empty())
}
#[test]
fn a_grant_of_a_record_without_a_required_scope_fails_as_not_granted() {
    for (name, refresh, expected) in [
        ("grant", false, (Some(Failure::NotGranted), true)),
        ("refresh re-grant", true, (Some(Failure::NotGranted), true)),
    ] {
        assert_eq!(scope_grant(refresh), expected, "{name}");
    }
}

#[path = "shared_tests.rs"]
mod shared;

const REQUIRED_SCOPES: &[u8] = b"read write";
pub(crate) fn terms() -> skein_oauth::Terms {
    skein_oauth::Terms {
        parameters: Box::new([]),
        openid: None,
        issued_client_id: false,
        varying_port: false,
        resource: None,
        required_scopes: None,
        revocation_endpoint: None,
        sign_in_again: Box::new([]),
    }
}

const EXPECTED_SUBJECT: &[u8] = b"expected-subject";
#[path = "openid_tests.rs"]
mod openid;

#[path = "paste_tests.rs"]
mod paste;
