//! The echo's scenarios (examples.md, 7), each over a few seeds: calm, where
//! every expectation holds whole, and chaotic, where streams may break and
//! every connection must still finish. Each admission point's scenario
//! checks what its clients saw for the evidence that it was reached. Every
//! world checks memory at every iteration, under the counting allocator.
//! The sweeps over many seeds are in `fuzzy_scenarios.rs`.

use skein_echo_world::scenarios::{
    EchoWorld, backpressure, busy, clients, closes, half_close, idle, shutdown, shutdown_abort, shutdown_line,
    too_long, worst,
};
use skein_heap::Counting;
use skein_sim::Config;
use skein_world::Outcome;

use skein_echo_world::proc::Proc;

#[global_allocator]
static HEAP: Counting = Counting;

const CALM: [u64; 3] = [1, 2, 3];
const CHAOS: [u64; 3] = [4, 5, 6];

fn calm(scenario: fn(u64, Config) -> EchoWorld) -> Vec<Outcome<Proc>> {
    CALM.iter().map(|seed| scenario(*seed, Config::calm()).run()).collect()
}

fn chaos(scenario: fn(u64, Config) -> EchoWorld) {
    for seed in CHAOS {
        let _outcome = scenario(seed, Config::chaos()).run();
    }
}

/// What connection `conn` of process `at` saw, at the end of a run.
fn seen(outcome: &Outcome<Proc>, at: usize, conn: u32) -> skein_echo_client::Seen {
    outcome.procs[at].as_client().expect("a fake client").seen(conn)
}

#[test]
fn many_clients_are_served_past_both_refusals_at_the_entrance() {
    let mut busy = 0;
    let mut rejected = 0;
    for outcome in calm(clients) {
        for at in 1..=3 {
            for conn in 0..2 {
                let seen = seen(&outcome, at, conn);
                busy += seen.busy;
                // A socket the protocol layer rejects is closed by io with
                // nothing said: the client hears the end, or, when its first
                // line was already there unread, a reset. Calm, nothing else
                // ends a stream unanswered.
                rejected += seen.silent + seen.broken;
            }
        }
    }
    assert!(busy > 0, "some connection was told busy at the domain's entrance");
    assert!(rejected > 0, "some connection was rejected at the protocol layer's entrance");
    chaos(clients);
}

#[test]
fn a_line_too_long_is_refused_and_the_connection_closed() {
    for outcome in calm(too_long) {
        assert_eq!(seen(&outcome, 1, 0).answered, 2, "the lines before it were answered");
    }
    chaos(too_long);
}

#[test]
fn a_peer_past_the_sessions_is_told_busy_and_one_past_the_connections_is_rejected() {
    for outcome in calm(busy) {
        let (refused, rejected) = (seen(&outcome, 1, 2), seen(&outcome, 1, 3));
        assert_eq!(refused.busy, 1, "the first of the two late ones is told busy");
        assert_eq!(rejected.silent, 1, "the second finds the protocol's connections full: rejected, unanswered");
    }
    chaos(busy);
}

#[test]
fn idle_connections_are_closed_at_their_deadline_and_not_before() {
    calm(idle);
    chaos(idle);
}

#[test]
fn a_client_that_stops_reading_is_stopped_by_backpressure() {
    calm(backpressure);
    chaos(backpressure);
}

#[test]
fn closes_and_resets_in_every_state_settle() {
    calm(closes);
    chaos(closes);
}

#[test]
fn a_shutdown_closes_idle_connections_at_once() {
    calm(shutdown);
    chaos(shutdown);
}

/// A pinned seed: under chaos, a stream of the echo's fails while its line
/// is out with the domain (Answering, then `Failed`), which the domain then
/// answers into a connection closing. Rare, about one seed in four hundred
/// of `clients`, so kept here; found by marking the cell and sweeping.
#[test]
fn a_stream_that_fails_while_its_line_is_answered_settles() {
    let _outcome = clients(387, Config::chaos()).run();
}

#[test]
fn the_same_seed_replays_to_the_same_trace() {
    for seed in [11_u64, 12] {
        let first = closes(seed, Config::chaos()).run();
        let second = closes(seed, Config::chaos()).run();
        assert!(first.trace == second.trace, "seed {seed}: the same records, in the same order, at the same times");
        for at in 1..=2 {
            for conn in 0..3 {
                assert_eq!(seen(&first, at, conn), seen(&second, at, conn), "seed {seed}: the clients saw the same");
            }
        }
    }
    let one = clients(1, Config::chaos()).run();
    let two = clients(2, Config::chaos()).run();
    assert!(one.trace != two.trace, "another seed, another run");
}

#[test]
fn memory_is_checked_at_every_iteration_against_the_worst_cases() {
    let outcome = clients(1, Config::calm()).run();
    let heap = outcome.heap.as_ref().expect("checked");
    assert_eq!(heap.len(), 4, "the echo and three clients, each checked against its own");
    for (at, (most, bound)) in heap.iter().enumerate() {
        assert!(*most > 0 && most <= bound, "process {at} held {most} bytes at most, within {bound}");
    }
    // Dropping the outcome checks that each process frees what it held.
}

#[test]
fn a_half_close_with_lines_unanswered_has_every_whole_line_answered_then_the_end() {
    calm(half_close);
    chaos(half_close);
}

/// Every connection reaches full intake and output together after a full
/// receive, observed from byte traffic while the echo has no internal work.
#[test]
fn every_echo_connection_fills_its_buffers_together_within_its_worst_case() {
    let limits = skein_echo_world::scenarios::server();
    for seed in CALM {
        let mut filled = false;
        let outcome = worst(seed, Config::calm()).run_with_inputs(|sim, _, procs| {
            let echo = procs[0].as_echo().expect("the echo");
            if !filled
                && sim.now() < skein_echo_world::scenarios::ms(1_000)
                && !echo.work_pending(sim.now())
                && echo.domain().sessions() == limits.protocol.conns
            {
                filled |=
                    skein_echo_world::referee::full_echo_buffers(sim.trace(), &limits.io) == limits.protocol.conns;
            }
        });
        assert!(filled, "seed {seed}: every connection filled its intake and output after a full receive");
        let (most, bound) = outcome.heap.as_ref().expect("checked")[0];
        assert!(most <= bound, "seed {seed}: the echo held {most} bytes, within {bound}");
    }
    chaos(worst);
}

#[test]
fn a_shutdown_answers_a_line_out_then_closes_its_connection() {
    calm(shutdown_line);
    chaos(shutdown_line);
}

#[test]
fn a_second_signal_aborts_a_connection_whose_peer_stopped_reading() {
    for outcome in calm(shutdown_abort) {
        let settled = outcome
            .trace
            .iter()
            .filter_map(|entry| {
                if entry.pid.raw() == 0
                    && matches!(
                        entry.event,
                        skein_sim::Event::Complete { kind: skein_sim::Summary::Close { .. }, result: Ok(_), .. }
                    )
                {
                    Some(entry.at)
                } else {
                    None
                }
            })
            .max()
            .expect("the echo closed its descriptors");
        assert!(
            settled >= skein_echo_world::scenarios::ms(200),
            "a graceful close was still held until the second signal"
        );
        assert!(
            settled < skein_echo_world::scenarios::ms(250),
            "the echo settled before the client abort at 500 ms and io's close timeout"
        );
        assert!(outcome.end < skein_echo_world::scenarios::ms(1_000), "the abort settles before io's close timeout");
    }
    chaos(shutdown_abort);
}

#[test]
fn a_seed_replays_to_the_same_echo_digest_at_its_end() {
    let trace = skein_world::domain::assert_replays(11, 12, |seed| {
        let outcome = closes(seed, Config::chaos()).run();
        let digests = outcome.state_digests();
        assert!(digests.procs[0].is_some(), "the echo gives its domain's digest");
        (outcome.trace.iter().map(|entry| format!("{entry:?}")).collect(), digests)
    });
    assert!(!trace.is_empty());
}

#[test]
fn a_seed_replays_to_the_same_digests_after_every_iteration() {
    let mut first = clients(1, Config::calm());
    first.digests(skein_world::Digests::Traced);
    let first = first.run();
    let mut second = clients(1, Config::calm());
    second.digests(skein_world::Digests::Traced);
    let second = second.run();
    assert_eq!(first.trace, second.trace);
    let digests: std::collections::BTreeSet<_> = first
        .trace
        .iter()
        .filter_map(|entry| {
            if let skein_sim::Event::Digest(digest) = entry.event {
                assert_eq!(entry.pid.raw(), 0, "only the echo supplies a digest");
                Some(digest)
            } else {
                None
            }
        })
        .collect();
    assert!(digests.len() > 1, "sessions coming and going change the echo's state");
}


#[path = "../../../examples/echo/service/src/io_stage.rs"]
mod io_stage;

#[test]
fn the_service_drains_each_stage_in_order_with_the_kernel_queue_near_full() {
    use skein_io::{self as io, Event, Request, kernel};
    use skein_lib::{Env, Queue, Time, Token, Wall};
    use skein_lib::stream::{Down, Read, Up};

    // A held write leaves room for ready work, but completions and timers
    // still wait for their two kernel submissions.
    const KERNEL_ROOM: u32 = 2;
    const OWNER: Token = Token::new(99);
    const DEADLINE: skein_lib::Duration = skein_lib::Duration::from_secs(1);
    let limits = skein_echo_world::scenarios::server().io;
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut machine = io::Io::new(&limits);
    let mut submissions = Queue::with_capacity(KERNEL_ROOM);
    let mut completions = Queue::with_capacity(KERNEL_ROOM);
    let mut told = Queue::uncapped();
    let timed = machine.adopt_append(kernel::Fd::new(40), DEADLINE).expect("append slot");
    let ready = machine.adopt_write_pipe(kernel::Fd::new(41), None).expect("pipe slot");
    io::down(&mut machine, &env, Request::Stream { stream: timed,
        down: Down::Demand { read: Read::Nothing, room: 1 } }, &mut submissions);
    machine.reclaim();
    io_stage::up(&mut machine, &env, &mut completions, &mut told, &mut submissions, Time::ZERO);
    drop(told.pop());
    io::down(&mut machine, &env, Request::Stream { stream: timed,
        down: Down::Send(Box::from(b"x".as_slice())) }, &mut submissions);
    let write = submissions.pop().expect("append write");
    io::down(&mut machine, &env, Request::Usage { owner: OWNER }, &mut submissions);
    let usage = submissions.pop().expect("usage query");
    completions.push(kernel::Complete { op: usage.op, kind: usage.kind,
        result: Ok(kernel::Done::Usage(kernel::Usage::ZERO)) });
    submissions.push(write);
    io::down(&mut machine, &env, Request::Stream { stream: ready,
        down: Down::Demand { read: Read::Nothing, room: 1 } }, &mut submissions);
    machine.reclaim();
    let now = Time::ZERO.saturating_add(DEADLINE);
    io_stage::up(&mut machine, &env, &mut completions, &mut told, &mut submissions, now);
    let blocked = machine.is_due(now) && !completions.is_empty();
    let write = submissions.pop().expect("kernel takes the held write");
    let late = Env { now, ..env };
    io_stage::up(&mut machine, &late, &mut completions, &mut told, &mut submissions, now);
    let events: Vec<_> = told.iter().collect();
    assert!(blocked, "completions and timers wait at the kernel edge");
    assert_eq!(events, [&Event::Stream { owner: ready, up: Up::Room },
        &Event::Usage { owner: OWNER, usage: io::Measured::Read(kernel::Usage::ZERO) },
        &Event::Stream { owner: timed, up: Up::Failed(skein_lib::stream::Fault::Other) }]);
    assert!(!machine.is_ready() && completions.is_empty() && !machine.is_due(now));
    assert_eq!(submissions.pop().expect("deadline cancels the write").kind,
        kernel::Op::Cancel { target: write.op });
}
