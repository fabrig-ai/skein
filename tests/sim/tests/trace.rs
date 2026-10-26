//! Trace retention changes what is kept, while completions still replay.

use skein_io::kernel::{Complete, Done, Family, Op};
use skein_sim::{Config, Pid, Trace};
use skein_sim_tests::{World, local, recv_op};

const SEED: u64 = 77;
const MESSAGE: &[u8] = b"ping";

fn call<'a>(world: &mut World, reaped: &'a mut Vec<Complete>, pid: Pid, op: Op) -> &'a Complete {
    reaped.push(world.call(pid, op));
    reaped.last().expect("the completion just reaped")
}

fn exchange(trace: Trace) -> (World, [Vec<Complete>; 2]) {
    let mut world = World::new(SEED, Config { trace, ..Config::calm() });
    let (client, server) = (world.spawn(), world.spawn());
    let (mut client_completions, mut server_completions) = (Vec::new(), Vec::new());
    let listener = match &call(&mut world, &mut server_completions, server, Op::Socket { family: Family::Ipv4 }).result
    {
        Ok(Done::Fd(fd)) => *fd,
        other => panic!("a listener socket: {other:?}"),
    };
    let address =
        match &call(&mut world, &mut server_completions, server, Op::Bind { fd: listener, addr: local(0) }).result {
            Ok(Done::Bound(address)) => *address,
            other => panic!("a bound listener: {other:?}"),
        };
    assert_eq!(
        call(&mut world, &mut server_completions, server, Op::Listen { fd: listener, backlog: 1 }).result,
        Ok(Done::Nothing)
    );
    let c = match &call(&mut world, &mut client_completions, client, Op::Socket { family: Family::Ipv4 }).result {
        Ok(Done::Fd(fd)) => *fd,
        other => panic!("a client socket: {other:?}"),
    };
    assert_eq!(
        call(&mut world, &mut client_completions, client, Op::Connect { fd: c, addr: address }).result,
        Ok(Done::Nothing)
    );
    let s = match &call(&mut world, &mut server_completions, server, Op::Accept { fd: listener }).result {
        Ok(Done::Accepted { fd, .. }) => *fd,
        other => panic!("an accepted connection: {other:?}"),
    };
    let send =
        call(&mut world, &mut client_completions, client, Op::Send { fd: c, bytes: Box::from(MESSAGE), from: 0 });
    assert_eq!(send.result, Ok(Done::Count(4)));
    let recv = call(&mut world, &mut server_completions, server, recv_op(s, MESSAGE.len()));
    assert_eq!(recv.result, Ok(Done::Count(4)));
    for (pid, completions, fd) in [(client, &mut client_completions, c), (server, &mut server_completions, s)] {
        assert_eq!(call(&mut world, completions, pid, Op::Close { fd }).result, Ok(Done::Nothing));
    }
    assert_eq!(call(&mut world, &mut server_completions, server, Op::Close { fd: listener }).result, Ok(Done::Nothing));
    world.settled(client);
    world.settled(server);
    world.sim.trace_digest(client, [7; 32]);
    world.sim.trace_digest(Pid::MACHINE, [3; 32]);
    (world, [client_completions, server_completions])
}

#[test]
fn a_world_with_its_trace_off_keeps_no_entry() {
    let (world, _) = exchange(Trace::Off);
    assert!(world.sim.trace().is_empty(), "neither kernel records nor digests are retained");
    assert!(world.sim.render_trace().is_empty(), "an off trace renders nothing");
}

#[test]
fn the_trace_setting_changes_no_completion() {
    let (whole, whole_completions) = exchange(Trace::Whole);
    let (off, off_completions) = exchange(Trace::Off);
    assert_eq!(whole_completions, off_completions, "both processes reap the same completions");
    assert_eq!(whole.sim.now(), off.sim.now(), "the clock is independent of trace retention");
    assert!(!whole.sim.trace().is_empty(), "whole retains the exchange");
}

#[test]
fn a_failure_with_its_trace_off_says_so_and_names_the_seed() {
    let result = std::panic::catch_unwind(|| {
        let mut world = World::new(SEED, Config { trace: Trace::Off, ..Config::calm() });
        let pid = world.spawn();
        world.socket(pid);
        world.sim.assert_no_open_fds(pid);
    });
    let payload = result.expect_err("an open descriptor fails the invariant");
    let message = payload.downcast_ref::<String>().expect("the simulator's failure message");
    assert!(message.contains("seed 77"), "{message}");
    assert!(message.contains("trace is off"), "{message}");
}
