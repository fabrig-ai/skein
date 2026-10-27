//! The hosted scripts use real pipes and the same factories as simulated worlds.

use skein_io::kernel::{Exit, Signal, Spawn};
use skein_lib::Duration;
use skein_shell::{Clock, open_root};
use skein_world::{HostedProgram, Inherited, real};
use skein_world_tests::hosted::{Act, Judge, Script};

fn exchange(_spawn: &Spawn, inherited: &Inherited) -> Script {
    Script::child(inherited, &[Act::Read(0), Act::Write(1, b"reply")])
}

fn wait_again(_spawn: &Spawn, inherited: &Inherited) -> Script {
    Script::child(inherited, &[Act::Read(0), Act::Write(1, b"reply"), Act::Read(0)])
}

fn run(make: fn(&Spawn, &Inherited) -> Script, acts: &[Act]) -> real::Outcome<Script> {
    let mut world = real::World::new(Judge);
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make, instances: 1, operations: 8 });
    let root = open_root(std::path::Path::new(".")).expect("startup root");
    world.spawn_with_fds(vec![root], || Script::parent(root, acts));
    world.run(&Clock::new(), Duration::from_secs(2))
}

#[test]
fn hosted_parent_and_child_exchange_over_real_pipes() {
    let outcome = run(exchange, &[Act::Spawn, Act::Write(0, b"hello"), Act::Read(1), Act::Read(1), Act::Wait]);
    assert_eq!(outcome.procs[0].received, b"reply");
    assert_eq!(outcome.procs[1].received, b"hello");
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
    assert!(outcome.killed.is_empty());
}

#[test]
fn kill_drops_child_and_settles_pending_read_before_parent_wait() {
    let outcome = run(
        wait_again,
        &[Act::Spawn, Act::Write(0, b"hello"), Act::Read(1), Act::Signal(Signal::Kill), Act::Read(1), Act::Wait],
    );
    assert_eq!(outcome.procs.len(), 1);
    assert_eq!(outcome.procs[0].received, b"reply");
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Signal(9)));
    assert_eq!(outcome.killed[0].exit, Exit::Signal(9));
}

fn terminated(_spawn: &Spawn, inherited: &Inherited) -> Script {
    Script::child(inherited, &[Act::ReadSignal, Act::Write(1, b"terminated")])
}

#[test]
fn parent_termination_arrives_as_a_signal_record_on_the_child_pipe() {
    let outcome = run(terminated, &[Act::Spawn, Act::Signal(Signal::Terminate), Act::Read(1), Act::Read(1), Act::Wait]);
    assert_eq!(outcome.procs[0].received, b"terminated");
    assert_eq!(outcome.procs[1].signals, [skein_io::kernel::ServiceSignal::Terminate]);
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
}

#[test]
fn parent_interrupt_arrives_on_the_hosted_child_signal_pipe() {
    use skein_world_tests::hosted::{check_interrupt, interrupted_child};
    let outcome = run(
        interrupted_child,
        &[
            Act::Spawn,
            Act::Signal(Signal::Interrupt),
            Act::Read(1),
            Act::Write(0, b"hello"),
            Act::Read(1),
            Act::Read(1),
            Act::Wait,
        ],
    );
    check_interrupt(&outcome.procs);
}

struct PlainJudge;

impl skein_world::Referee<skein_world::end_to_end::Binary> for PlainJudge {
    fn act(&mut self, _now: skein_lib::Time, _procs: &mut [skein_world::end_to_end::Binary]) {}
    fn observe(&mut self, _now: skein_lib::Time, _procs: &[skein_world::end_to_end::Binary]) {}
    fn next_deadline(&self) -> Option<skein_lib::Time> {
        None
    }
    fn overdue(&self, _now: skein_lib::Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        true
    }
}

#[test]
fn an_unhandled_interrupt_ends_a_real_child_with_signal_two() {
    use skein_world::end_to_end::{Binary, Command, Mode};
    for group in [false, true] {
        let command = Command {
            program: skein_scratch::paths::binary("world_process_fixture"),
            arguments: vec!["never".into()],
            environment: vec![],
            directory: "/tmp".into(),
        };
        let mut binary = Binary::start(command, Mode::Pipes, 1024).expect("the plain child starts");
        if group {
            binary.signal_group(Signal::Interrupt);
        } else {
            binary.signal(Signal::Interrupt);
        }
        let mut world = real::World::new(PlainJudge);
        world.spawn_with_fds(binary.descriptors(), || binary);
        let outcome = world.run(&Clock::new(), Duration::from_secs(2));
        assert_eq!(outcome.procs[0].exit_status(), Some(Exit::Signal(2)));
    }
}

#[derive(Debug)]
struct SignalJudge {
    control: real::Controls,
    child: bool,
    sent: bool,
}

impl skein_world::Referee<Script> for SignalJudge {
    fn act(&mut self, _now: skein_lib::Time, procs: &mut [Script]) {
        if self.sent {
            return;
        }
        if self.child {
            // This is the parent's public spawn result, not its service state.
            if !procs[0].results.iter().any(|result| matches!(result, Ok(skein_io::kernel::Done::Spawned { .. }))) {
                return;
            }
            self.control.signal(1, skein_io::kernel::ServiceSignal::Interrupt);
        } else {
            self.control.signal(0, skein_io::kernel::ServiceSignal::Terminate);
            self.control.signal(1, skein_io::kernel::ServiceSignal::Interrupt);
        }
        self.sent = true;
    }
    fn observe(&mut self, _now: skein_lib::Time, procs: &[Script]) {
        if self.child
            && !self.sent
            && procs[0].results.iter().any(|result| matches!(result, Ok(skein_io::kernel::Done::Spawned { .. })))
        {
            self.control.signal(1, skein_io::kernel::ServiceSignal::Interrupt);
            self.sent = true;
        }
    }
    fn next_deadline(&self) -> Option<skein_lib::Time> {
        None
    }
    fn overdue(&self, _now: skein_lib::Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        self.sent
    }
}

#[test]
fn referee_real_signal_reaches_only_the_signalfd_reader_and_pipe_signal_reaches_its_host() {
    let mut world = real::World::new_controlled(|control| SignalJudge { control, child: false, sent: false });
    world.spawn_signalfd(|signal| {
        Script::child(
            &Inherited { dir: None, roots: vec![], appends: vec![], pipes: vec![], signal },
            &[Act::ReadSignal],
        )
    });
    world.spawn_signals(|signal| {
        Script::child(
            &Inherited { dir: None, roots: vec![], appends: vec![], pipes: vec![], signal },
            &[Act::ReadSignal],
        )
    });
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(outcome.procs[0].signals, [skein_io::kernel::ServiceSignal::Terminate]);
    assert_eq!(outcome.procs[1].signals, [skein_io::kernel::ServiceSignal::Interrupt]);
}

#[test]
fn referee_signal_reaches_a_hosted_child_through_its_pipe() {
    let mut world = real::World::new_controlled(|control| SignalJudge { control, child: true, sent: false });
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make: terminated, instances: 1, operations: 8 });
    let root = open_root(std::path::Path::new(".")).expect("startup root");
    world.spawn_with_fds(vec![root], || Script::parent(root, &[Act::Spawn, Act::Read(1), Act::Read(1), Act::Wait]));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(outcome.procs[0].received, b"terminated");
    assert_eq!(outcome.procs[1].signals, [skein_io::kernel::ServiceSignal::Interrupt]);
}

#[test]
#[should_panic(expected = "process 0 woke for policy deadline")]
fn a_keep_time_after_the_last_word_fails_the_world_naming_the_process() {
    let clock = Clock::new();
    let mut world = real::World::new(Judge);
    // The pause and the last word use the same iteration snapshot.
    world.spawn(|| Script::new(&[Act::PauseFor(Duration::from_millis(5))]));
    let _outcome = world.run(&clock, Duration::from_secs(2));
}

#[test]
fn a_never_settling_world_names_each_hosts_pending_work_and_deadlines() {
    use skein_io::kernel::{Complete, Submit};
    use skein_lib::{Queue, Time, Wall};
    use skein_shell::Host;

    #[derive(Debug)]
    struct Pending {
        work: bool,
        deadline: Option<Time>,
        completions: Queue<Complete>,
        submissions: Queue<Submit>,
    }
    impl Host for Pending {
        fn iterate(&mut self, _now: Time, _wall: Wall) {}
        fn completions(&mut self) -> &mut Queue<Complete> {
            &mut self.completions
        }
        fn submissions(&mut self) -> &mut Queue<Submit> {
            &mut self.submissions
        }
        fn work_pending(&self, _now: Time) -> bool {
            self.work
        }
        fn next_deadline(&self) -> Option<Time> {
            self.deadline
        }
        fn next_policy_deadline(&self) -> Option<Time> {
            self.deadline
        }
        fn is_empty(&self) -> bool {
            !self.work && self.deadline.is_none()
        }
        fn operations(&self) -> u32 {
            1
        }
        fn worst_case(&self) -> u64 {
            0
        }
    }
    struct NeverPassed;
    impl skein_world::Referee<Pending> for NeverPassed {
        fn act(&mut self, _now: Time, _procs: &mut [Pending]) {}
        fn observe(&mut self, _now: Time, _procs: &[Pending]) {}
        fn next_deadline(&self) -> Option<Time> {
            Some(Time::ZERO)
        }
        fn overdue(&self, _now: Time) -> Option<String> {
            None
        }
        fn passed(&self) -> bool {
            false
        }
    }
    let failure = std::panic::catch_unwind(|| {
        let mut world = real::World::new(NeverPassed);
        for (work, deadline) in [(false, None), (true, None), (false, Some(Time::ZERO))] {
            world.spawn(|| Pending {
                work,
                deadline,
                completions: Queue::with_capacity(1),
                submissions: Queue::with_capacity(1),
            });
        }
        world.run(&Clock::new(), Duration::from_millis(20))
    })
    .expect_err("the pending host cannot settle");
    let text = failure.downcast_ref::<String>().expect("patience diagnostic");
    let iterations: u32 = text
        .split("iterations (including failing iteration): ")
        .nth(1)
        .expect("iteration count")
        .split(';')
        .next()
        .expect("count before CPU time")
        .parse()
        .expect("unsigned count");
    let recent = iterations.saturating_sub(1).min(8);
    assert!(text.contains("referee: passed=false; last_word=None; next_deadline=Some(0 ns) (due"), "{text}");
    let quiet = text.find("host 0: work_pending=false; next_deadline=None; next_policy_deadline=None; is_empty=true; ring_count=0; work_pending_last_8=0/8").expect("quiet host");
    let pending = text.find(&format!("host 1: work_pending=true; next_deadline=None; next_policy_deadline=None; is_empty=false; ring_count=0; work_pending_last_8={recent}/8")).expect("host continuing each completed pass");
    let timer = text.find("host 2: work_pending=false; next_deadline=Some(0 ns) (due").expect("due timer host");
    assert!(quiet < pending && pending < timer, "living hosts remain in admission order: {text}");
    assert!(text[timer..].contains("next_policy_deadline=Some(0 ns) (due"));
    assert!(text[timer..].contains("is_empty=false; ring_count=0; work_pending_last_8=0/8"));
}

#[test]
fn a_zero_patience_deadline_reports_the_failing_iteration() {
    let failure = std::panic::catch_unwind(|| {
        let mut world = real::World::new(Judge);
        world.spawn(|| Script::new(&[]));
        world.run(&Clock::new(), Duration::ZERO)
    })
    .expect_err("zero patience fails before the first script iteration");
    let text = failure.downcast_ref::<String>().expect("the deadline fails with its diagnostic");
    let cpu = text
        .lines().next().expect("deadline header")
        .strip_prefix(
            "the real loop settles within 0 ms; iterations (including failing iteration): 1; thread CPU elapsed (schedstat): ",
        )
        .expect("the deadline names its budget, failing iteration and thread CPU source");
    if cpu != "UNAVAILABLE (ns)" {
        let nanos = cpu.strip_suffix(" ns").expect("available thread CPU has nanosecond units");
        assert!(nanos.parse::<u64>().is_ok(), "available thread CPU is an unsigned elapsed count: {cpu}");
    }
}

#[test]
fn a_world_that_ends_by_itself_passes() {
    let mut world = real::World::new(Judge);
    world.spawn(|| Script::new(&[]));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert!(skein_world::Host::is_empty(&outcome.procs[0]));
}

#[test]
fn a_scenario_that_names_a_later_last_word_may_wait_for_it() {
    let clock = Clock::new();
    let deadline = clock.now().now.saturating_add(Duration::from_millis(5));
    let mut world = real::World::new(skein_world_tests::Later(skein_world::LastWord::After(deadline)));
    world.spawn(|| Script::new(&[Act::Pause(deadline)]));
    let outcome = world.run(&clock, Duration::from_secs(2));
    assert!(outcome.end >= deadline);
}

#[test]
#[should_panic(expected = "process 0 woke for policy deadline")]
fn a_named_later_last_word_still_rejects_policy_deadlines_after_it() {
    let clock = Clock::new();
    let start = clock.now().now;
    let after = start.saturating_add(Duration::from_millis(5));
    let deadline = start.saturating_add(Duration::from_millis(10));
    let mut world = real::World::new(skein_world_tests::Later(skein_world::LastWord::After(after)));
    world.spawn(|| Script::new(&[Act::Pause(deadline)]));
    let _outcome = world.run(&clock, Duration::from_secs(2));
}
