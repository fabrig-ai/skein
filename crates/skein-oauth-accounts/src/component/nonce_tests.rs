//! Seeded inputs are drawn at admission, before a listener or private load answers.
use super::*;
use crate::tests::{crypto, limits, refresh_account};
use skein_lib::{List, Wall, Writer, bytes};

fn openid() -> (Component, Limits) {
    let mut profile = limits();
    profile.client.nonce_bytes = 32;
    profile.client.keys = 1;
    profile.client.key_set_bytes = 512;
    profile.client.metadata_bytes = 512;
    let mut account = refresh_account(0, ([127, 0, 0, 1], 31000).into());
    if let Account::SignIn { registration, .. } = &mut account {
        registration.terms = Some(oauth::Terms {
            openid: Some(oauth::OpenId { issuer: bytes::copy_of(b"http://127.0.0.1:31000/"), key_set: None }),
            ..crate::tests::terms()
        });
        registration.scope = bytes::copy_of(b"openid");
    }
    let mut accounts = List::with_capacity(1);
    accounts.push(account).ok().expect("one account");
    (Component::new(accounts, &profile, crypto(17)).expect("OpenID account"), profile)
}

#[test]
fn the_nonce_is_drawn_from_the_seeded_generator() {
    let (mut component, limits) = openid();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    component.down(
        &env,
        Request::SignIn { account: 0, subject: None },
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
        &mut Queue::uncapped(),
    );
    let id = component.bindings.get(0).copied().flatten().expect("sign-in bound");
    let exchange = component.exchanges.get(id).expect("admitted exchange");
    let nonce = match &exchange.sign_in {
        Some(oauth::Event::SignIn { nonce, .. }) => nonce.as_deref(),
        Some(
            oauth::Event::Redirected { .. }
            | oauth::Event::Refresh { .. }
            | oauth::Event::Revoke { .. }
            | oauth::Event::Http(_)
            | oauth::Event::Tick { .. }
            | oauth::Event::Cancel
            | oauth::Event::Reset,
        )
        | None => None,
    };
    let mut raw = [0; 80];
    crypto(17).fill(&mut raw);
    let mut expected = Writer::new(32);
    skein_lib::base64::encode(skein_lib::base64::Alphabet::Url, &raw[56..], &mut expected).expect("nonce room");
    assert!(nonce == Some(expected.finish().as_ref()));
}
