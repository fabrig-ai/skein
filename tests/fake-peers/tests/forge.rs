//! Shared forge names, loopback admission and real byte settlement on Sim.
use skein_fake_peers::{Error, Limits, forge};
use skein_lib::Time;
use skein_shell::Host;

#[path = "forge/fixtures.rs"]
mod fixtures;
#[path = "forge/git.rs"]
mod git;
#[path = "forge/github.rs"]
mod github;
use fixtures::{ONE_REPLY, SEED, World, address, config, limits, protocol_limits, request};

#[test]
fn forge_peer_refuses_unusable_configuration_before_effects() {
    for case in fixtures::refusals() {
        assert_eq!(
            forge::Peer::new(case.address, case.limits, case.config, case.protocol, case.shared, fixtures::SEED).err(),
            Some(Error::Limits),
            "{}",
            case.name
        );
    }
}

#[test]
fn forge_peer_publishes_its_bound_loopback_address() {
    let mut world = World::new();
    assert_eq!(world.peer.address(), None);
    world.bind();
    let address = skein_fake_peers::Listening::address(&world.peer);
    assert_eq!(Some(address), world.peer.address());
    assert!(address.ip().is_loopback());
    assert_ne!(address.port(), 0);
    assert!(world.peer.worst_case() > 0);
    world.settle();
}

#[test]
fn repository_urls_include_the_peers_bound_port() {
    let mut world = World::new();
    world.bind();
    let bound = world.peer.address().expect("bound listener");
    let html = format!("\"html_url\":\"http://{bound}/org/repo\"");
    for target in [b"/api/v1/repos/org/repo".as_slice(), b"/api/v1/repositories/2"] {
        let fd = world.open();
        world.send(fd, &request(target));
        let response = world.response(fd);
        assert!(skein_lib::bytes::find(&response, html.as_bytes()).is_some(), "{target:?}");
        world.close(fd);
    }
    world.settle();
}

#[test]
fn a_second_fake_shares_names_replies_and_time() {
    let mut world = World::new();
    world.bind();
    let (commit, git_replies, last_now) = world.git_push();
    assert_eq!(git_replies, ONE_REPLY);
    assert_eq!(commit, fixtures::PUSHED_COMMIT);
    assert_eq!(last_now, world.forge.borrow().now);
    assert_ne!(last_now, Time::ZERO);
    let fd = world.open();
    world.send(fd, &fixtures::request(b"/api/v1/repos/org/repo/branches/work"));
    let response = world.response(fd);
    assert!(response.starts_with(b"HTTP/1.1 200 "));
    assert!(response.ends_with(fixtures::WORK_BRANCH_BODY));
    let branch = world
        .peer
        .observations()
        .iter()
        .find_map(|record| {
            if let forge::Observation::Call {
                result: Ok(skein_fake_forge_domain::api::Answer::Commit(commit)), ..
            } = record
            {
                Some(*commit)
            } else {
                None
            }
        })
        .expect("HTTP branch call's frozen terminal");
    assert_eq!(branch, commit);
    assert_eq!(world.replies, ONE_REPLY);
    assert!(world.forge.borrow().pending.is_empty());
    world.close(fd);
    world.settle();
}

#[test]
fn shutdown_keeps_a_mid_request_client_until_its_terminal() {
    let mut world = World::new();
    let fd = world.open();
    let request = fixtures::request(b"/api/v1/repos/org/repo/branches/main");
    let split = b"GET /api/v1/repos/org/repo/branches/main HTTP/1.1\r\n".len();
    world.send(fd, &request[..split]);
    skein_fake_peers::Listening::shutdown(&mut world.peer);
    world.pass();
    assert!(!world.peer.is_empty());
    world.send(fd, &request[split..]);
    let response = world.response(fd);
    assert!(response.starts_with(b"HTTP/1.1 200 "));
    assert_eq!(world.replies, ONE_REPLY);
    assert_eq!(
        world.peer.observations().iter().filter(|record| matches!(record, forge::Observation::Call { .. })).count(),
        ONE_REPLY
    );
    world.close(fd);
    world.settle();
    assert!(world.peer.is_empty());
}

#[test]
fn an_endpoint_set_after_binding_delivers_the_next_hook() {
    for (name, host, secret) in fixtures::unusable_endpoints() {
        let mut world = World::delivery(fixtures::SEED);
        world.bind();
        world.forge.borrow_mut().hooks = host.map(|host| fixtures::endpoint(host, secret));
        world.create_issue_and_settle();
        assert_eq!(world.delivery_outcomes(), [&forge::Delivery::NoAddress], "{name}");
        assert_eq!(world.connects, fixtures::NO_POSTS, "{name}");
        assert_eq!(world.received_posts(), fixtures::NO_POSTS, "{name}");
    }
    fixtures::refused_endpoint_updates_preserve_ownership();
    let mut world = World::delivery(fixtures::SEED);
    world.bind_receiver(false);
    world.create_issue_and_settle();
    assert_eq!(world.delivery_outcomes(), [&forge::Delivery::Written]);
    assert_eq!(world.received_posts(), fixtures::ONE_POST);
    assert_eq!(world.connects, fixtures::ONE_POST);
}

#[test]
fn loopback_calls_and_deliveries_leave_frozen_observations() {
    let mut world = World::delivery(fixtures::SEED);
    world.bind_receiver(false);
    let response = world.create_issue_and_settle();
    assert!(response.starts_with(b"HTTP/1.1 201 "));
    assert_eq!(world.domain_calls(), fixtures::WRITE_AND_READBACK);
    assert_eq!(world.received_posts(), fixtures::ONE_POST);
    assert_eq!(fixtures::record_kinds(&world), ["connection", "head", "call", "delivery"]);
    for record in world.peer.observations() {
        if let forge::Observation::Head { call, .. } = record {
            for header in &call.headers {
                if header.name.eq_ignore_ascii_case(b"authorization") {
                    assert_eq!(&*header.value, b"");
                }
            }
        }
    }
    let frozen = world.frozen_trace();
    assert!(fixtures::heads_hide_token(&world));
    world.mutate_owner();
    assert_eq!(world.frozen_trace(), frozen);
}

#[test]
fn a_peer_keeping_connections_and_deliveries_freezes_no_heads_or_calls() {
    let mut every = World::delivery(fixtures::SEED);
    every.bind_receiver(false);
    let expected_response = every.create_issue_and_settle();
    assert_eq!(every.domain_calls(), fixtures::WRITE_AND_READBACK);

    let mut world = World::delivery(fixtures::SEED);
    let bound = world.peer.worst_case();
    world.peer = world.peer.keeping(forge::Kept { connections: true, deliveries: true, ..forge::Kept::NONE });
    assert_eq!(world.peer.worst_case(), bound);
    world.bind_receiver(false);
    let response = world.create_issue_and_settle();
    assert_eq!(response, expected_response);
    assert_eq!(world.forge.borrow().domain.tally(), every.forge.borrow().domain.tally());
    assert_eq!(world.received_posts(), fixtures::ONE_POST);
    assert_eq!(world.delivery_outcomes(), [&forge::Delivery::Written]);
    assert_eq!(fixtures::record_kinds(&world), ["connection", "delivery"]);
    assert_eq!(world.peer.worst_case(), bound);
}

#[test]
fn keeping_every_kind_reproduces_the_default_frozen_trace() {
    let mut every = World::delivery(fixtures::SEED);
    every.peer = every.peer.keeping(forge::Kept::EVERY);
    every.bind_receiver(false);
    every.create_issue_and_settle();

    let mut default = World::delivery(fixtures::SEED);
    default.bind_receiver(false);
    default.create_issue_and_settle();
    assert_eq!(every.frozen_trace(), default.frozen_trace());
}

#[test]
fn late_deliveries_are_written_once() {
    let mut world = World::late_delivery(fixtures::SEED);
    world.bind_receiver(false);
    world.create_issue_and_settle();
    assert_eq!(world.delivery_outcomes(), [&forge::Delivery::Written]);
    assert_eq!(world.received_posts(), fixtures::ONE_POST);
    assert!(world.first_delivery_at() >= fixtures::LATE_MIN);
    assert_eq!(world.connects, fixtures::ONE_POST);
}

#[test]
fn lost_hooks_make_no_delivery_attempt() {
    let mut world = World::lost_delivery(fixtures::SEED);
    world.bind_receiver(false);
    world.create_issue_and_settle();
    assert_eq!(world.delivery_outcomes(), Vec::<&forge::Delivery>::new());
    assert_eq!(world.received_posts(), fixtures::NO_POSTS);
    assert_eq!(world.connects, fixtures::NO_POSTS);
}

#[test]
fn unwritable_deliveries_are_dropped_once() {
    for case in fixtures::unwritable_cases() {
        let mut world = case.world();
        world.inject_hooks(case.hooks);
        world.settle_hooks();
        assert_eq!(world.delivery_outcomes(), case.outcomes, "{}", case.name);
        assert_eq!(world.connects, case.connects, "{}", case.name);
        assert_eq!(world.received_posts(), case.posts, "{}", case.name);
    }
}

#[test]
fn a_seed_replays_names_replies_observations_and_deliveries() {
    let first = fixtures::recorded_delivery(fixtures::SEED);
    let second = fixtures::recorded_delivery(fixtures::SEED);
    assert_eq!(first.peer.observations(), second.peer.observations());
    assert_eq!(first.wire_posts(), second.wire_posts());
    assert_eq!(first.frozen_trace(), second.frozen_trace());
}

#[test]
fn connection_records_fill_a_cap_that_every_kind_exceeds() {
    const CONNECTION_OPEN_AND_CLOSE_RECORDS: u32 = 2;
    for kept in [forge::Kept { connections: true, ..forge::Kept::NONE }, forge::Kept::EVERY] {
        let mut world = World::new();
        world.peer = forge::Peer::new(
            address(),
            Limits { observations: CONNECTION_OPEN_AND_CLOSE_RECORDS, ..limits() },
            config(),
            protocol_limits(),
            std::rc::Rc::clone(&world.forge),
            SEED,
        )
        .unwrap()
        .keeping(kept);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let fd = world.open();
            world.send(fd, &request(b"/api/v1/repos/org/repo"));
            let response = world.response(fd);
            world.close(fd);
            world.settle();
            response
        }));
        if kept == forge::Kept::EVERY {
            skein_fake_peers_tests::assert_panic(result, "frozen record count fits");
        } else {
            assert!(result.unwrap().starts_with(b"HTTP/1.1 200"));
            assert_eq!(world.peer.observations().len(), usize::try_from(CONNECTION_OPEN_AND_CLOSE_RECORDS).unwrap());
            assert_eq!(fixtures::record_kinds(&world), ["connection"]);
        }
    }
}

#[test]
fn without_records_serves_past_a_zero_cap() {
    for recording in [true, false] {
        let mut world = World::new();
        let mut peer = forge::Peer::new(
            address(),
            Limits { observations: 0, observation_bytes: 0, ..limits() },
            config(),
            protocol_limits(),
            std::rc::Rc::clone(&world.forge),
            SEED,
        )
        .unwrap();
        if !recording {
            peer = peer.without_records();
        }
        world.peer = peer;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let fd = world.connect();
            world.send(fd, &request(b"/api/v1/repos/org/repo"));
            let response = world.response(fd);
            world.close(fd);
            response
        }));
        if recording {
            skein_fake_peers_tests::assert_panic(result, "frozen record count fits");
        } else {
            assert!(result.unwrap().starts_with(b"HTTP/1.1 200"));
            assert!(world.peer.observations().is_empty());
            world.settle();
        }
    }
}

#[test]
fn without_records_omits_the_observation_bound() {
    let mut world = World::new();
    let before = world.peer.worst_case();
    world.peer = world.peer.without_records();
    skein_fake_peers_tests::assert_record_bound::<forge::Observation>(
        before,
        world.peer.worst_case(),
        limits().observations,
        limits().observation_bytes,
    );
    world.pass();
    skein_fake_peers_tests::assert_record_bound::<forge::Observation>(
        before,
        world.peer.worst_case(),
        limits().observations,
        limits().observation_bytes,
    );
}

#[test]
fn an_owner_seeds_the_setup_admin_and_token_before_serving() {
    let mut world = World::new();
    let store = world.peer.setup_mut();
    let admin = store.account(b"site-admin", true, b"Site Administrator");
    let token = b"owner-seeded-admin-token";
    store.token(admin, b"owner", &[b"all"], skein_lib::Secret::new(fixtures::boxed(token)));
    let fd = world.open();
    let request = format!(
        "GET /api/v1/user HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: token {}\r\nConnection: close\r\n\r\n",
        std::str::from_utf8(token).unwrap()
    );
    world.send(fd, request.as_bytes());
    let response = world.response(fd);
    assert!(response.starts_with(b"HTTP/1.1 200"));
    let expected = format!(r#"{{"id":{admin},"login":"site-admin","is_admin":true,"full_name":"Site Administrator"}}"#);
    assert!(response.ends_with(expected.as_bytes()), "{}", String::from_utf8_lossy(&response));
    world.close(fd);
    world.settle();
}

#[test]
fn a_setup_grant_lets_a_forbidden_bot_write_over_loopback() {
    let mut world = World::new();
    let store = world.peer.setup_mut();
    store.account(b"site-admin", true, b"");
    store.account(b"person", false, b"");
    let bot = store.account(b"bot", false, b"");
    let token = b"bot-token";
    store.token(bot, b"bot", &[b"all"], skein_lib::Secret::new(fixtures::boxed(token)));
    let target = "/api/v1/repos/org/repo/branches";
    let body = br#"{"new_branch_name":"bot-work","old_ref_name":"910a2dec89025cc1000000000000000100000000"}"#;
    let refused = world.exchange("POST", target, token, body);
    assert!(refused.starts_with(b"HTTP/1.1 403"), "{}", String::from_utf8_lossy(&refused));
    let granted = world.exchange(
        "PUT",
        "/api/v1/repos/org/repo/collaborators/bot",
        fixtures::TOKEN,
        br#"{"permission":"write"}"#,
    );
    assert!(granted.starts_with(b"HTTP/1.1 204"));
    let accepted = world.exchange("POST", target, token, body);
    assert!(accepted.starts_with(b"HTTP/1.1 201"), "{}", String::from_utf8_lossy(&accepted));
    assert_eq!(
        world.forge.borrow().domain.permission(fixtures::REPOSITORY, bot),
        Ok(skein_fake_forge_domain::api::Permission::Write)
    );
    world.settle();
}

#[test]
fn two_setup_hooks_each_receive_the_change_with_their_own_signature() {
    let mut world = World::delivery(SEED);
    let addresses = world.bind_setup_receivers();
    world.peer.setup_mut().account(b"site-admin", true, b"");
    let secrets = ["first-listener-secret", "second-listener-secret"];
    for (index, address) in addresses.iter().enumerate() {
        let body = format!(
            r#"{{"type":"forgejo","config":{{"url":"http://{address}/receiver/{index}?demo=1","content_type":"json","secret":"{}"}},"events":["issues"],"active":true}}"#,
            secrets[index]
        );
        let reply = world.exchange("POST", "/api/v1/repos/org/repo/hooks", fixtures::TOKEN, body.as_bytes());
        assert!(reply.starts_with(b"HTTP/1.1 201"), "{}", String::from_utf8_lossy(&reply));
    }
    let reply = world.exchange("POST", "/api/v1/repos/org/repo/issues", fixtures::TOKEN, fixtures::ISSUE_BODY);
    assert!(reply.starts_with(b"HTTP/1.1 201"));
    world.finish_setup_deliveries();
    assert_eq!(world.connects, 2);
    assert_eq!(world.delivery_outcomes(), [&forge::Delivery::Written, &forge::Delivery::Written]);
    assert_eq!(world.wire_posts().len(), 2);
    for (index, wire) in world.wire_posts().iter().enumerate() {
        let split = wire.windows(4).position(|part| part == b"\r\n\r\n").unwrap() + 4;
        let (head, body) = wire.split_at(split);
        assert!(head.starts_with(format!("POST /receiver/{index}?demo=1 HTTP/1.1\r\n").as_bytes()));
        assert!(
            head.windows(format!("Host: {}\r\n", addresses[index]).len())
                .any(|part| part == format!("Host: {}\r\n", addresses[index]).as_bytes())
        );
        let signature = head
            .split(|byte| *byte == b'\n')
            .find_map(|line| line.strip_prefix(b"X-Forgejo-Signature: "))
            .unwrap()
            .strip_suffix(b"\r")
            .unwrap();
        let mut signed = [0; 32];
        for (byte, digits) in signed.iter_mut().zip(signature.chunks_exact(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(digits).unwrap(), 16).unwrap();
        }
        let mut hmac = skein_lib::sha256::Hmac::new(secrets[index].as_bytes());
        hmac.update(body);
        assert_eq!(signed, hmac.finish());
    }
    assert_eq!(world.forge.borrow().domain.tally().hooks, 1);
}

#[test]
fn a_forge_peer_answers_on_its_adopted_listener() {
    let mut world = World::new();
    let address = world.adopt_listener();
    let fd = world.open();
    assert_eq!(world.peer.address(), Some(address));
    world.send(fd, &request(b"/api/v1/repos/org/repo"));
    let response = world.response(fd);
    assert!(response.starts_with(b"HTTP/1.1 200"), "{}", String::from_utf8_lossy(&response));
    assert!(skein_lib::bytes::find(&response, b"org/repo").is_some());
    world.close(fd);
    world.settle();
}

#[test]
fn a_tls_peer_serves_forgejo_over_the_fixed_test_chain() {
    use rustls::{
        ClientConfig, ClientConnection, RootCertStore,
        pki_types::{CertificateDer, ServerName},
    };
    use skein_lib::Duration;
    use skein_shell::{Clock, Kernel};
    use std::{
        io::{Read, Write},
        net::TcpStream,
        sync::Arc,
    };

    let mut peer =
        forge::Peer::new(address(), limits(), config(), protocol_limits(), fixtures::shared(), SEED).unwrap();
    peer.tls().unwrap();
    let kernel = Kernel::open(skein_shell::Config { operations: peer.operations() }).expect("kernel ring");
    let mut world = git::Loop { peer, kernel, clock: Clock::new() };
    let deadline = world.clock.now().now.saturating_add(Duration::from_secs(6));
    while world.peer.address().is_none() {
        world.pass();
        assert!(world.clock.now().now < deadline, "TLS listener binds");
    }
    let bound = world.peer.address().unwrap();
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(skein_fake_peers::transport::ROOT)).unwrap();
    let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut tls = ClientConnection::new(Arc::new(config), ServerName::try_from("127.0.0.1").unwrap()).unwrap();
    let mut stream = TcpStream::connect(bound).unwrap();
    stream.set_nonblocking(true).unwrap();
    tls.writer().write_all(&request(b"/api/v1/repos/org/repo")).unwrap();
    let mut response = Vec::new();
    loop {
        world.pass();
        match tls.complete_io(&mut stream) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("TLS exchange: {error}"),
        }
        let mut buffer = [0; 4096];
        match tls.reader().read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => response.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            // HTTP's length delimits this response even when the peer closes the socket.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => panic!("TLS response: {error}"),
        }
        assert!(world.clock.now().now < deadline, "TLS exchange settles");
    }
    assert!(response.starts_with(b"HTTP/1.1 200 "), "{}", String::from_utf8_lossy(&response));
    let split = skein_lib::bytes::find(&response, b"\r\n\r\n").expect("HTTP head");
    let head = std::str::from_utf8(&response[..split]).unwrap();
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
        })
        .expect("HTTP response length");
    assert_eq!(response.len() - split - 4, length, "complete HTTP body over TLS");
    let html = format!("\"html_url\":\"https://{bound}/org/repo\"");
    assert!(skein_lib::bytes::find(&response, html.as_bytes()).is_some(), "{}", String::from_utf8_lossy(&response));
    drop(stream);
    world.peer.shutdown();
    while !world.peer.is_empty() {
        world.pass();
        assert!(world.clock.now().now < deadline, "TLS peer settles");
    }
}

#[test]
fn tls_refuses_a_github_peer_and_limits_below_one_record() {
    // TLS needs room for one 16 KiB plaintext record (transport::validate).
    const BELOW_RECORD: u32 = 16_383;
    let github = forge::Peer::github(
        address(),
        address(),
        Limits { io: skein_io::Limits { sockets: 6, ..limits().io }, ..limits() },
        github::config(),
        github::limits(),
        fixtures::shared(),
        SEED,
    )
    .unwrap();
    let forgejo = forge::Peer::new(
        address(),
        Limits { plaintext: BELOW_RECORD, ..limits() },
        config(),
        protocol_limits(),
        fixtures::shared(),
        SEED,
    )
    .unwrap();
    for mut peer in [github, forgejo] {
        let bound = peer.worst_case();
        assert_eq!(peer.tls(), Err(Error::Limits));
        assert_eq!(peer.worst_case(), bound);
        assert_eq!(peer.address(), None);
        assert!(peer.observations().is_empty());
    }
}

#[test]
fn a_tls_peer_s_worst_case_counts_its_tls_buffers() {
    let mut world = World::new();
    let plaintext = world.peer.worst_case();
    world.peer.tls().unwrap();
    assert!(world.peer.worst_case() > plaintext);
    world.pass();
    assert!(world.peer.worst_case() > plaintext);
    world.settle();
}

#[test]
fn tls_admission_reserves_the_longest_https_origin() {
    const HTTPS_ORIGIN: &[u8] = b"https://127.0.0.1:65535";
    let cap = u32::try_from(HTTPS_ORIGIN.len()).unwrap();
    for (target_bytes, expected) in [(cap, Ok(())), (cap - 1, Err(Error::Limits))] {
        let mut protocol = protocol_limits();
        protocol.documents.target_bytes = target_bytes;
        let mut peer = forge::Peer::new(address(), limits(), config(), protocol, fixtures::shared(), SEED).unwrap();
        let before = peer.worst_case();
        assert_eq!(peer.tls(), expected);
        if expected.is_err() {
            assert_eq!(peer.worst_case(), before);
        }
    }
}
