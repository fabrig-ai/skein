//! The product world's steps, clock, inputs and bounded process output.
#[path = "steps/support.rs"]
mod support;

use skein_lib::{Rng, Time};
use skein_sim::{Config, Trace};
use skein_sim_world::{Digests, PRINTED_BYTES, Printed, Stream, World};
use skein_world_tests::{Act, Script, ms};
use std::io::{ErrorKind, Write};

#[test]
fn a_world_stepped_in_slices_ends_with_the_trace_of_the_world_run_whole() {
    for seed in [1, 7, 19, 97, u64::MAX] {
        support::compare(seed);
    }
}

#[test]
fn a_clock_moved_short_of_the_next_due_instant_turns_no_process() {
    let mut world = support::exchange(7);
    loop {
        world.step();
        if !world.busy() {
            break;
        }
    }
    let due = world.next_due().expect("a script waits");
    let before = Time::from_nanos(due.as_nanos() - 1);
    let turns: Vec<_> = world.processes().iter().map(|process| process.turns).collect();
    let trace = world.parts().0.trace().to_vec();
    let steps = world.steps();
    world.advance(before);
    assert_eq!(world.now(), before);
    assert!(!world.busy());
    assert_eq!(world.steps(), steps);
    assert_eq!(world.parts().0.trace(), trace);
    assert_eq!(world.processes().iter().map(|process| process.turns).collect::<Vec<_>>(), turns);
    let _finished = support::run(world, None);
}

#[test]
fn an_input_entered_at_a_moment_replays_to_the_same_trace() {
    let mut moment = None;
    let mut traces = Vec::new();
    for pacing in [Some(Rng::new(71)), None] {
        let mut pacing = pacing;
        let mut world = World::new(7, Config::calm());
        world.digests(Digests::Traced);
        let at = world.spawn(|| Script::new(ms(1000), &[]));
        world.step();
        support::advance(&mut world, ms(10), &mut pacing);
        let here = (world.steps(), world.now());
        match moment {
            Some(moment) => assert_eq!(here, moment),
            None => moment = Some(here),
        }
        world.process(at).give(Act::Socket);
        assert!(world.busy(), "input makes its next turn ready");
        let end = support::run(world, pacing);
        assert!(end.submissions[0] > 0, "the input caused kernel work");
        traces.push(end.trace);
    }
    assert_eq!(traces[0], traces[1], "a replay names the same step and instant");
}

#[test]
fn the_steps_count_from_zero() {
    let mut world = World::<Script>::new(7, Config::calm());
    assert_eq!(world.steps(), 0);
    for steps in 1..=3 {
        assert!(!world.step().ran, "an empty world's step ran no process");
        assert_eq!(world.steps(), steps);
    }
    assert_eq!(world.finish().steps, 3);
}

#[test]
fn a_printed_line_is_taken_whole_once() {
    let mut world = World::new(7, Config::calm());
    let printer = world.printer(b"script", Stream::Output);
    world.spawn(|| Script::new(Time::ZERO, &[Act::Wait(ms(10))]).printing(printer));
    world.step();
    assert!(world.printed().is_empty(), "a partial line waits for its newline");
    world.advance(ms(10));
    world.step();
    assert_eq!(
        world.printed(),
        vec![
            Printed { process: Box::from(&b"script"[..]), stream: Stream::Output, line: Box::from(&b"abc"[..]) },
            Printed { process: Box::from(&b"script"[..]), stream: Stream::Output, line: Box::from(&b"d"[..]) },
        ]
    );
    assert!(world.printed().is_empty(), "each line is taken once");
    let _finished = world.finish();
}

#[test]
fn a_printer_past_its_bound_would_block() {
    let mut world = World::<Script>::new(7, Config::calm());
    let mut output = world.printer(b"script", Stream::Output);
    let mut error = world.printer(b"script", Stream::Error);
    let mut bytes = vec![b'a'; PRINTED_BYTES];
    bytes[PRINTED_BYTES - 1] = b'\n';
    output.write_all(&bytes).expect("exactly the backlog bound");
    assert_eq!(error.write(b"x").expect_err("the bound is shared by all streams").kind(), ErrorKind::WouldBlock);
    let lines = world.printed();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].line.len(), PRINTED_BYTES - 1);
    error.write_all(b"x\n").expect("taking lines releases the bound");
    assert_eq!(
        world.printed(),
        vec![Printed { process: Box::from(&b"script"[..]), stream: Stream::Error, line: Box::from(&b"x"[..]) }]
    );
}

#[test]
fn a_trace_off_world_keeps_no_entries_and_names_it_in_the_tail() {
    let mut world = World::new(7, Config { trace: Trace::Off, ..Config::calm() });
    world.spawn(|| Script::new(Time::ZERO, &[Act::Socket]));
    assert_eq!(world.tail(), "the trace is off\n");
    assert!(support::run(world, None).trace.is_empty());
}

#[test]
#[should_panic(expected = "time moves only while the world is idle")]
fn a_busy_world_refuses_a_clock_advance() {
    let mut world = World::new(7, Config::calm());
    world.spawn(|| Script::new(Time::ZERO, &[]));
    world.advance(ms(1));
}

#[test]
#[should_panic(expected = "time never moves past the next due instant")]
fn an_idle_world_refuses_to_advance_past_its_next_due_instant() {
    let mut world = World::new(7, Config::calm());
    world.spawn(|| Script::new(ms(10), &[]));
    world.advance(ms(11));
}

#[test]
#[should_panic(expected = "time never moves backwards")]
fn a_world_refuses_to_move_its_clock_backwards() {
    let mut world = World::<Script>::new(7, Config::calm());
    world.advance(ms(10));
    world.advance(ms(9));
}
