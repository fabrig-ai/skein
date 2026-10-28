//! Connection pool scenarios stay under measured heap ceilings.

use core::net::Ipv4Addr;

use skein_heap::{Counting, Meter};
use skein_io::kernel::Addr;
use skein_lib::{Duration, Env, List, Queue, Time, Token};
use skein_llm_connection::{Component, Deadlines, Endpoint, Event, Limits, MAX_OUT, Request};
use skein_tls_world::pki;

#[global_allocator]
static HEAP: Counting = Counting;

fn full_pool_limits() -> Limits {
    Limits {
        endpoints: 1,
        connections: 2,
        calls: 2,
        per_endpoint: 2,
        memory: 16 * 1024 * 1024,
        idle_keep: Duration::from_secs(10),
        io: skein_io::Limits {
            sockets: 2,
            refusals: 1,
            intake: 19_000,
            receive: 1024,
            output: 19_000,
            sends: 2,
            accepts: 1,
            backlog: 1,
            close_timeout: Duration::from_secs(1),
            names: 0,
            retry: Duration::from_millis(10),
        },
        // HTTP may demand one whole head line; TLS must admit that derived read.
        tls: skein_tls::client::Limits {
            read: skein_llm::client::largest_read(&skein_llm_world::limits()),
            send: 4096,
            records: skein_tls::client::MAX_RECORD,
        },
    }
}

// Bounds two admitted TLS calls, a refused third call and the pool tables.
const POOL_PEAK_CEILING: u64 = 1_300_000;

#[test]
fn full_pool_stays_under_its_peak_ceiling() {
    let mut limits = full_pool_limits();
    let meter = Meter::new();
    meter.start();
    let mut endpoints = List::with_capacity(1);
    endpoints
        .push(Endpoint {
            address: Addr::from((Ipv4Addr::LOCALHOST, 443)),
            transport: skein_llm_connection::Transport::Tls {
                server_name: skein_tls::Name::new("skein.test").expect("name"),
                trust: pki::client(&[]),
            },
            llm: skein_llm::Endpoint::codex(),
            limits: skein_llm_world::limits(),
            credential: skein_llm::client::CredentialLimits { value: 2048, account_id: 128 },
        })
        .expect("one endpoint");
    let first = skein_llm_world::call(7);
    let destination = endpoints.get(0).unwrap();
    let mut client = destination.limits;
    client.http.request = skein_llm::client::request_head(&destination.llm, &destination.credential, &client).unwrap();
    let measured = skein_llm::client::measure(
        &first.prompt,
        Some(skein_llm_world::credential_of(&first)),
        &destination.llm,
        &client,
    )
    .unwrap();
    limits.memory = 2 * skein_llm::client::reservation(measured, &client).unwrap();
    let second = skein_llm_world::call(8);
    let third = skein_llm_world::call(9);
    let mut up = Queue::with_capacity(MAX_OUT.above);
    let mut io = Queue::with_capacity(MAX_OUT.below);
    let env = Env { now: Time::ZERO, wall: pki::VALID, limits };
    let mut component = Component::new(endpoints, &limits).expect("component");
    component.down(
        &env,
        Request::Start {
            drop_reasoning: false,
            call: Token::new(7),
            endpoint: 0,
            prompt: first.prompt,
            credential: first.credential,
            deadlines: Deadlines::none(),
        },
        &mut up,
        &mut io,
    );
    component.down(
        &env,
        Request::Start {
            drop_reasoning: false,
            call: Token::new(8),
            endpoint: 0,
            prompt: second.prompt,
            credential: second.credential,
            deadlines: Deadlines::none(),
        },
        &mut up,
        &mut io,
    );
    assert_eq!(io.len(), 2, "both physical slots are occupied");
    assert_eq!(component.reserved(), limits.memory, "both calls fill the memory pool exactly");
    component.down(
        &env,
        Request::Start {
            drop_reasoning: false,
            call: Token::new(9),
            endpoint: 0,
            prompt: third.prompt,
            credential: third.credential,
            deadlines: Deadlines::none(),
        },
        &mut up,
        &mut io,
    );
    match up.pop() {
        Some(Event::Refused { call, why }) => {
            assert_eq!(call, Token::new(9));
            assert_eq!(why, skein_llm_connection::Refusal::Calls { bound: 2 });
        }
        other => panic!("expected pool refusal, got {other:?}"),
    }
    let measured = meter.end();
    assert!(measured.peak() <= POOL_PEAK_CEILING, "full pool peak {} exceeds {POOL_PEAK_CEILING}", measured.peak());
    assert!(meter.held() <= POOL_PEAK_CEILING, "full pool retained {} exceeds {POOL_PEAK_CEILING}", meter.held());
    drop(component);
    drop(up);
    drop(io);
    drop(first.endpoint);
    drop(second.endpoint);
    drop(third.endpoint);
    assert_eq!(meter.held(), 0, "all pool allocations released");
}

// Bound the largest separate process peaks across plaintext and TLS at seed 37.
const SIMULATED_CLIENT_PEAK_CEILING: u64 = 3_200_000;
const SIMULATED_PEER_PEAK_CEILING: u64 = 1_600_000;

#[test]
fn full_fake_process_and_connection_pool_fit_and_free_their_separate_heaps() {
    for transport in [skein_fake_peers::Transport::Plaintext, skein_fake_peers::Transport::Tls] {
        let first =
            skein_llm_connection_world::simulated::run(37, false, transport, true, skein_world::Memory::Checked);
        let heap = first.heap.as_ref().unwrap();
        assert_eq!(heap.len(), 2);
        let [(client, _), (peer, _)] = heap.as_slice() else { panic!("two separate process heaps") };
        assert!(*client > 0 && *client <= SIMULATED_CLIENT_PEAK_CEILING);
        assert!(*peer > 0 && *peer <= SIMULATED_PEER_PEAK_CEILING);
        if transport == skein_fake_peers::Transport::Plaintext {
            let second =
                skein_llm_connection_world::simulated::run(37, false, transport, true, skein_world::Memory::Checked);
            skein_llm_connection_world::simulated::assert_replay(&first, &second);
            assert_eq!(first.heap, second.heap);
        }
    }
}

// Bound separate process peaks for the 1500-byte batched upload at seed 23.
const BATCHED_CLIENT_PEAK_CEILING: u64 = 1_700_000;
const BATCHED_PEER_PEAK_CEILING: u64 = 830_000;

#[test]
fn bounded_request_batches_preserve_exact_upload_latency_replay_and_separate_heaps() {
    use skein_fake_peers::Transport;
    use skein_llm_connection_world::simulated::{assert_replay, run_with_input};
    use skein_world::Memory;

    let first = run_with_input(23, false, Transport::Plaintext, false, Memory::Checked, 1500);
    let second = run_with_input(23, false, Transport::Plaintext, false, Memory::Checked, 1500);
    assert_replay(&first, &second);
    assert_eq!(first.heap, second.heap);
    assert!(first.iterations < 256, "bounded upload settled in {} iterations", first.iterations);
    assert_eq!(first.procs.len(), 2, "client and fake remain separately owned processes");
    let heap = first.heap.as_ref().unwrap();
    let [(client, _), (peer, _)] = heap.as_slice() else { panic!("two separate process heaps") };
    assert!(*client > 0 && *client <= BATCHED_CLIENT_PEAK_CEILING);
    assert!(*peer > 0 && *peer <= BATCHED_PEER_PEAK_CEILING);
}

// Bounds the largest peak across all seven owner-close phases at seed 53.
const OWNER_CLOSE_PEAK_CEILING: u64 = 2_600_000;

#[test]
fn an_owner_close_in_each_phase_stays_under_the_peak_ceiling() {
    use skein_llm_connection_world::world::{Point, World};
    for point in
        [Point::Waiting, Point::Connecting, Point::Head, Point::Streaming, Point::Draining, Point::Idle, Point::Closing]
    {
        let calls = if point == Point::Waiting { 2 } else { 1 };
        let meter = Meter::new();
        meter.start();
        let mut world = World::configured(53, calls, 1, 1);
        if point == Point::Closing {
            world.until(Point::Idle);
            world.delay_close = true;
            world.request(Request::Close);
            world.until(Point::Closing);
        } else {
            world.until(point);
            world.request(Request::Close);
        }
        world.finish();
        let sample = meter.end();
        assert!(
            sample.peak() <= OWNER_CLOSE_PEAK_CEILING,
            "{point:?} peak {} exceeds {OWNER_CLOSE_PEAK_CEILING}",
            sample.peak()
        );
        drop(world);
        assert_eq!(meter.held(), 0);
    }
}

// Bounds eight declared starts, including connection and memory waits, at seed 59.
const WAITING_STARTS_PEAK_CEILING: u64 = 2_900_000;

#[test]
fn every_declared_waiting_start_stays_under_the_peak_ceiling() {
    use skein_llm_connection_world::world::{Point, World};
    let meter = Meter::new();
    meter.start();
    let mut input = skein_llm_world::call(7);
    input.endpoint = skein_llm::Endpoint::codex();
    input.prompt.instructions = b"close-world".as_slice().into();
    let mut bounds = skein_llm_world::limits();
    bounds.http.request = skein_llm::client::request_head(
        &input.endpoint,
        &skein_llm::client::CredentialLimits { value: 2048, account_id: 128 },
        &bounds,
    )
    .unwrap();
    let measured = skein_llm::client::measure(
        &input.prompt,
        Some(skein_llm_world::credential_of(&input)),
        &input.endpoint,
        &bounds,
    )
    .unwrap();
    let memory = 2 * skein_llm::client::reservation(measured, &bounds).unwrap();
    drop(input);
    let mut world = World::empty_with_memory(59, 8, 1, 1, memory);
    for token in 7..15 {
        world.start(token);
    }
    assert_eq!(world.component.reserved(), memory);
    world.until(Point::Head);
    assert_eq!(world.connections(), 1, "one running call, one connection wait, six memory waits");
    world.request(Request::Close);
    world.finish();
    assert_eq!(world.judge.completed, 8);
    assert_eq!(world.component.reserved(), 0);
    let sample = meter.end();
    assert!(
        sample.peak() <= WAITING_STARTS_PEAK_CEILING,
        "waiting peak {} exceeds {WAITING_STARTS_PEAK_CEILING}",
        sample.peak()
    );
    drop(world);
    assert_eq!(meter.held(), 0);
}

// Bounds calls at two endpoints with different limits at seed 89.
const TWO_ENDPOINTS_PEAK_CEILING: u64 = 4_200_000;

#[test]
fn two_endpoints_with_different_limits_stay_under_the_peak_ceiling() {
    use skein_llm_connection_world::world::World;
    let meter = Meter::new();
    meter.start();
    let mut world = World::configured(89, 3, 1, 2);
    world.request(Request::Cancel { call: Token::new(9) });
    world.start_at(10, 1, Deadlines::none());
    world.request(Request::Close);
    world.finish();
    assert_eq!(world.judge.completed, 3);
    let sample = meter.end();
    assert!(
        sample.peak() <= TWO_ENDPOINTS_PEAK_CEILING,
        "two endpoint peak {} exceeds {TWO_ENDPOINTS_PEAK_CEILING}",
        sample.peak()
    );
    drop(world);
    assert_eq!(meter.held(), 0);
}

#[test]
fn connection_bound_counts_socket_buffers_and_pool_without_the_owners_io() {
    let mut limits = full_pool_limits();
    let mut endpoints = List::with_capacity(1);
    endpoints
        .push(Endpoint {
            address: Addr::from((Ipv4Addr::LOCALHOST, 443)),
            transport: skein_llm_connection::Transport::Tls {
                server_name: skein_tls::Name::new("skein.test").expect("name"),
                trust: pki::client(&[]),
            },
            llm: skein_llm::Endpoint::codex(),
            limits: skein_llm_world::limits(),
            credential: skein_llm::client::CredentialLimits { value: 2048, account_id: 128 },
        })
        .unwrap();
    let small_bound = skein_llm_connection::worst_case(&limits, &endpoints).unwrap();
    // Owner entity/refusal tables belong to its io, not this connection pool.
    limits.io.sockets += 1000;
    limits.io.refusals += 1000;
    assert_eq!(skein_llm_connection::worst_case(&limits, &endpoints).unwrap(), small_bound);
    // Every physical pool slot prices its own intake, receive and output.
    limits.io.intake += 1024;
    limits.io.receive += 512;
    limits.io.output += 2048;
    let delta = u64::from(limits.connections) * (1024 + 512 + 2048);
    assert_eq!(skein_llm_connection::worst_case(&limits, &endpoints).unwrap(), small_bound + delta);
    limits.memory += 1024;
    assert_eq!(skein_llm_connection::worst_case(&limits, &endpoints).unwrap(), small_bound + delta + 1024);
    limits.io.sends += 1;
    let queue_delta = u64::from(limits.connections) * u64::try_from(size_of::<Box<[u8]>>()).unwrap();
    assert_eq!(
        skein_llm_connection::worst_case(&limits, &endpoints).unwrap(),
        small_bound + delta + 1024 + queue_delta
    );
    endpoints.get_mut(0).unwrap().transport = skein_llm_connection::Transport::Plaintext;
    let tls = skein_tls::client::worst_case(&limits.tls).unwrap() * u64::from(limits.connections);
    assert_eq!(
        skein_llm_connection::worst_case(&limits, &endpoints).unwrap(),
        small_bound + delta + 1024 + queue_delta - tls
    );
}
