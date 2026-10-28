//! Actual io listener admission and clean process settlement.
use std::net::{Ipv4Addr, SocketAddr};

use skein_fake_llm_domain::{self as domain, api};
use skein_fake_llm_protocol::{documents, provider};
use skein_fake_peers::{Limits, Transport, llm};
use skein_io::kernel::{Done, Family, Op};
use skein_lib::{Duration, Time};
use skein_llm::{Credential, Endpoint, Provider, Scheme};
use skein_shell::Host;

#[path = "llm/request.rs"]
mod request;
#[path = "llm/tls.rs"]
mod tls;
use skein_fake_peers_tests as support;

fn limits() -> Limits {
    Limits {
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
        observations: 16,
        observation_bytes: 32_768,
    }
}

fn make(address: SocketAddr, transport: Transport) -> Result<llm::Peer, skein_fake_peers::Error> {
    let call = skein_llm_world::call(1);
    make_on(call.endpoint, call.credential.expect("fixture credential"), address, transport)
}

fn make_on(
    endpoint: Endpoint,
    credential: Credential,
    address: SocketAddr,
    transport: Transport,
) -> Result<llm::Peer, skein_fake_peers::Error> {
    make_scripted(endpoint, credential, address, transport, Box::new([]))
}

fn make_scripted(
    endpoint: Endpoint,
    credential: Credential,
    address: SocketAddr,
    transport: Transport,
    scripts: Box<[api::Script]>,
) -> Result<llm::Peer, skein_fake_peers::Error> {
    make_scripted_with_limits(endpoint, credential, address, transport, scripts, limits())
}

fn make_scripted_with_limits(
    endpoint: Endpoint,
    credential: Credential,
    address: SocketAddr,
    transport: Transport,
    scripts: Box<[api::Script]>,
    limits: Limits,
) -> Result<llm::Peer, skein_fake_peers::Error> {
    let machine_limits = skein_llm_world::fake::config();
    let domain = domain::Domain::configured(
        &machine_limits,
        23,
        scripts,
        api::Menu { arguments: Box::new([]), invalid: Box::new([]) },
    )
    .expect("configured script domain");
    make_domain_with_limits(endpoint, credential, address, transport, domain, limits)
}

fn make_domain_with_limits(
    endpoint: Endpoint,
    credential: Credential,
    address: SocketAddr,
    transport: Transport,
    domain: domain::Domain,
    limits: Limits,
) -> Result<llm::Peer, skein_fake_peers::Error> {
    let machine_limits = skein_llm_world::fake::config();
    llm::Peer::new(
        address,
        transport,
        limits,
        provider::Config {
            without_credential: false,
            usage_fields: skein_fake_llm_protocol::documents::UsageFields::ALL,
            echo: skein_llm::openai::Echo::NONE,
            provider: match endpoint.provider {
                Provider::OpenAi | Provider::OpenAiCodex => documents::Provider::OpenAi,
                Provider::Anthropic => documents::Provider::Anthropic,
            },
            path: endpoint.target,
            headers: Box::new([]),
        },
        credential,
        skein_llm_world::fake::limits(&skein_llm_world::limits()),
        domain,
        machine_limits,
    )
}

#[test]
fn fake_peer_binds_loopback_and_shutdown_settles_the_actual_listener() {
    for transport in [Transport::Plaintext, Transport::Tls] {
        let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
        let pid = sim.spawn_process();
        let mut peer = make(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), transport).unwrap();
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
        assert_ne!(peer.address().unwrap().port(), 0);
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
        assert!(!peer.work_pending(Time::ZERO));
    }
}

#[test]
fn fake_peer_refuses_non_loopback_configuration_before_effects() {
    assert!(matches!(
        make(SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 80)), Transport::Plaintext),
        Err(skein_fake_peers::Error::Limits)
    ));
}

#[test]
fn peer_refuses_unrepresentable_faces_and_incompatible_http_buffers() {
    let good = limits();
    let protocol = skein_llm_world::fake::limits(&skein_llm_world::limits());
    assert!(llm::valid(&good, &protocol));
    for bad in [
        Limits { connections: 0, ..good },
        Limits { connections: u32::MAX, ..good },
        Limits { queue: 63, ..good },
        Limits { plaintext: 1, ..good },
        Limits { ciphertext: 18_431, ..good },
        Limits { io: skein_io::Limits { sockets: u32::MAX, ..good.io }, ..good },
        Limits { io: skein_io::Limits { output: good.ciphertext - 1, ..good.io }, ..good },
    ] {
        assert!(!llm::valid(&bad, &protocol));
        let call = skein_llm_world::call(1);
        assert_eq!(
            make_scripted_with_limits(
                call.endpoint,
                call.credential.unwrap(),
                (Ipv4Addr::LOCALHOST, 0).into(),
                Transport::Plaintext,
                Box::new([]),
                bad
            )
            .err(),
            Some(skein_fake_peers::Error::Limits),
        );
    }
}

#[test]
fn llm_admission_keeps_its_ciphertext_reserve_and_send_allowance() {
    let limits = limits();
    let mut protocol = skein_llm_world::fake::limits(&skein_llm_world::limits());
    protocol.http.send = limits.ciphertext - 1024 - 32;
    assert!(llm::valid(&limits, &protocol));
    protocol.http.send += 1;
    assert!(!llm::valid(&limits, &protocol));
}

#[test]
fn provider_constructors_refuse_unrepresentable_edges() {
    let good = skein_llm_world::fake::limits(&skein_llm_world::limits());
    assert!(provider::valid(&good));
    let mut send = good;
    send.http.send = u32::MAX;
    let mut chunk = good;
    chunk.sse.chunk = 0;
    let mut request = good;
    request.documents.openai.request = u32::MAX;
    let mut model = good;
    model.documents.model_ceiling = 0;
    let mut head = good;
    head.http.head = 1;
    for bad in [send, chunk, request, model, head] {
        assert!(!provider::valid(&bad));
        assert_eq!(provider::Server::new(skein_lib::Token::new(1), &bad).err(), Some(provider::Error::Limits));
        let config = provider::Config {
            without_credential: false,
            usage_fields: documents::UsageFields::ALL,
            echo: skein_llm::openai::Echo::NONE,
            provider: documents::Provider::OpenAi,
            path: Box::from(b"/".as_slice()),
            headers: Box::new([]),
        };
        assert_eq!(provider::Service::new(config, &bad).err(), Some(provider::Error::Limits));
    }
}

#[test]
fn a_connection_past_the_peers_limit_is_rejected_and_the_peer_settles() {
    let mut peer = make(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Transport::Plaintext).unwrap();
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

#[test]
fn a_release_name_past_64_bytes_is_refused_as_limits() {
    let mut peer = make(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Transport::Plaintext).unwrap();
    for name in [b"".as_slice(), &[b'x'; 65]] {
        assert_eq!(peer.release(name), Err(skein_fake_peers::Error::Limits), "name length {}", name.len());
    }
    for _ in 0..skein_llm_world::fake::config().calls {
        assert_eq!(peer.release(&[b'x'; 64]), Ok(()));
    }
}

#[test]
fn a_release_past_the_queue_is_refused_as_limits() {
    let mut peer = make(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Transport::Plaintext).unwrap();
    for _ in 0..skein_llm_world::fake::config().calls {
        assert_eq!(peer.release(b"ready"), Ok(()));
    }
    assert_eq!(peer.release(b"extra"), Err(skein_fake_peers::Error::Limits));
    peer.iterate(Time::ZERO, skein_lib::Wall::EPOCH);
    assert_eq!(peer.release(b"next"), Ok(()));
}

#[test]
fn a_queued_release_keeps_the_peer_working() {
    let mut peer = make(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Transport::Plaintext).unwrap();
    peer.iterate(Time::ZERO, skein_lib::Wall::EPOCH);
    assert!(!peer.work_pending(Time::ZERO));
    assert_eq!(peer.release(b"ready"), Ok(()));
    assert!(peer.work_pending(Time::ZERO));
    peer.iterate(Time::ZERO, skein_lib::Wall::EPOCH);
    assert!(!peer.work_pending(Time::ZERO));
}

fn key() -> Credential {
    Credential::key(b"k0".as_slice().into())
}

fn bearer() -> Credential {
    Credential::bearer(b"b1".as_slice().into())
}

fn local_peer(endpoint: Endpoint, credential: Credential) -> llm::Peer {
    make_on(endpoint, credential, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), Transport::Plaintext)
        .expect("configured peer")
}

#[test]
fn accept_returns_each_credentials_index_in_order() {
    let mut peer = local_peer(Endpoint::anthropic(), key());
    for index in 1..provider::MAX_CREDENTIALS {
        assert_eq!(peer.accept(bearer()), Ok(index), "index {index}");
    }
}

#[test]
fn accept_refuses_a_list_the_route_cannot_serve() {
    let codex = || Credential::codex(b"b0".as_slice().into(), b"a0".as_slice().into());
    let cap = skein_llm_world::fake::limits(&skein_llm_world::limits()).http.head;
    let rows = [
        (Endpoint::anthropic(), key(), 1, Credential { account_id: b"a0".as_slice().into(), ..key() }),
        (Endpoint::openai(), key(), 1, codex()),
        (Endpoint::codex(), codex(), 1, bearer()),
        (Endpoint::anthropic(), key(), provider::MAX_CREDENTIALS, bearer()),
        (
            Endpoint::anthropic(),
            key(),
            1,
            Credential::key(vec![b'x'; usize::try_from(cap).unwrap() - key().value.len() + 1].into_boxed_slice()),
        ),
    ];
    for (row, (endpoint, first, count, added)) in rows.into_iter().enumerate() {
        let mut peer = local_peer(endpoint, first);
        for _ in 1..count {
            peer.accept(bearer()).expect("list filled to its bound");
        }
        assert_eq!(peer.accept(added), Err(skein_fake_peers::Error::Limits), "row {row}");
    }
}

const FUTURE_CREDENTIAL: u32 = 1; // Peer::new accepts index 0; accept adds index 1.

fn anthropic_reset_script() -> api::Script {
    api::Script {
        cue: b"hi".as_slice().into(),
        turns: Box::new([api::Turn {
            lines: Box::new([api::Line::Fail {
                error: api::Error::Exhausted { retry_after: Some(Duration::from_secs(1)) },
                report: api::Report::none(),
                credentials: Box::new([FUTURE_CREDENTIAL]),
            }]),
            finish: api::Finish::Stop,
            tokens: 0,
        }]),
    }
}

#[test]
fn accept_refuses_a_key_at_an_anthropic_reset_lines_index() {
    let scripts = Box::new([anthropic_reset_script()]);
    let mut peer = make_scripted(
        Endpoint::anthropic(),
        bearer(),
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Transport::Plaintext,
        scripts,
    )
    .unwrap();
    assert_eq!(peer.accept(key()), Err(skein_fake_peers::Error::Limits));
}

#[test]
fn played_failures_validate_future_credentials_in_every_group() {
    let script = anthropic_reset_script();
    let rows = [
        ("cued", api::Scripts { cued: Box::new([script.clone()]), asked: Box::new([]), otherwise: Box::new([]) }),
        ("asked", api::Scripts { cued: Box::new([]), asked: Box::new([script.clone()]), otherwise: Box::new([]) }),
        ("otherwise", api::Scripts { cued: Box::new([]), asked: Box::new([]), otherwise: script.turns }),
    ];
    for (label, scripts) in rows {
        let config = skein_llm_world::fake::config();
        let domain =
            domain::Domain::played(&config, 23, scripts, api::Menu { arguments: Box::new([]), invalid: Box::new([]) })
                .expect("played script domain");
        let mut peer = make_domain_with_limits(
            Endpoint::anthropic(),
            bearer(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            Transport::Plaintext,
            domain,
            limits(),
        )
        .expect("future credential indices are admitted");
        assert_eq!(peer.accept(key()), Err(skein_fake_peers::Error::Limits), "{label}");
        assert_eq!(peer.accept(bearer()), Ok(FUTURE_CREDENTIAL), "{label}");
    }
}

fn send_request(peer: &mut llm::Peer, endpoint: &Endpoint, fields: &[u8], body: &[u8]) -> Vec<u8> {
    let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
    let (server, client) = (sim.spawn_process(), sim.spawn_process());
    request::request(&mut sim, server, client, peer, &endpoint.target, fields, body)
}

#[test]
fn an_exhausted_account_gets_reset_evidence_then_its_turns_reply() {
    const DELAY: Duration = Duration::from_secs(2);
    let endpoint = Endpoint::openai();
    let script = api::Script {
        cue: b"limited".as_slice().into(),
        turns: Box::new([api::Turn {
            lines: Box::new([
                api::Line::Fail {
                    error: api::Error::Exhausted { retry_after: Some(DELAY) },
                    report: api::Report::none(),
                    credentials: Box::new([0]),
                },
                api::Line::Text { text: b"through".as_slice().into() },
            ]),
            finish: api::Finish::Stop,
            tokens: 1,
        }]),
    };
    let mut peer = make_scripted(
        endpoint.clone(),
        key(),
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Transport::Plaintext,
        Box::new([script]),
    )
    .expect("scripted peer");
    let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
    let (server, client) = (sim.spawn_process(), sim.spawn_process());
    let body = br#"{"model":"gpt","store":false,"instructions":"limited","input":[{"role":"user","content":[{"type":"input_text","text":"hi"}]}],"stream":true}"#;
    let fields = b"authorization: Bearer k0\r\n";
    let failed = request::request(&mut sim, server, client, &mut peer, &endpoint.target, fields, body);
    let failed = core::str::from_utf8(&failed).expect("HTTP response");
    assert!(failed.starts_with("HTTP/1.1 429"), "{failed}");
    assert!(failed.contains(r#""resets_in_seconds":2"#), "{failed}");
    assert!(!failed.to_ascii_lowercase().contains("retry-after"), "{failed}");
    sim.advance_to(sim.now().saturating_add(DELAY));
    let answered = request::request(&mut sim, server, client, &mut peer, &endpoint.target, fields, body);
    let answered = core::str::from_utf8(&answered).expect("HTTP response");
    assert!(answered.starts_with("HTTP/1.1 200"), "{answered}");
    assert!(answered.contains(r#""text":"through""#), "{answered}");
    assert!(answered.contains("response.completed"), "{answered}");
    assert!(!answered.contains("resets_in_seconds"), "{answered}");
    peer.shutdown();
    for _ in 0..1000 {
        request::pass(&mut sim, server, &mut peer);
        if peer.is_empty() {
            break;
        }
    }
    assert!(peer.is_empty());
    sim.assert_quiescent(server);
    sim.assert_no_open_fds(server);
    sim.assert_no_open_fds(client);
}

#[test]
fn accept_is_refused_once_a_request_has_reached_the_peer() {
    let endpoint = Endpoint::anthropic();
    let mut peer = local_peer(endpoint.clone(), key());
    send_request(&mut peer, &endpoint, b"x-api-key: k0\r\n", b"");
    assert_eq!(peer.accept(bearer()), Err(skein_fake_peers::Error::Limits));
}

struct Seen {
    presented: Option<provider::Presented>,
    query: Vec<Option<u32>>,
}

fn seen(peer: &llm::Peer) -> Seen {
    Seen {
        presented: peer.observations().iter().find_map(|record| match record {
            llm::Observation::Credential { route, scheme, index, .. } => {
                Some(provider::Presented { route: *route, scheme: *scheme, index: *index })
            }
            llm::Observation::Accepted { .. }
            | llm::Observation::Head { .. }
            | llm::Observation::Query { .. }
            | llm::Observation::Answered { .. }
            | llm::Observation::Closed { .. } => None,
        }),
        query: peer
            .observations()
            .iter()
            .filter_map(|record| match record {
                llm::Observation::Query { query, .. } => Some(query.credential),
                llm::Observation::Accepted { .. }
                | llm::Observation::Head { .. }
                | llm::Observation::Credential { .. }
                | llm::Observation::Answered { .. }
                | llm::Observation::Closed { .. } => None,
            })
            .collect(),
    }
}

fn observation_rows() -> [(Endpoint, &'static [u8], [ObservationRow; 5]); 2] {
    [
        (
            Endpoint::anthropic(),
            br#"{"model":"claude","messages":[{"role":"user","content":"hi"}],"max_tokens":16,"stream":true}"#
                .as_slice(),
            [
                (b"x-api-key: k0\r\n".as_slice(), Some(Scheme::Key), Some(0), &[Some(0)]),
                (b"authorization: Bearer b1\r\n", Some(Scheme::Bearer), Some(1), &[Some(1)]),
                (b"x-api-key: unknown\r\n", Some(Scheme::Key), None, &[]),
                (b"authorization: Bearer unknown\r\n", Some(Scheme::Bearer), None, &[]),
                (b"", None, None, &[]),
            ],
        ),
        (
            Endpoint::openai(),
            br#"{"model":"gpt","store":false,"instructions":"","input":[{"role":"user","content":[{"type":"input_text","text":"hi"}]}],"stream":true}"#
                .as_slice(),
            [
                (b"authorization: Bearer k0\r\n".as_slice(), Some(Scheme::Key), Some(0), &[Some(0)]),
                (b"authorization: Bearer b1\r\n", Some(Scheme::Bearer), Some(1), &[Some(1)]),
                (b"authorization: Bearer unknown-key\r\n", Some(Scheme::Bearer), None, &[]),
                (b"authorization: Bearer unknown\r\n", Some(Scheme::Bearer), None, &[]),
                (b"", None, None, &[]),
            ],
        ),
    ]
}

type ObservationRow = (&'static [u8], Option<Scheme>, Option<u32>, &'static [Option<u32>]);

#[test]
fn each_request_is_observed_by_route_scheme_and_index() {
    for (endpoint, body, rows) in observation_rows() {
        for (row, (fields, scheme, index, query)) in rows.into_iter().enumerate() {
            let mut peer = local_peer(endpoint.clone(), key());
            peer.accept(bearer()).expect("second credential accepted");
            send_request(&mut peer, &endpoint, fields, body);
            let got = seen(&peer);
            assert_eq!(
                (got.presented, got.query.as_slice()),
                (Some(provider::Presented { route: endpoint.provider, scheme, index }), query),
                "{:?} row {row}",
                endpoint.provider
            );
        }
    }
}

#[test]
fn no_head_observation_holds_a_credential_field() {
    let endpoint = Endpoint::anthropic();
    let mut peer = local_peer(endpoint.clone(), key());
    send_request(
        &mut peer,
        &endpoint,
        b"Authorization: Bearer b1\r\nX-API-Key: k0\r\nChatgpt-Account-Id: a0\r\nx-visible: visible\r\n",
        b"",
    );
    let names: Vec<_> = peer
        .observations()
        .iter()
        .filter_map(|record| match record {
            llm::Observation::Head { headers, .. } => {
                Some(headers.iter().map(|header| header.name.as_ref()).collect::<Vec<_>>())
            }
            llm::Observation::Accepted { .. }
            | llm::Observation::Credential { .. }
            | llm::Observation::Query { .. }
            | llm::Observation::Answered { .. }
            | llm::Observation::Closed { .. } => None,
        })
        .collect();
    assert_eq!(names, [vec![b"host".as_slice(), b"content-type", b"connection", b"content-length", b"x-visible"]]);
}

fn body(extra: &str) -> Vec<u8> {
    format!(r#"{{"model":"gpt","store":false,"stream":true,"instructions":"","input":[{{"role":"user","content":[{{"type":"input_text","text":"hi"}}]}}]{extra}}}"#).into_bytes()
}

#[test]
fn a_refused_field_is_named() {
    let limits = skein_llm_world::fake::limits(&skein_llm_world::limits()).documents;
    let fields: [&[u8]; 15] = [
        b"background",
        b"conversation",
        b"max_output_tokens",
        b"max_tool_calls",
        b"metadata",
        b"moderation",
        b"multi_agent",
        b"prompt",
        b"prompt_cache_retention",
        b"safety_identifier",
        b"temperature",
        b"top_logprobs",
        b"top_p",
        b"truncation",
        b"user",
    ];
    for field in fields {
        let extra = format!(",\"{}\":null", std::str::from_utf8(field).unwrap());
        let got = documents::request_on(Provider::OpenAi, &body(&extra), &limits).map(|_| ());
        assert_eq!(got, Err(documents::Error::Refused(field)));
    }
    for (route, extra) in [
        (Provider::OpenAi, ""),
        (Provider::OpenAi, r#", "x-extra":{"top_p":null}"#),
        (Provider::OpenAiCodex, r#", "top_p":null"#),
    ] {
        assert_eq!(documents::request_on(route, &body(extra), &limits).map(|_| ()), Ok(()));
    }
}

#[test]
fn a_refused_field_is_answered_unsupported_naming_it() {
    for (extra, expected) in [
        (r#", "top_p":0.9"#, br#"{"type":"error","error":{"code":"subscription_sharing_unsupported_capability","message":"subscription_sharing_unsupported_capability","param":"top_p"}}"#.as_slice()),
        (r#", "prompt_cache_retention":"24h""#, br#"{"type":"error","error":{"code":"subscription_sharing_unsupported_capability","message":"subscription_sharing_unsupported_capability","param":"prompt_cache_retention"}}"#),
    ] {
        let endpoint = Endpoint::openai();
        let mut peer = local_peer(endpoint.clone(), key());
        let response = send_request(&mut peer, &endpoint, b"authorization: Bearer k0\r\n", &body(extra));
        let offset = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(
            (response.starts_with(b"HTTP/1.1 400 Bad Request\r\n"), &response[offset..]),
            (true, expected),
            "{extra}"
        );
    }
}

struct ScopeRow {
    endpoint: Endpoint,
    credential: Credential,
    fields: &'static [u8],
    extra: &'static str,
    expected: Vec<api::Caching>,
}

fn scope_rows() -> [ScopeRow; 4] {
    let scope = api::Caching::Scope([0xab; 16]);
    [
        ScopeRow {
            endpoint: Endpoint::openai(),
            credential: key(),
            fields: b"authorization: Bearer k0\r\n".as_slice(),
            extra: r#", "prompt_cache_key":"abababab-abab-abab-abab-abababababab""#,
            expected: vec![scope],
        },
        ScopeRow {
            endpoint: Endpoint::openai(),
            credential: key(),
            fields: b"authorization: Bearer k0\r\nsession-id: abababab-abab-abab-abab-abababababab\r\n",
            extra: r#", "prompt_cache_key":"abababab-abab-abab-abab-abababababab""#,
            expected: vec![],
        },
        ScopeRow {
            endpoint: Endpoint::openai(),
            credential: key(),
            fields: b"authorization: Bearer k0\r\n",
            extra: r#", "prompt_cache_key":"ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB""#,
            expected: vec![],
        },
        ScopeRow {
            endpoint: Endpoint::codex(),
            credential: Credential::codex(b"b1".as_slice().into(), b"a0".as_slice().into()),
            fields: b"authorization: Bearer b1\r\nchatgpt-account-id: a0\r\n",
            extra: r#", "prompt_cache_key":"abababab-abab-abab-abab-abababababab""#,
            expected: vec![],
        },
    ]
}

#[test]
fn the_public_route_takes_its_scope_from_the_cache_key_alone() {
    for ScopeRow { endpoint, credential, fields, extra, expected } in scope_rows() {
        let mut peer = local_peer(endpoint.clone(), credential);
        send_request(&mut peer, &endpoint, fields, &body(extra));
        let got: Vec<_> = peer
            .observations()
            .iter()
            .filter_map(|record| match record {
                llm::Observation::Query { query, .. } => Some(query.caching.clone()),
                llm::Observation::Accepted { .. }
                | llm::Observation::Head { .. }
                | llm::Observation::Credential { .. }
                | llm::Observation::Answered { .. }
                | llm::Observation::Closed { .. } => None,
            })
            .collect();
        assert_eq!(got, expected, "{:?} {extra}", endpoint.provider);
    }
}

#[test]
fn without_records_serves_past_a_zero_cap() {
    for recording in [true, false] {
        let endpoint = Endpoint::openai();
        let mut peer = make_scripted_with_limits(
            endpoint.clone(),
            key(),
            (Ipv4Addr::LOCALHOST, 0).into(),
            Transport::Plaintext,
            Box::new([]),
            Limits { observations: 0, observation_bytes: 0, ..limits() },
        )
        .unwrap();
        if !recording {
            peer = peer.without_records();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            send_request(&mut peer, &endpoint, b"authorization: Bearer k0\r\n", &body(""))
        }));
        if recording {
            support::assert_panic(result, "the world's referee observation count fits its configured cap");
        } else {
            assert!(result.unwrap().starts_with(b"HTTP/1.1 200"));
            assert!(peer.observations().is_empty());
        }
    }
}

#[test]
fn responses_function_output_and_user_text_select_the_same_input_turn() {
    let config = skein_llm_world::fake::config();
    let text_turn = |text: &[u8]| api::Turn {
        lines: Box::new([api::Line::Text { text: text.into() }]),
        finish: api::Finish::Stop,
        tokens: 1,
    };
    let scripts = api::Scripts {
        cued: Box::new([api::Script {
            cue: b"input script".as_slice().into(),
            turns: Box::new([text_turn(b"wait"), text_turn(b"selected")]),
        }]),
        asked: Box::new([]),
        otherwise: Box::new([]),
    };
    let rules = api::Rules {
        cued: Box::new([Box::new([
            api::Rule { cue: b"expected".as_slice().into(), turn: 1 },
            api::Rule { cue: Box::new([]), turn: 0 },
        ])]),
        asked: Box::new([]),
    };
    let domain = domain::Domain::played_by_input(
        &config,
        23,
        scripts,
        api::Menu { arguments: Box::new([]), invalid: Box::new([]) },
        rules,
    )
    .expect("input-selected domain");
    let endpoint = Endpoint::openai();
    let mut peer = make_domain_with_limits(
        endpoint.clone(),
        key(),
        (Ipv4Addr::LOCALHOST, 0).into(),
        Transport::Plaintext,
        domain,
        limits(),
    )
    .expect("input-selected byte peer");
    let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
    let (server, client) = (sim.spawn_process(), sim.spawn_process());
    for input in [
        r#"[{"type":"function_call","call_id":"waiting","name":"wait","arguments":"{}"},{"type":"function_call_output","call_id":"waiting","output":"expected"}]"#,
        r#"[{"role":"user","content":[{"type":"input_text","text":"expected"}]}]"#,
    ] {
        let body =
            format!(r#"{{"model":"gpt","store":false,"instructions":"input script","input":{input},"stream":true}}"#);
        let response = request::request(
            &mut sim,
            server,
            client,
            &mut peer,
            &endpoint.target,
            b"authorization: Bearer k0\r\n",
            body.as_bytes(),
        );
        let response = core::str::from_utf8(&response).expect("HTTP response");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains(r#""text":"selected""#), "{response}");
        assert!(response.contains("response.completed"), "{response}");
    }
    peer.shutdown();
    for _ in 0..1000 {
        request::pass(&mut sim, server, &mut peer);
        if peer.is_empty() {
            break;
        }
    }
    assert!(peer.is_empty());
    sim.assert_quiescent(server);
    sim.assert_no_open_fds(server);
    sim.assert_no_open_fds(client);
}

#[test]
fn an_llm_peer_answers_on_its_adopted_listener() {
    let mut sim = skein_sim::Sim::new(23, skein_sim::Config::calm());
    let (server, client) = (sim.spawn_process(), sim.spawn_process());
    let (fd, address) = support::held_listener(&mut sim, server);
    let endpoint = Endpoint::openai();
    let mut peer = make_on(endpoint.clone(), key(), address, Transport::Plaintext).expect("configured peer");
    peer.adopt_listener(fd, address).expect("held listener before the first drive");
    let response = request::request(
        &mut sim,
        server,
        client,
        &mut peer,
        &endpoint.target,
        b"authorization: Bearer k0\r\n",
        &body(""),
    );
    assert_eq!(peer.address(), Some(address));
    assert!(response.starts_with(b"HTTP/1.1 200"));
    assert!(peer.observations().iter().any(|event| matches!(event, llm::Observation::Answered { result: Ok(()), .. })));
    peer.shutdown();
    for _ in 0..1000 {
        if peer.is_empty() {
            break;
        }
        request::pass(&mut sim, server, &mut peer);
    }
    assert!(peer.is_empty());
    for pid in [server, client] {
        sim.assert_quiescent(pid);
        sim.assert_no_open_fds(pid);
    }
}
