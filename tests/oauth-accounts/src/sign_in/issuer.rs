//! Shared private sign-in issuer fixtures (oauth.md, 6.8).
use super::{Duration, Ipv4Addr, Process, ROOM, Story, Transport, bytes, fake, oauth, peer, registration};

pub(super) fn issuer(story: Story, large: bool) -> Process {
    make(story, large, None)
}

#[cfg(test)]
pub(super) fn with_access_token(story: Story, access: &[u8]) -> Process {
    make(story, false, Some(access))
}

fn make(story: Story, large: bool, access_token: Option<&[u8]>) -> Process {
    let registration = registration(story);
    let profile = crate::world::limits();
    let mut issuer = peer::Peer::new(
        (Ipv4Addr::LOCALHOST, 31000).into(),
        Transport::Plaintext,
        skein_fake_peers::Limits {
            io: profile.io,
            connections: 2,
            queue: ROOM,
            plaintext: 32_768,
            ciphertext: 32_768,
            observations: ROOM,
            observation_bytes: 4096,
        },
        fake::Config {
            authorization_url: registration.authorization_url,
            token_endpoint: registration.token_endpoint,
            client_id: registration.client_id,
            client_secret: registration.client_secret,
            redirect_uri: registration.redirect_uri,
            refresh_token: if large { vec![b's'; 256].into_boxed_slice() } else { bytes::copy_of(b"seed") },
            terms: port_terms(story),
        },
        fake::Limits {
            document: profile.client.document,
            uri_bytes: 256,
            request_bytes: 1024,
            codes: if matches!(story, Story::TwoPorts | Story::OpenIdAgain | Story::Return(super::Return::After)) {
                2
            } else {
                1
            },
            rotations: 2,
            plans: 2,
            carried: 3,
        },
        profile.server,
    )
    .expect("issuer");
    for (access, refresh) in [(b'a', b"r1".as_slice()), (b'b', b"r2".as_slice())] {
        issuer
            .queue(fake::Plan {
                status: 200,
                body: fake::Body::Token(oauth::TokenResponse {
                    access_token: access_token.map_or_else(
                        || {
                            vec![if story == Story::TwoPorts { b'a' } else { access }; if large { 256 } else { 16 }]
                                .into_boxed_slice()
                        },
                        bytes::copy_of,
                    ),
                    refresh_token: (story != Story::OpenIdAgain).then(|| {
                        if large {
                            vec![if access == b'a' { b'r' } else { b't' }; 256].into_boxed_slice()
                        } else {
                            bytes::copy_of(refresh)
                        }
                    }),
                    expires_in: 30,
                    scope: (story == Story::RequiredScopes).then(|| bytes::copy_of(b"read")),
                    id_token: None,
                    earliest_refresh_at: None,
                }),
                delay: Duration::from_millis(5),
                retry_after: Duration::ZERO,
            })
            .expect("plan");
    }
    Process::Issuer(Box::new(issuer))
}

pub(super) fn port_terms(story: Story) -> Option<fake::Terms> {
    if matches!(
        story,
        Story::OpenIdAgain | Story::Return(super::Return::After) | Story::IssuedClientId | Story::AnotherSubject
    ) {
        return Some(fake::Terms {
            issued: if story == Story::IssuedClientId {
                Box::new([bytes::copy_of(super::ISSUED_CLIENT)])
            } else {
                Box::new([])
            },
            omit_issued: false,
            varying_port: false,
            revocation_endpoint: None,
            openid: Some(fake::OpenId {
                issuer: bytes::copy_of(super::ISSUER),
                key_set_url: bytes::copy_of(super::KEYS),
                subject: bytes::copy_of(super::SUBJECT),
                lifetime: Duration::from_secs(60),
                header_kid: true,
                fault: None,
            }),
        });
    }
    matches!(story, Story::VaryingTaken | Story::TwoPorts).then(|| fake::Terms {
        issued: Box::new([]),
        omit_issued: false,
        revocation_endpoint: None,
        varying_port: true,
        openid: None,
    })
}

/// The independent issuer's fixed signing key set for heap fixtures.
#[must_use]
pub fn key_set() -> Box<[u8]> {
    let registration = registration(Story::OpenIdAgain);
    let profile = crate::world::limits();
    let mut issuer = fake::Issuer::new(
        fake::Config {
            authorization_url: registration.authorization_url,
            token_endpoint: registration.token_endpoint,
            client_id: registration.client_id,
            client_secret: registration.client_secret,
            redirect_uri: registration.redirect_uri,
            refresh_token: bytes::copy_of(b"seed"),
            terms: port_terms(Story::OpenIdAgain),
        },
        fake::Limits {
            document: profile.client.document,
            uri_bytes: profile.client.uri_bytes,
            request_bytes: profile.client.request_bytes,
            codes: 1,
            rotations: 1,
            plans: 1,
            carried: 3,
        },
    )
    .expect("issuer");
    let mut out = skein_lib::Queue::uncapped();
    issuer.step(
        fake::Event::Get {
            id: 0,
            endpoint: bytes::copy_of(super::KEYS),
            now: skein_lib::Time::ZERO,
            wall: skein_lib::Wall::EPOCH,
        },
        &mut out,
    );
    if let Some(fake::Request::Http(answer)) = out.pop() { answer.body } else { panic!("issuer keys") }
}
