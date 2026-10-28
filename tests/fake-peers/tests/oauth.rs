//! Actual issuer listener admission and settlement.
use skein_fake_oauth as issuer;
use skein_fake_peers::{Limits, Transport, oauth};
use skein_io::kernel::{Done, Family, Op};
use skein_lib::{Duration, bytes};
use skein_shell::Host;

use std::net::{Ipv4Addr, SocketAddr};

use skein_fake_peers_tests as support;

fn make(transport: Transport, terms: Option<issuer::Terms>) -> oauth::Peer {
    make_with_cap(transport, terms, 16)
}

fn make_with_cap(transport: Transport, terms: Option<issuer::Terms>, observations: u32) -> oauth::Peer {
    make_with_cap_at(transport, terms, observations, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
}

fn make_with_cap_at(
    transport: Transport,
    terms: Option<issuer::Terms>,
    observations: u32,
    address: SocketAddr,
) -> oauth::Peer {
    let scheme = if transport == Transport::Plaintext { "http" } else { "https" };
    let document = skein_oauth::Limits {
        document_bytes: 1024,
        string_bytes: 256,
        token_bytes: 256,
        client_bytes: 64,
        detail_bytes: 64,
        record_bytes: 2048,
        depth: 8,
        tokens: 64,
        issued_client_bytes: 0,
        granted_bytes: 64,
        subject_bytes: 0,
        id_token_bytes: 1024,
    };
    let limits = Limits {
        io: skein_io::Limits {
            sockets: 3,
            refusals: 1,
            intake: 32_768,
            receive: 1024,
            output: 32_768,
            sends: 2,
            accepts: 1,
            backlog: 1,
            close_timeout: Duration::from_secs(1),
            names: 0,
            retry: Duration::from_millis(1),
        },
        connections: 2,
        queue: 64,
        plaintext: 32_768,
        ciphertext: 32_768,
        observations,
        observation_bytes: 4096,
    };
    oauth::Peer::new(
        address,
        transport,
        limits,
        issuer::Config {
            authorization_url: format!("{scheme}://127.0.0.1/authorize").into_bytes().into_boxed_slice(),
            token_endpoint: format!("{scheme}://127.0.0.1/token").into_bytes().into_boxed_slice(),
            client_id: bytes::copy_of(b"client"),
            client_secret: None,
            redirect_uri: bytes::copy_of(b"http://127.0.0.1:1234/callback"),
            refresh_token: bytes::copy_of(b"seed"),
            terms,
        },
        issuer::Limits { document, uri_bytes: 256, request_bytes: 1024, codes: 2, rotations: 2, plans: 2, carried: 1 },
        skein_http::server::Limits { head: 2048, headers: 16, body: 1024, read: 256, response: 2059, send: 512 },
    )
    .expect("the issuer fixture has usable limits")
}

#[test]
fn issuer_process_binds_and_settles_for_both_transports() {
    for transport in [Transport::Plaintext, Transport::Tls] {
        let mut peer = make(transport, None);
        let mut sim = skein_sim::Sim::new(31, skein_sim::Config::calm());
        let pid = sim.spawn_process();
        for _ in 0..100 {
            sim.reap(pid, peer.completions());
            peer.iterate(sim.now(), sim.wall());
            sim.submit(pid, peer.submissions());
            if peer.address().is_some() {
                break;
            }
            if !peer.work_pending(sim.now())
                && let Some(next) = sim.next_due()
            {
                sim.advance_to(next);
            }
        }
        let address = skein_fake_peers::Listening::address(&peer);
        assert_eq!(Some(address), peer.address());
        assert!(address.ip().is_loopback());
        let observations = peer.observations().len();
        skein_fake_peers::Listening::shutdown(&mut peer);
        for _ in 0..100 {
            sim.reap(pid, peer.completions());
            peer.iterate(sim.now(), sim.wall());
            sim.submit(pid, peer.submissions());
            if peer.is_empty() {
                break;
            }
            if !peer.work_pending(sim.now())
                && let Some(next) = sim.next_due()
            {
                sim.advance_to(next);
            }
        }
        assert!(peer.is_empty());
        assert_eq!(peer.observations().len(), observations);
        sim.assert_quiescent(pid);
        sim.assert_no_open_fds(pid);
    }
}

#[test]
fn a_connection_past_the_peers_limit_is_rejected_and_the_peer_settles() {
    let mut peer = make(Transport::Plaintext, None);
    let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
    let (pid, client) = (sim.spawn_process(), sim.spawn_process());
    for _ in 0..100 {
        sim.reap(pid, peer.completions());
        peer.iterate(sim.now(), sim.wall());
        sim.submit(pid, peer.submissions());
        if peer.address().is_some() {
            break;
        }
        if let Some(next) = sim.next_due() {
            sim.advance_to(next);
        }
    }
    let Done::Fd(fd) = support::call(&mut sim, client, 1, Op::Socket { family: Family::Ipv4 }) else {
        panic!("a socket returns its descriptor");
    };
    assert_eq!(support::call(&mut sim, client, 2, Op::Connect { fd, addr: peer.address().unwrap() }), Done::Nothing);
    // The accept is already in flight when admission stops.
    peer.shutdown();
    for _ in 0..100 {
        sim.reap(pid, peer.completions());
        peer.iterate(sim.now(), sim.wall());
        sim.submit(pid, peer.submissions());
        if peer.is_empty() {
            break;
        }
        if let Some(next) = sim.next_due() {
            sim.advance_to(next);
        }
    }
    assert!(peer.is_empty());
    sim.assert_quiescent(pid);
    sim.assert_no_open_fds(pid);
    assert_eq!(support::call(&mut sim, client, 3, Op::Close { fd }), Done::Nothing);
    sim.assert_quiescent(client);
    sim.assert_no_open_fds(client);
}

fn terms() -> issuer::Terms {
    issuer::Terms {
        issued: Box::new([]),
        omit_issued: false,
        revocation_endpoint: Some(bytes::copy_of(b"http://127.0.0.1/revoke")),
        varying_port: false,
        openid: Some(issuer::OpenId {
            issuer: bytes::copy_of(b"http://127.0.0.1"),
            key_set_url: bytes::copy_of(b"http://127.0.0.1/jwks"),
            subject: bytes::copy_of(b"person"),
            lifetime: Duration::from_secs(30),
            header_kid: true,
            fault: None,
        }),
    }
}

fn pass(sim: &mut skein_sim::Sim, pid: skein_sim::Pid, peer: &mut oauth::Peer) {
    sim.reap(pid, peer.completions());
    peer.iterate(sim.now(), sim.wall());
    sim.submit(pid, peer.submissions());
    sim.advance();
}

fn fetch(
    sim: &mut skein_sim::Sim,
    (pid, peer): (skein_sim::Pid, &mut oauth::Peer),
    client: skein_sim::Pid,
    request: &[u8],
) -> Vec<u8> {
    use skein_io::kernel::Submit;
    use skein_lib::{Queue, Token};
    for _ in 0..1000 {
        if peer.address().is_some() {
            break;
        }
        pass(sim, pid, peer);
    }
    let Done::Fd(fd) = support::call(sim, client, 1, Op::Socket { family: Family::Ipv4 }) else {
        panic!("socket descriptor")
    };
    support::call(sim, client, 2, Op::Connect { fd, addr: peer.address().expect("listener") });
    let mut from = 0;
    while usize::try_from(from).expect("request length") < request.len() {
        let Done::Count(sent) = support::call(sim, client, 3, Op::Send { fd, bytes: request.into(), from }) else {
            panic!("send count")
        };
        from += sent;
    }
    let mut reply = Vec::new();
    loop {
        let mut submissions = Queue::with_capacity(1);
        let mut completions = Queue::with_capacity(1);
        submissions.push(Submit { op: Token::new(4), kind: Op::Recv { fd, buf: Box::new([0; 4096]) } });
        sim.submit(client, &mut submissions);
        let mut received = None;
        for _ in 0..1000 {
            sim.reap(client, &mut completions);
            if let Some(complete) = completions.pop() {
                received = Some(complete);
                break;
            }
            pass(sim, pid, peer);
        }
        let complete = received.expect("response arrives");
        let Done::Count(count) = complete.result.expect("read succeeds") else { panic!("read count") };
        if count == 0 {
            break;
        }
        let Op::Recv { buf, .. } = complete.kind else { panic!("receive buffer") };
        reply.extend_from_slice(&buf[..usize::try_from(count).expect("read count")]);
    }
    support::call(sim, client, 5, Op::Close { fd });
    for _ in 0..100 {
        pass(sim, pid, peer);
    }
    reply
}

fn request(method: &str, target: &str, body: &str) -> Vec<u8> {
    format!("{method} {target} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\ncontent-type: application/x-www-form-urlencoded\r\ncontent-length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn world() -> (skein_sim::Sim, skein_sim::Pid, skein_sim::Pid, oauth::Peer) {
    let mut sim = skein_sim::Sim::new(31, skein_sim::Config::calm());
    let (pid, client) = (sim.spawn_process(), sim.spawn_process());
    (sim, pid, client, make(Transport::Plaintext, Some(terms())))
}

fn settle(sim: &mut skein_sim::Sim, pid: skein_sim::Pid, client: skein_sim::Pid, peer: &mut oauth::Peer) {
    peer.shutdown();
    for _ in 0..1000 {
        if peer.is_empty() {
            break;
        }
        pass(sim, pid, peer);
    }
    assert!(peer.is_empty());
    for process in [pid, client] {
        sim.assert_quiescent(process);
        sim.assert_no_open_fds(process);
    }
}

#[test]
fn an_oauth_peer_answers_on_its_adopted_listener() {
    let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
    let (server, client) = (sim.spawn_process(), sim.spawn_process());
    let (fd, address) = support::held_listener(&mut sim, server);
    let mut peer = make_with_cap_at(Transport::Plaintext, Some(terms()), 16, address);
    peer.adopt_listener(fd, address).expect("held listener before the first drive");
    let reply = fetch(&mut sim, (server, &mut peer), client, &request("GET", "/jwks", ""));
    assert_eq!(peer.address(), Some(address));
    assert!(reply.starts_with(b"HTTP/1.1 200"));
    assert!(peer.observations().iter().any(|record| {
        matches!(record, oauth::Observation::Document { target, .. } if target.as_ref() == b"/jwks")
    }));
    settle(&mut sim, server, client, &mut peer);
}

#[test]
fn the_metadata_and_key_set_are_served_on_loopback() {
    let (mut sim, pid, client, mut peer) = world();
    for target in ["/.well-known/openid-configuration", "/jwks"] {
        let reply = fetch(&mut sim, (pid, &mut peer), client, &request("GET", target, ""));
        assert!(reply.starts_with(b"HTTP/1.1 200"));
    }
    let documents: Vec<&[u8]> =
        peer.observations()
            .iter()
            .filter_map(|record| {
                if let oauth::Observation::Document { target, .. } = record { Some(target.as_ref()) } else { None }
            })
            .collect();
    assert_eq!(documents, [b"/.well-known/openid-configuration".as_slice(), b"/jwks"]);
    assert!(!peer.observations().iter().any(|record| matches!(record, oauth::Observation::Answered { .. })));
    settle(&mut sim, pid, client, &mut peer);
}

fn plan(status: u16) -> issuer::Plan {
    issuer::Plan {
        status,
        body: issuer::Body::Token(skein_oauth::TokenResponse {
            access_token: bytes::copy_of(ACCESS),
            refresh_token: Some(bytes::copy_of(REFRESH)),
            expires_in: 30,
            scope: None,
            id_token: None,
            earliest_refresh_at: None,
        }),
        delay: Duration::ZERO,
        retry_after: Duration::ZERO,
    }
}

// Scripted credential values appear only in this fixture.
const ACCESS: &[u8] = b"scripted-access-token";
const REFRESH: &[u8] = b"scripted-refresh-token";
const HINT: &str = "held.header.signature";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

fn sign_in(sim: &mut skein_sim::Sim, pid: skein_sim::Pid, client: skein_sim::Pid, peer: &mut oauth::Peer) -> Vec<u8> {
    peer.queue(plan(200)).expect("sign-in plan");
    let target = format!(
        "/authorize?response_type=code&client_id=client&redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcallback&state=0123456789abcdef&code_challenge={CHALLENGE}&code_challenge_method=S256&id_token_hint={HINT}"
    );
    let reply = fetch(sim, (pid, peer), client, &request("GET", &target, ""));
    let code = reply.windows(b"?code=".len()).position(|part| part == b"?code=").expect("redirect code");
    let code = &reply[code + b"?code=".len()..];
    let code = code.split(|byte| *byte == b'&').next().expect("code field");
    let body = format!(
        "grant_type=authorization_code&client_id=client&redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcallback&code={}&code_verifier={VERIFIER}",
        std::str::from_utf8(code).expect("code text")
    );
    fetch(sim, (pid, peer), client, &request("POST", "/token", &body));
    code.to_vec()
}

#[test]
fn a_revocation_is_answered_as_scripted_on_loopback() {
    for status in [200, 400] {
        let (mut sim, pid, client, mut peer) = world();
        peer.queue(plan(status)).unwrap();
        let body = "client_id=client&token=seed&token_type_hint=refresh_token";
        let reply = fetch(&mut sim, (pid, &mut peer), client, &request("POST", "/revoke", body));
        let status_line = format!("HTTP/1.1 {status}");
        assert!(reply.starts_with(status_line.as_bytes()));
        let answered: Vec<u16> = peer
            .observations()
            .iter()
            .filter_map(
                |record| {
                    if let oauth::Observation::Answered { status, .. } = record { Some(*status) } else { None }
                },
            )
            .collect();
        assert_eq!(answered, [status]);
        assert!(peer.observations().iter().any(|record| matches!(record, oauth::Observation::Revoke { .. })));
        settle(&mut sim, pid, client, &mut peer);
    }
}

#[test]
fn no_observation_holds_a_token_or_an_id_token() {
    let (mut sim, pid, client, mut peer) = world();
    let code = sign_in(&mut sim, pid, client, &mut peer);
    peer.queue(plan(200)).unwrap();
    let refresh =
        format!("grant_type=refresh_token&client_id=client&refresh_token={}", std::str::from_utf8(REFRESH).unwrap());
    fetch(&mut sim, (pid, &mut peer), client, &request("POST", "/token", &refresh));
    peer.queue(plan(200)).unwrap();
    let revoke =
        format!("client_id=client&token_type_hint=refresh_token&token={}", std::str::from_utf8(REFRESH).unwrap());
    fetch(&mut sim, (pid, &mut peer), client, &request("POST", "/revoke", &revoke));
    let targets: Vec<&[u8]> = peer
        .observations()
        .iter()
        .filter_map(|record| {
            if let oauth::Observation::Authorize { target, .. } | oauth::Observation::Document { target, .. } = record {
                Some(target.as_ref())
            } else {
                None
            }
        })
        .collect();
    for secret in [ACCESS, REFRESH, HINT.as_bytes(), code.as_slice()] {
        assert!(
            !targets.iter().any(|target| target.windows(secret.len()).any(|part| part == secret)),
            "observation contains a credential value"
        );
    }
    assert!(targets.iter().any(|target| target.windows(b"id_token_hint".len()).any(|part| part == b"id_token_hint")));
    settle(&mut sim, pid, client, &mut peer);
}

#[test]
fn without_records_serves_past_a_zero_cap() {
    for recording in [true, false] {
        let mut peer = make_with_cap(Transport::Plaintext, Some(terms()), 0);
        if !recording {
            peer = peer.without_records();
        }
        let mut sim = skein_sim::Sim::new(31, skein_sim::Config::calm());
        let (pid, client) = (sim.spawn_process(), sim.spawn_process());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fetch(&mut sim, (pid, &mut peer), client, &request("GET", "/jwks", ""))
        }));
        if recording {
            support::assert_panic(result, "world observation count cap");
        } else {
            assert!(result.unwrap().starts_with(b"HTTP/1.1 200"));
            assert!(peer.observations().is_empty());
            settle(&mut sim, pid, client, &mut peer);
        }
    }
}
