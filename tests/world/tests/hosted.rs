//! Hosting through the world harness, with scripted services on raw pipes.

use skein_heap::Counting;
use skein_io::kernel::{Complete, Error, Exit, Fd, Op, Spawn, Submit};
use skein_lib::{Queue, Time, Token, Wall};
use skein_sim::{Config, Handle};
use skein_world::{Cut, Host, HostedProgram, Inherited, Machine, Memory, Referee, StartupRoot, World};
use skein_world_tests::hosted::{Act, Judge, RootMachine, Script};

#[global_allocator]
static HEAP: Counting = Counting;

fn child(spawn: &Spawn, inherited: &Inherited) -> Script {
    assert_eq!(&*spawn.args[0], b"argument");
    assert_eq!(&*spawn.env[0], b"KEY=value");
    Script::child(inherited, &[Act::Read(0), Act::Write(1, b"reply")])
}

#[test]
fn a_spawn_hosts_the_child_with_its_arguments_environment_and_pipes() {
    let mut world = World::new(7, Config::calm(), Judge, Memory::Checked).with_machine(RootMachine);
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make: child, instances: 1, operations: 8 });
    world.spawn_root(Handle::new(1), |root| {
        Script::parent(root, &[Act::Spawn, Act::Write(0, b"hello"), Act::Read(1), Act::Read(1), Act::Wait])
    });
    let outcome = world.run();
    assert_eq!(outcome.procs.len(), 2);
    assert_eq!(outcome.procs[0].received, b"reply");
    assert_eq!(outcome.procs[1].received, b"hello");
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
}

fn exchange(seed: u64) -> skein_world::Outcome<Script, RootMachine> {
    let faults = skein_sim::Faults {
        latency: 1000,
        latency_max: skein_lib::Duration::from_millis(1),
        ..skein_sim::Faults::NONE
    };
    let mut world =
        World::new(seed, Config { faults, ..Config::calm() }, Judge, Memory::Checked).with_machine(RootMachine);
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make: child, instances: 1, operations: 8 });
    world.spawn_root(Handle::new(1), |root| {
        Script::parent(root, &[Act::Spawn, Act::Write(0, b"hello"), Act::Read(1), Act::Read(1), Act::Wait])
    });
    world.run()
}

#[test]
fn hosted_processes_replay_and_each_frees_its_metered_heap() {
    let first = exchange(7);
    let second = exchange(7);
    assert_eq!(first.trace, second.trace, "hosted process records replay with the same seed");
    assert_eq!(first.heap, second.heap, "each process's peaks replay too");
    let heap = first.heap.as_ref().expect("checked memory");
    assert_eq!(heap.len(), 2, "parent and hosted child are separately metered");
    assert!(heap.iter().all(|(peak, bound)| *peak > 0 && peak <= bound), "both process heaps fit their own bounds");
    assert_ne!(first.trace, exchange(8).trace, "another seed changes the delayed completions");
    drop(first);
    drop(second);
}

fn hogging_child(_spawn: &Spawn, inherited: &Inherited) -> Script {
    let mut script = Script::child(inherited, &[]);
    script.hog = 64 * 1024;
    script
}

fn leaky_child(_spawn: &Spawn, inherited: &Inherited) -> Script {
    let mut script = Script::child(inherited, &[]);
    script.hog = 1;
    script.leak = true;
    script
}

fn empty_child_without_exit(_spawn: &Spawn, inherited: &Inherited) -> Script {
    let mut script = Script::child(inherited, &[]);
    script.terminal = None;
    script
}

fn simple(make: fn(&Spawn, &Inherited) -> Script, acts: &[Act]) -> skein_world::Outcome<Script, RootMachine> {
    let mut world = World::new(7, Config::calm(), Judge, Memory::Checked).with_machine(RootMachine);
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make, instances: 1, operations: 8 });
    world.spawn_root(Handle::new(1), |root| Script::parent(root, acts));
    world.run()
}

#[test]
#[should_panic(expected = "process 1 held")]
fn a_hosted_child_past_its_own_worst_case_fails() {
    let _outcome = simple(hogging_child, &[Act::Spawn, Act::Read(1), Act::Wait]);
}

#[test]
#[should_panic(expected = "a leak")]
fn a_hosted_child_that_leaks_fails_when_dropped() {
    drop(simple(leaky_child, &[Act::Spawn, Act::Read(1), Act::Wait]));
}

#[test]
#[should_panic(expected = "idle, unsettled")]
fn a_hosted_child_without_a_terminal_does_not_settle() {
    let _outcome = simple(empty_child_without_exit, &[Act::Spawn, Act::Read(1)]);
}

#[test]
#[should_panic(expected = "read to its end")]
fn closing_a_hosted_childs_output_without_reading_its_end_does_not_settle() {
    let _outcome = simple(leaky_child, &[Act::Spawn, Act::Wait]);
}

#[test]
fn a_parent_kills_a_hosted_child_mid_exchange() {
    use skein_world_tests::hosted::{Story, check_story, story};
    let outcome = story(7, Story::Kill);
    check_story(Story::Kill, &outcome);
}

#[test]
fn a_hosted_child_exits_before_its_parent_writes() {
    use skein_world_tests::hosted::{Story, check_story, story};
    let outcome = story(7, Story::EarlyExit);
    check_story(Story::EarlyExit, &outcome);
}

#[test]
fn a_parents_interrupt_reaches_its_hosted_child() {
    use skein_world_tests::hosted::{Story, check_story, story};
    let outcome = story(7, Story::Interrupt);
    check_story(Story::Interrupt, &outcome);
}

#[test]
fn a_parents_termination_signal_reaches_its_hosted_child() {
    use skein_world_tests::hosted::{Story, check_story, story};
    let outcome = story(7, Story::Terminate);
    check_story(Story::Terminate, &outcome);
}

#[test]
fn a_hosted_child_reaches_its_worst_case_and_still_frees_exactly_what_it_held() {
    use skein_world_tests::hosted::{Story, check_story, story};
    let outcome = story(7, Story::Memory);
    check_story(Story::Memory, &outcome);
}

#[test]
fn every_hosted_story_replays_under_its_own_memory_meter() {
    use skein_world_tests::hosted::{Story, check_story, story};
    for scenario in [Story::Exchange, Story::Kill, Story::EarlyExit, Story::Interrupt, Story::Terminate, Story::Memory]
    {
        let first = story(19, scenario);
        let second = story(19, scenario);
        check_story(scenario, &first);
        check_story(scenario, &second);
        assert_eq!(first.trace, second.trace, "the hosted story replays its records");
        assert_eq!(first.heap, second.heap, "the hosted story replays its surviving process peaks");
    }
}

fn leaky_killed_child(_spawn: &Spawn, inherited: &Inherited) -> Script {
    let _leaked = Box::leak(Box::new([0; 64]));
    Script::child(inherited, &[Act::Read(0), Act::Write(1, b"reply"), Act::Read(0)])
}

#[test]
#[should_panic(expected = "terminated process 1 freed")]
fn a_killed_childs_leak_is_checked_when_it_is_dropped() {
    use skein_io::kernel::Signal;
    drop(simple(
        leaky_killed_child,
        &[Act::Spawn, Act::Write(0, b"hello"), Act::Read(1), Act::Signal(Signal::Kill), Act::Read(1), Act::Wait],
    ));
}

/// Keeps the root's startup signal alongside the existing exchange script.
#[derive(Debug)]
struct CuttingScript {
    script: Script,
    signal: Option<Fd>,
    closing: bool,
}

impl Host for CuttingScript {
    fn iterate(&mut self, now: Time, wall: Wall) {
        if self.closing && !self.script.completions().is_empty() {
            self.closing = false;
        }
        self.script.iterate(now, wall);
        if self.script.is_empty()
            && let Some(fd) = self.signal.take()
        {
            self.script.submissions().push(Submit { op: Token::new(u64::MAX), kind: Op::Close { fd } });
            self.closing = true;
        }
    }
    fn drain(&mut self) {
        self.script.drain();
    }
    fn completions(&mut self) -> &mut Queue<Complete> {
        self.script.completions()
    }
    fn submissions(&mut self) -> &mut Queue<Submit> {
        self.script.submissions()
    }
    fn work_pending(&self, now: Time) -> bool {
        self.script.work_pending(now) || (self.script.is_empty() && self.signal.is_some())
    }
    fn next_deadline(&self) -> Option<Time> {
        self.script.next_deadline()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        self.script.next_policy_deadline()
    }
    fn is_empty(&self) -> bool {
        self.script.is_empty() && self.signal.is_none() && !self.closing
    }
    fn worst_case(&self) -> u64 {
        self.script.worst_case()
    }
    fn operations(&self) -> u32 {
        self.script.operations()
    }
}

#[derive(Debug, Default)]
struct CuttingMachine {
    opened: u64,
    cuts: u32,
}

impl Machine for CuttingMachine {
    fn open_root(&mut self, path: &[u8]) -> Result<Handle, Error> {
        assert_eq!(path, b"launch");
        self.opened += 1;
        Ok(Handle::new(self.opened))
    }
    fn close_root(&mut self, _root: Handle) {}
    fn cut(&mut self, cut: Cut, held: &[Handle], _seed: u64) {
        assert_eq!(cut, Cut::PowerLoss);
        assert_eq!(held, &[Handle::new(1)]);
        self.cuts += 1;
    }
    fn step(&mut self, call: skein_sim::Call, answers: &mut Queue<skein_sim::Answer>) {
        RootMachine.step(call, answers);
    }
}

struct CuttingJudge;

impl Referee<CuttingScript> for CuttingJudge {
    fn act(&mut self, _now: Time, _procs: &mut [CuttingScript]) {}
    fn observe(&mut self, _now: Time, _procs: &[CuttingScript]) {}
    fn next_deadline(&self) -> Option<Time> {
        None
    }
    fn overdue(&self, _now: Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        true
    }
}

fn cutting_parent(inherited: &Inherited) -> CuttingScript {
    CuttingScript {
        script: Script::parent(
            inherited.roots[0].1,
            &[Act::Spawn, Act::Write(0, b"hello"), Act::Read(1), Act::Read(1), Act::Wait],
        ),
        signal: Some(inherited.signal),
        closing: false,
    }
}

fn cutting_child(spawn: &Spawn, inherited: &Inherited) -> CuttingScript {
    CuttingScript { script: child(spawn, inherited), signal: None, closing: false }
}

fn cutting_idle(inherited: &Inherited) -> CuttingScript {
    CuttingScript { script: Script::child(inherited, &[]), signal: None, closing: false }
}

fn cutting_world() -> World<CuttingScript, CuttingJudge, CuttingMachine> {
    let mut world =
        World::new(7, Config::calm(), CuttingJudge, Memory::Checked).with_machine(CuttingMachine::default());
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make: cutting_child, instances: 1, operations: 8 });
    world.spawn_restartable(
        vec![StartupRoot { name: Box::from(&b"launch"[..]), path: Box::from(&b"launch"[..]) }],
        cutting_parent,
    );
    world
}

#[test]
fn a_power_loss_drops_hosted_children_and_restarts_the_roots() {
    let mut world = cutting_world();
    world.spawn_restartable(vec![], cutting_idle);
    world.cut(0, 2, Cut::PowerLoss);
    let (mut roots, mut old_child, mut restarted) = (None, None, false);
    let outcome = world.run_with_inputs(|_, pids, _| {
        let original = *roots.get_or_insert([pids[0], pids[1]]);
        if pids[0] == original[0] && pids.len() == 3 {
            old_child = Some(pids[2]);
        } else if pids[0] != original[0] && !restarted {
            assert!(old_child.is_some(), "the child was running when power failed");
            assert_ne!(pids[1], original[1], "the other root also restarted");
            assert_eq!(pids.len(), 2, "only roots remain immediately after the cut");
            restarted = true;
        }
        if restarted && pids.len() == 3 {
            assert_ne!(Some(pids[2]), old_child, "the parent launches a fresh child");
        }
    });
    assert!(restarted);
    assert_eq!(outcome.machine.cuts, 1);
    assert_eq!(outcome.machine.opened, 2);
    assert_eq!(outcome.procs.len(), 3);
    assert_eq!(outcome.procs[0].script.received, b"reply");
    assert_eq!(outcome.procs[2].script.received, b"hello");
    assert_eq!(outcome.procs[0].script.child_exit, Some(Exit::Code(0)));
}

fn admit_without_factory(world: &mut World<CuttingScript, CuttingJudge, CuttingMachine>) {
    world.spawn(|| CuttingScript { script: Script::new(&[]), signal: None, closing: false });
}

#[test]
fn a_power_loss_needs_every_roots_factory() {
    for (label, before_planning, message) in [
        ("planning", true, "power loss needs every process's restart factory"),
        ("execution", false, "power loss needs every live process's factory"),
    ] {
        let refusal = std::panic::catch_unwind(|| {
            let mut world = cutting_world();
            if before_planning {
                admit_without_factory(&mut world);
            }
            world.cut(0, 2, Cut::PowerLoss);
            if !before_planning {
                admit_without_factory(&mut world);
            }
            drop(world.run());
        })
        .expect_err(label);
        let text = refusal
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| refusal.downcast_ref::<String>().map(String::as_str))
            .expect("panic text");
        assert_eq!(text, message, "{label}");
    }
}

fn launch_directory(
    dir: &[u8],
    max_fds: u32,
    exchange: bool,
    program: HostedProgram<Script>,
) -> skein_world::Outcome<Script, skein_world_tests::launch::LaunchFiles> {
    use skein_world_tests::launch;
    let mut machine = launch::LaunchFiles::default();
    let root = machine.parent_root();
    let mut world = World::new(7, Config { max_fds, ..Config::calm() }, Judge, Memory::Checked).with_machine(machine);
    world.host_roots(program, launch::roots);
    world.spawn_root(root, |fd| launch::parent(fd, b".", dir, exchange));
    world.run()
}

#[test]
fn a_hosted_child_inherits_its_launch_directory() {
    let mut outcome = launch_directory(b"work", Config::calm().max_fds, true, skein_world_tests::launch::program());
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
    outcome.machine.check_file();
    outcome.machine.finish();
}

#[test]
fn a_launch_directory_beneath_nothing_refuses_the_spawn() {
    for (dir, error) in [(b"missing".as_slice(), Error::NotFound), (b"..", Error::Escape), (b"/", Error::Escape)] {
        let mut outcome = launch_directory(dir, Config::calm().max_fds, false, skein_world_tests::launch::program());
        assert_eq!(outcome.procs.len(), 1, "{dir:?}: no child admitted");
        assert!(outcome.procs[0].results.contains(&Err(error)), "{dir:?}: the open's error");
        outcome.machine.finish();
    }
}

#[test]
fn the_launch_directory_counts_toward_the_childs_descriptor_limit() {
    use skein_world_tests::launch;
    let mut outcome = launch_directory(b"work", launch::CHILD_DESCRIPTORS - 1, false, launch::program());
    assert_eq!(outcome.procs.len(), 1);
    assert!(outcome.procs[0].results.contains(&Err(Error::TooManyOpenFiles)));
    outcome.machine.finish();
}

#[test]
fn a_world_with_no_machine_hands_no_launch_directory() {
    fn no_directory(_spawn: &Spawn, inherited: &Inherited) -> Script {
        assert!(inherited.dir.is_none());
        Script::child(inherited, &[])
    }
    let mut world = skein_sim_world::World::new(7, Config::calm());
    world.host(HostedProgram { program: Box::from(&b"hosted"[..]), make: no_directory, instances: 1, operations: 8 });
    world.spawn_root(Handle::new(1), |root| Script::parent(root, &[Act::Spawn]));
    // Observe admission before the synthetic parent root reaches any machine call.
    while world.processes().len() < 2 {
        world.step();
        if !world.busy() {
            world.advance(world.next_due().expect("spawn completion due"));
        }
    }
    assert_eq!(world.processes().len(), 2);
}

#[test]
fn a_launch_directory_fits_at_the_childs_descriptor_limit() {
    use skein_world_tests::launch;
    fn closing_child(_spawn: &Spawn, inherited: &Inherited) -> Script {
        assert!(inherited.dir.is_some());
        Script::child(inherited, &[Act::Read(0)])
    }
    let mut program = launch::program();
    program.make = closing_child;
    let mut outcome = launch_directory(b"work", launch::CHILD_DESCRIPTORS, true, program);
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
    outcome.machine.finish();
}
