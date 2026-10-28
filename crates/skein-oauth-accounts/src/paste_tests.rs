//! Redirect admission from the owner's paste and the listener (oauth.md, 6.5).
use super::private::Owner;
use super::*;
use skein_lib::{Token, Writer};

const REGISTERED: &[u8] = b"http://127.0.0.1:0/callback";
const BOUND: &[u8] = b"http://127.0.0.1:41000/callback";
const CONFIDENTIAL: &[u8] = b"https://service.example/callback";
const PORT: u16 = 41000;

fn account(public: bool, issued: bool) -> Account {
    let mut account = refresh_account(0, ([127, 0, 0, 1], 31000).into());
    if let Account::SignIn { registration, keeper, .. } = &mut account {
        registration.redirect_uri = bytes::copy_of(if public { REGISTERED } else { CONFIDENTIAL });
        registration.client_secret = if public { None } else { Some(bytes::copy_of(b"fake secret")) };
        registration.terms = Some(skein_oauth::Terms { varying_port: public, issued_client_id: issued, ..terms() });
        *keeper = Keeper::Owner { kept: None };
    }
    account
}
fn configured(public: bool, issued: bool) -> Owner {
    let mut profile = limits();
    profile.client.document.issued_client_bytes = 32;
    Owner::configured([account(public, issued)], &profile)
}
fn waiting(issued: bool) -> (Owner, Token, Box<[u8]>) {
    let mut owner = configured(true, issued);
    owner.ask(Request::SignIn { account: 0, subject: None });
    let listener = owner.listen();
    owner.listening(listener, PORT);
    let visit = owner.visit();
    (owner, listener, visit)
}
fn joined(parts: &[&[u8]]) -> Box<[u8]> {
    let mut length = 0_usize;
    for part in parts {
        length = length.checked_add(part.len()).expect("fixture length");
    }
    let mut out = Writer::new(length);
    for part in parts {
        out.put(part).expect("fixture room");
    }
    out.finish()
}
fn paste(owner: &mut Owner, address: &[u8], state: &[u8], fields: &[u8]) {
    owner.ask(Request::Redirected { account: 0, uri: joined(&[address, b"?state=", state, fields]) });
}
fn taken(owner: &mut Owner) {
    assert_event(owner.above.pop(), Event::Taken { account: 0 });
    assert!(owner.above.is_empty(), "the sign-in terminal follows later");
}
fn refused(owner: &mut Owner, why: Refusal) {
    assert_refused(owner.above.pop(), 0, Asked::Redirected, why);
    assert!(owner.above.is_empty());
}
fn head(state: &[u8], fields: &[u8]) -> Box<[u8]> {
    joined(&[b"GET /callback?state=", state, fields, b" HTTP/1.1\r\nHost: 127.0.0.1:41000\r\n\r\n"])
}
fn settle_listener(owner: &mut Owner, listener: Token) {
    let mut held = Queue::uncapped();
    for _ in 0_u32..16 {
        owner.fire();
        let mut closed = false;
        while let Some(request) = owner.io.pop() {
            if let Lower::Close { entity } = request
                && entity == Token::new(10)
            {
                owner.socket(LowerEvent::Closed { owner: listener });
                closed = true;
            } else {
                held.push(request);
            }
        }
        while let Some(request) = held.pop() {
            owner.io.push(request);
        }
        if closed {
            return;
        }
    }
    panic!("taken sign-in closes its listener");
}
fn signs_in(owner: &mut Owner) {
    let identity = owner.sent();
    let mut events = owner.finish_tokens(identity, private::BODY);
    assert_event(events.pop(), Event::SignedIn { account: 0, generation: 1, missing: Box::new([]), subject: None });
    assert!(events.is_empty(), "one sign-in terminal");
    for _ in 0_u32..8 {
        owner.fire();
    }
    assert!(owner.io.is_empty() && owner.above.is_empty(), "one code exchange");
}

#[test]
fn a_pasted_redirect_is_taken_and_signs_in() {
    let (mut owner, listener, visit) = waiting(false);
    // A connection already accepted when the paste wins still receives a refusal.
    let callback = owner.callback(listener);
    paste(&mut owner, BOUND, Owner::query(&visit, b"state").expect("state"), b"&code=code");
    taken(&mut owner);
    owner.socket(LowerEvent::Accepted {
        owner: listener,
        socket: Token::new(12),
        peer: skein_io::Addr::Ip(([127, 0, 0, 1], 41002).into()),
    });
    assert!(matches_reject(owner.io.pop()));
    let request = head(Owner::query(&visit, b"state").expect("state"), b"&code=code");
    let mut remaining = request.as_ref();
    let mut answer = List::with_capacity(4096);
    for _ in 0_u32..128 {
        owner.fire();
        if owner.callback_io(listener, callback, &mut remaining, &mut answer) {
            break;
        }
    }
    assert!(answer.as_slice().starts_with(b"HTTP/1.1 404"));
    signs_in(&mut owner);
}
fn matches_reject(request: Option<Lower>) -> bool {
    if let Some(Lower::Reject { socket }) = request { socket == Token::new(12) } else { false }
}

#[test]
fn a_paste_that_is_not_the_sign_ins_is_refused_and_the_sign_in_still_waits() {
    for (name, address, fields, wrong_state, expected) in [
        (
            "another host",
            b"http://localhost:41000/callback".as_slice(),
            b"&code=code".as_slice(),
            false,
            Refusal::Redirect,
        ),
        ("another port", b"http://127.0.0.1:41001/callback", b"&code=code", false, Refusal::Redirect),
        ("registered zero port", REGISTERED, b"&code=code", false, Refusal::Redirect),
        ("another path", b"http://127.0.0.1:41000/other", b"&code=code", false, Refusal::Redirect),
        ("fragment", BOUND, b"&code=code#fragment", false, Refusal::Redirect),
        ("state twice", BOUND, b"&state=other&code=code", false, Refusal::Redirect),
        ("code twice", BOUND, b"&code=one&code=two", false, Refusal::Redirect),
        ("space", BOUND, b"&code=one two", false, Refusal::Redirect),
        ("control byte", BOUND, b"&code=one\x01two", false, Refusal::Redirect),
        ("both", BOUND, b"&code=code&error=denied", false, Refusal::Redirect),
        ("neither", BOUND, b"", false, Refusal::Redirect),
        ("empty code", BOUND, b"&code=", false, Refusal::Redirect),
        ("no issued id", BOUND, b"&code=code", false, Refusal::Redirect),
        ("empty issued id", BOUND, b"&code=code&client_id=", false, Refusal::Redirect),
        ("another state", BOUND, b"&code=code&client_id=issued", true, Refusal::State),
        ("past url bound", BOUND, b"&code=code&extra=", false, Refusal::Redirect),
    ] {
        let (mut owner, listener, visit) = waiting(true);
        let state = Owner::query(&visit, b"state").expect("state");
        if name == "past url bound" {
            let fields = joined(&[fields, &[b'x'; 1024]]);
            paste(&mut owner, address, state, &fields);
        } else {
            paste(&mut owner, address, if wrong_state { b"another-state" } else { state }, fields);
        }
        assert_eq!(refusal_of(owner.above.pop()), Some((0, Asked::Redirected, expected)), "{name}");
        assert!(owner.io.is_empty() && owner.above.is_empty(), "{name}");
        paste(&mut owner, BOUND, state, b"&code=code&client_id=issued");
        taken(&mut owner);
        settle_listener(&mut owner, listener);
        signs_in(&mut owner);
    }
}

#[test]
fn after_state_refusals_every_paste_is_full_and_the_listener_still_takes_its_own() {
    let (mut owner, listener, visit) = waiting(false);
    for _ in 0..STATE_REFUSALS {
        paste(&mut owner, BOUND, b"another-state", b"&code=code");
        refused(&mut owner, Refusal::State);
    }
    paste(&mut owner, BOUND, Owner::query(&visit, b"state").expect("state"), b"&code=code");
    refused(&mut owner, Refusal::Full { bound: 4 });
    let callback = owner.callback_read(listener, &head(Owner::query(&visit, b"state").expect("state"), b"&code=code"));
    assert!(owner.callback_answer(listener, callback).starts_with(b"HTTP/1.1 200"));
    signs_in(&mut owner);
}

#[test]
fn a_paste_is_refused_as_not_waiting() {
    for name in [
        "before sign-in",
        "before listener",
        "before visit",
        "at deadline",
        "past deadline",
        "after cancel",
        "after taken",
        "after terminal",
    ] {
        let (mut owner, listener, visit) =
            if name == "before sign-in" || name == "before listener" || name == "before visit" {
                let mut owner = configured(true, false);
                if name != "before sign-in" {
                    owner.ask(Request::SignIn { account: 0, subject: None });
                }
                if name == "before visit" {
                    let listener = owner.listen();
                    owner.listening(listener, PORT);
                }
                (owner, Token::new(0), Box::from([]))
            } else {
                waiting(false)
            };
        match name {
            "at deadline" => owner.env.now = Time::ZERO.saturating_add(owner.env.limits.client.sign_in_time),
            "past deadline" => {
                owner.env.now = Time::ZERO
                    .saturating_add(owner.env.limits.client.sign_in_time)
                    .saturating_add(Duration::from_secs(1));
            }
            "after cancel" => {
                owner.ask(Request::Cancel { account: 0 });
                drop(owner.above.pop());
            }
            "after taken" | "after terminal" => {
                paste(&mut owner, BOUND, Owner::query(&visit, b"state").expect("state"), b"&code=code");
                taken(&mut owner);
                if name == "after terminal" {
                    settle_listener(&mut owner, listener);
                    signs_in(&mut owner);
                }
            }
            "before sign-in" | "before listener" | "before visit" => {}
            _ => unreachable!("fixture phase"),
        }
        paste(&mut owner, BOUND, b"arbitrary", b"&code=code");
        assert_eq!(refusal_of(owner.above.pop()), Some((0, Asked::Redirected, Refusal::NotWaiting)), "{name}");
        if name == "at deadline" || name == "past deadline" {
            owner.fire();
            assert_eq!(
                failure_of(owner.above.pop()),
                Some(Failed {
                    account: 0,
                    ends: Ends::SignIn,
                    failure: Failure::Exchange(skein_oauth::Failure::TimedOut),
                }),
                "{name}"
            );
        }
    }
}

#[test]
fn a_paste_judged_between_the_listeners_read_and_its_answer_wins_and_the_listener_refuses() {
    let (mut owner, listener, visit) = waiting(false);
    let state = Owner::query(&visit, b"state").expect("state");
    let callback = owner.callback_read(listener, &head(state, b"&code=listener-code"));
    paste(&mut owner, BOUND, state, b"&code=pasted-code");
    taken(&mut owner);
    assert!(owner.callback_answer(listener, callback).starts_with(b"HTTP/1.1 404"));
    signs_in(&mut owner);
}

#[test]
fn the_listeners_redirect_taken_first_leaves_a_later_paste_not_waiting() {
    let (mut owner, listener, visit) = waiting(false);
    let state = Owner::query(&visit, b"state").expect("state");
    let callback = owner.callback_read(listener, &head(state, b"&code=listener-code"));
    owner.fire();
    paste(&mut owner, BOUND, state, b"&code=pasted-code");
    refused(&mut owner, Refusal::NotWaiting);
    assert!(owner.callback_answer(listener, callback).starts_with(b"HTTP/1.1 200"));
    signs_in(&mut owner);
}

#[test]
fn the_listeners_redirect_with_another_state_is_refused_and_the_sign_in_still_waits() {
    let (mut owner, listener, visit) = waiting(false);
    for _ in 0..STATE_REFUSALS.saturating_add(1) {
        let callback = owner.callback_read(listener, &head(b"another-state", b"&code=code"));
        assert!(owner.callback_answer(listener, callback).starts_with(b"HTTP/1.1 404"));
        assert!(owner.above.is_empty());
    }
    // Listener state refusals do not spend the owner's paste allowance.
    paste(&mut owner, BOUND, b"another-state", b"&code=code");
    refused(&mut owner, Refusal::State);
    let callback = owner.callback_read(listener, &head(Owner::query(&visit, b"state").expect("state"), b"&code=code"));
    assert!(owner.callback_answer(listener, callback).starts_with(b"HTTP/1.1 200"));
    signs_in(&mut owner);
}

#[test]
fn a_listener_redirect_at_the_deadline_is_refused_and_the_sign_in_times_out() {
    let (mut owner, listener, visit) = waiting(false);
    let callback = owner.callback_read(listener, &head(Owner::query(&visit, b"state").expect("state"), b"&code=code"));
    owner.env.now = Time::ZERO.saturating_add(owner.env.limits.client.sign_in_time);
    assert!(owner.callback_answer(listener, callback).starts_with(b"HTTP/1.1 404"));
    assert_eq!(
        failure_of(owner.above.pop()),
        Some(Failed { account: 0, ends: Ends::SignIn, failure: Failure::Exchange(skein_oauth::Failure::TimedOut) })
    );
    assert!(owner.above.is_empty() && owner.io.is_empty());
}

#[test]
fn a_confidential_redirect_is_answered_taken() {
    let mut owner = configured(false, false);
    owner.ask(Request::SignIn { account: 0, subject: None });
    let visit = owner.visit();
    paste(&mut owner, CONFIDENTIAL, Owner::query(&visit, b"state").expect("state"), b"&code=code");
    taken(&mut owner);
    signs_in(&mut owner);
}

#[test]
fn a_state_bound_under_32_is_unusable() {
    for (bound, expected) in [(31, Some(Unusable::Limits)), (32, None)] {
        let mut profile = limits();
        profile.client.state_bytes = bound;
        let mut configured = List::with_capacity(1);
        configured.push(account(true, false)).ok().expect("account room");
        assert_eq!(Component::new(configured, &profile, crypto(17)).err(), expected, "state bound {bound}");
        if bound == 32 {
            let mut owner = Owner::configured([account(true, false)], &profile);
            owner.ask(Request::SignIn { account: 0, subject: None });
            let listener = owner.listen();
            owner.listening(listener, PORT);
            let visit = owner.visit();
            assert_eq!(Owner::query(&visit, b"state").expect("state").len(), 32);
        }
    }
}

#[test]
fn an_admitted_provider_error_gets_the_return_before_the_sign_ins_failure() {
    let (mut owner, listener, visit) = waiting(true);
    owner.ask(Request::Return { account: 0, location: bytes::copy_of(b"https://app.example/back") });
    let callback =
        owner.callback_read(listener, &head(Owner::query(&visit, b"state").expect("state"), b"&error=access_denied"));
    let answer = owner.callback_answer(listener, callback);
    assert!(answer.starts_with(b"HTTP/1.1 303"));
    assert!(bytes::find(&answer, b"location: https://app.example/back\r\n").is_some());
    assert!(bytes::find(&answer, b"Sign-in received.").is_some());
    assert_eq!(
        failure_of(owner.above.pop()),
        Some(Failed { account: 0, ends: Ends::SignIn, failure: Failure::Exchange(skein_oauth::Failure::Refused) })
    );
    assert!(owner.io.is_empty() && owner.above.is_empty());
}

#[test]
fn an_owner_cancel_or_abort_stops_a_callback_waiting_for_judgement() {
    for (name, request) in [("cancel", Request::Cancel { account: 0 }), ("abort", Request::Abort)] {
        let (mut owner, listener, visit) = waiting(false);
        let callback =
            owner.callback_read(listener, &head(Owner::query(&visit, b"state").expect("state"), b"&code=code"));
        owner.ask(request);
        assert!(owner.callback_answer(listener, callback).is_empty(), "{name}");
        assert_eq!(
            failure_of(owner.above.pop()),
            Some(Failed {
                account: 0,
                ends: Ends::SignIn,
                failure: Failure::Exchange(skein_oauth::Failure::Cancelled),
            }),
            "{name}"
        );
        for _ in 0_u32..8 {
            owner.fire();
        }
        if name == "abort" {
            assert_event(owner.above.pop(), Event::Closed);
        }
        assert!(owner.above.is_empty() && owner.io.is_empty(), "{name}");
        assert!(!owner.component.has_work(), "{name}");
    }
}

#[test]
fn a_refused_listener_redirect_keeps_the_return_for_the_redirect_taken() {
    let (mut owner, listener, visit) = waiting(false);
    owner.ask(Request::Return { account: 0, location: bytes::copy_of(b"https://app.example/back") });
    let callback = owner.callback_read(listener, &head(b"another-state", b"&code=code"));
    let answer = owner.callback_answer(listener, callback);
    assert!(answer.starts_with(b"HTTP/1.1 404"));
    assert!(bytes::find(&answer, b"location:").is_none());
    let callback = owner.callback_read(listener, &head(Owner::query(&visit, b"state").expect("state"), b"&code=code"));
    let answer = owner.callback_answer(listener, callback);
    assert!(answer.starts_with(b"HTTP/1.1 303"));
    assert!(bytes::find(&answer, b"location: https://app.example/back\r\n").is_some());
    signs_in(&mut owner);
}
