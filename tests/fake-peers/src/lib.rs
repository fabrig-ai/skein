use skein_io::kernel::{Done, Op, Submit};
use skein_lib::{Queue, Token};
use skein_sim::{Pid, Sim};

/// Submit one scripted client operation and drive it to completion.
pub fn call(sim: &mut Sim, pid: Pid, token: u64, kind: Op) -> Done {
    let mut submissions = Queue::with_capacity(1);
    let mut completions = Queue::with_capacity(1);
    submissions.push(Submit { op: Token::new(token), kind });
    sim.submit(pid, &mut submissions);
    for _ in 0..100 {
        sim.reap(pid, &mut completions);
        if let Some(complete) = completions.pop() {
            return complete.result.expect("scripted client's operation succeeds");
        }
        assert!(sim.advance(), "the client operation must complete");
    }
    panic!("the client operation did not settle");
}

/// The named observation cap, rather than another failure, ended this exchange.
pub fn assert_panic<T: std::fmt::Debug>(result: std::thread::Result<T>, expected: &str) {
    let panic = result.expect_err("recording at cap zero panics");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("assertion message");
    assert!(message.contains(expected), "{message}");
}

/// Open and listen before the peer's first pass, retaining the chosen address.
pub fn held_listener(sim: &mut Sim, pid: Pid) -> (skein_io::kernel::Fd, std::net::SocketAddr) {
    use skein_io::kernel::Family;
    let Done::Fd(fd) = call(sim, pid, 1, Op::Socket { family: Family::Ipv4 }) else {
        panic!("socket descriptor");
    };
    let Done::Bound(addr) = call(sim, pid, 2, Op::Bind { fd, addr: (std::net::Ipv4Addr::LOCALHOST, 0).into() }) else {
        panic!("bound address");
    };
    assert_eq!(call(sim, pid, 3, Op::Listen { fd, backlog: 1 }), Done::Nothing);
    (fd, addr)
}
