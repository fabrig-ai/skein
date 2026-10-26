//! Binary startup and outside observations over the world's shared ring.

use skein_io::kernel::{Complete, Exit, Op, Signal, Submit};
use skein_lib::{Duration, Queue, Time, Wall};
use skein_shell::Clock;
use skein_world::end_to_end::{Binary, Command, Containment, Mode, StartError};
use skein_world::{Host, Referee, real};
use skein_world_tests::hosted::{Act, Script};

const WORLD_PATIENCE: Duration = Duration::from_secs(2);

/// Whether `fd` is open: a poll that does not wait answers every open
/// descriptor, whatever it is, and refuses a closed one as EBADF. A `Stat`
/// of `/dev/fd` would not do on macOS, where the child's handle is a kqueue.
fn is_open(fd: skein_io::kernel::Fd) -> bool {
    // EBADF is 9 on Linux and macOS alike.
    skein_shell::wait_readable(fd, Duration::ZERO) != Err(9)
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
const ALL: [Containment; 3] = [Containment::Walk, Containment::Best, Containment::Group];
#[cfg(not(target_os = "linux"))]
const ALL: [Containment; 1] = [Containment::Group];

#[derive(Debug)]
enum Process {
    Child(Binary),
    Person(Script),
}

impl Process {
    fn host(&self) -> &dyn Host {
        match self {
            Self::Child(child) => child,
            Self::Person(person) => person,
        }
    }

    fn host_mut(&mut self) -> &mut dyn Host {
        match self {
            Self::Child(child) => child,
            Self::Person(person) => person,
        }
    }
}

impl Host for Process {
    fn drain(&mut self) {
        self.host_mut().drain();
    }

    fn iterate(&mut self, now: Time, wall: Wall) {
        self.host_mut().iterate(now, wall);
    }
    fn completions(&mut self) -> &mut Queue<Complete> {
        self.host_mut().completions()
    }
    fn submissions(&mut self) -> &mut Queue<Submit> {
        self.host_mut().submissions()
    }
    fn work_pending(&self, now: Time) -> bool {
        self.host().work_pending(now)
    }
    fn next_deadline(&self) -> Option<Time> {
        self.host().next_deadline()
    }

    fn next_policy_deadline(&self) -> Option<Time> {
        self.host().next_policy_deadline()
    }

    fn is_empty(&self) -> bool {
        self.host().is_empty()
    }
    fn worst_case(&self) -> u64 {
        self.host().worst_case()
    }
    fn operations(&self) -> u32 {
        self.host().operations()
    }
}

struct Judge {
    signal: bool,
    sent: bool,
    act_at: Option<Time>,
}

impl Referee<Process> for Judge {
    fn act(&mut self, _now: Time, procs: &mut [Process]) {
        let (child, person) = procs.split_at_mut(1);
        let Process::Child(child) = &child[0] else { panic!("binary observer") };
        if let Some(Process::Person(person)) = person.first_mut()
            && person.awaiting.is_some_and(|bytes| child.stderr().windows(bytes.len()).any(|line| line == bytes))
        {
            person.awaiting = None;
        }
        if self.signal && !self.sent {
            let Process::Child(child) = &mut procs[0] else { panic!("binary observer") };
            if child.stderr().ends_with(b"ready\n") {
                child.signal(Signal::Terminate);
                self.sent = true;
            }
        }
    }
    fn observe(&mut self, now: Time, procs: &[Process]) {
        let Process::Child(child) = &procs[0] else { panic!("binary observer") };
        let awaiting = match procs.get(1) {
            Some(Process::Person(person)) => {
                person.awaiting.is_some_and(|bytes| child.stderr().windows(bytes.len()).any(|line| line == bytes))
            }
            Some(Process::Child(_)) | None => false,
        };
        self.act_at = (awaiting || (self.signal && !self.sent && child.stderr().ends_with(b"ready\n"))).then_some(now);
    }
    fn next_deadline(&self) -> Option<Time> {
        self.act_at
    }
    fn overdue(&self, _now: Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        !self.signal || self.sent
    }
}

fn command(script: &str) -> Command {
    Command {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), script.into(), "fixture".into(), "argument".into()],
        environment: vec![("KEY".into(), "value".into())],
        directory: "/tmp".into(),
    }
}

fn run(command: Command, mode: Mode, acts: &[Act], signal: bool) -> real::Outcome<Process> {
    let mut binary = Binary::start(command, mode, 16 * 1024).unwrap_or_else(|error| match error {
        StartError::Ring(why) => panic!("io_uring is not usable here, so end to end cannot run: {why}"),
        StartError::Directory(errno) => panic!("binary directory failed: errno {errno}"),
        StartError::Tree(error) => panic!("tree startup failed: {error:?}"),
        StartError::Child(error) => panic!("binary startup failed: {error:?}"),
    });
    let streams = binary.take_streams().expect("the person's streams");
    let mut world = real::World::new(Judge { signal, sent: false, act_at: None });
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    world.spawn_with_fds(streams.descriptors(), || Process::Person(Script::pipes(streams.descriptors(), acts)));
    world.run(&Clock::new(), Duration::from_secs(2))
}

fn observations(outcome: &real::Outcome<Process>) -> (&Binary, &Script) {
    let Process::Child(child) = &outcome.procs[0] else { panic!("binary observer") };
    let Process::Person(person) = &outcome.procs[1] else { panic!("scripted person") };
    (child, person)
}

// The directory is compared as the system resolves it: macOS's /tmp is a link
// to /private/tmp, and the child's shell names its directory without links.

/// The containments a supervised case runs under: the walk (`Containment::Walk`)
/// is Linux's, its subreaper and pidfds (shell.md, 9); macOS has the group alone.
const WALKS: &[bool] = if cfg!(target_os = "linux") { &[true, false] } else { &[false] };

#[test]
fn binary_pipes_preserve_arguments_environment_directory_stderr_and_exit() {
    let outcome = run(
        command(
            "test \"$(pwd -P)\" = \"$(cd /tmp && pwd -P)\" || exit 90; test \"$1:$KEY\" = argument:value || exit 91; read value; printf 'answer:%s' \"$value\"; printf 'diagnostic' >&2; exit 17",
        ),
        Mode::Pipes,
        &[Act::Write(0, b"hello\n"), Act::Close(0), Act::Read(0), Act::Read(0), Act::Read(0)],
        false,
    );
    let (child, person) = observations(&outcome);
    assert_eq!(person.received, b"answer:hello");
    assert_eq!(child.stderr(), b"diagnostic");
    assert_eq!(child.exit_status(), Some(Exit::Code(17)));
}

#[test]
fn terminal_is_controlling_and_the_persons_interrupt_reaches_only_the_child() {
    let outcome = run(
        command(
            "test -t 0 && test -t 1 && test ! -t 2 || exit 90; test -r /dev/tty || exit 91; trap 'printf interrupted >&2; exit 23' INT; printf 'ready\\n'; read value; exit 92",
        ),
        Mode::Terminal,
        &[
            Act::ReadUntil(0, b"ready\r\n"),
            Act::Write(0, b"\x03"),
            Act::Read(0),
            Act::Read(0),
            Act::Read(0),
            Act::Read(0),
        ],
        false,
    );
    let (child, person) = observations(&outcome);
    assert!(person.received.starts_with(b"ready\r\n"));
    assert_eq!(child.stderr(), b"interrupted");
    assert_eq!(child.exit_status(), Some(Exit::Code(23)));
}

#[test]
fn referee_signal_and_child_exit_are_shared_ring_events() {
    let outcome = run(
        command("trap 'printf terminated >&2; exit 31' TERM; printf 'ready\\n' >&2; read value"),
        Mode::Pipes,
        &[Act::Read(1), Act::Read(1)],
        true,
    );
    let (child, _) = observations(&outcome);
    assert_eq!(child.stderr(), b"ready\nterminated");
    assert_eq!(child.exit_status(), Some(Exit::Code(31)));
}

#[test]
fn invalid_binary_reports_a_startup_failure() {
    let mut command = command("exit 0");
    command.program = "/missing-skein-binary".into();
    assert!(matches!(
        Binary::start(command, Mode::Terminal, 1024),
        Err(StartError::Child(skein_io::kernel::Error::NotFound))
    ));
}

#[test]
fn an_abandoned_binary_is_reaped_and_its_untransferred_descriptors_are_closed() {
    let binary = Binary::start(command("read value"), Mode::Pipes, 1024).expect("the binary starts");
    let descriptors = binary.descriptors();
    drop(binary);
    for descriptor in descriptors {
        assert!(!is_open(descriptor), "abandonment closes every owned descriptor");
    }
}

struct TreeJudge {
    end: bool,
    sent: bool,
}

impl Referee<Process> for TreeJudge {
    fn act(&mut self, _now: Time, procs: &mut [Process]) {
        let Process::Child(child) = &mut procs[0] else { panic!("binary observer") };
        if self.end && !self.sent && ready_descendant(child.stderr()).is_some() {
            child.end();
            self.sent = true;
        }
    }
    fn observe(&mut self, _now: Time, _procs: &[Process]) {}
    fn next_deadline(&self) -> Option<Time> {
        None
    }
    fn overdue(&self, _now: Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        !self.end || self.sent
    }
}

fn fixture(mode: &str) -> Command {
    Command {
        program: skein_scratch::paths::binary("world_process_fixture"),
        arguments: if mode == "exit" { vec!["exit".into(), "0".into()] } else { vec![mode.into()] },
        environment: Vec::new(),
        directory: "/tmp".into(),
    }
}

fn tree_run(
    mode: &str,
    containment: Containment,
    expectation: skein_world::end_to_end::Expectation,
) -> real::Outcome<Process> {
    let binary = Binary::start_contained(fixture(mode), Mode::Pipes, 4096, expectation, containment)
        .expect("tree fixture starts");
    drive_tree(binary, mode == "fork-live")
}

fn drive_tree(binary: Binary, end: bool) -> real::Outcome<Process> {
    let mut world = real::World::new(TreeJudge { end, sent: false });
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    world.run(&Clock::new(), Duration::from_secs(2))
}

fn tree_child(outcome: &real::Outcome<Process>) -> &Binary {
    let Process::Child(child) = &outcome.procs[0] else { panic!("binary observer") };
    child
}

fn descendant(stderr: &[u8]) -> u32 {
    ready_descendant(stderr).expect("fixture names its descendant")
}

fn ready_descendant(stderr: &[u8]) -> Option<u32> {
    std::str::from_utf8(stderr)
        .expect("fixture stderr is text")
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .find_map(|line| line.strip_prefix("descendant:").and_then(|pid| pid.trim().parse().ok()))
}

fn assert_tree_counted(child: &Binary, containment: Containment) {
    use skein_world::end_to_end::{Method, PeakScope};
    let counts = child.counts().expect("the whole tree settled before counts");
    eprintln!("tree counts: {counts:?}");
    assert!(counts.peak_rss_bytes > 0, "the kernel counts resident memory");
    assert!(
        counts.user.as_nanos().checked_add(counts.system.as_nanos()).expect("CPU fits") > 0,
        "the kernel counts tree CPU"
    );
    match containment {
        Containment::Walk => assert_eq!(counts.method, Method::Walk),
        Containment::Group => assert_eq!(counts.method, Method::Group),
        Containment::Best => {}
    }
    match counts.method {
        Method::Cgroup => assert_eq!(counts.peak_scope, PeakScope::Tree),
        Method::Walk | Method::Group => assert!(matches!(counts.peak_scope, PeakScope::LargestProcess { .. })),
    }
}

#[test]
fn a_descendant_left_in_the_group_is_ended_and_counted() {
    for containment in ALL {
        let outcome = tree_run("fork-exit", containment, skein_world::end_to_end::Expectation::Measure);
        let child = tree_child(&outcome);
        assert_eq!(skein_shell::process_live(descendant(child.stderr())), Ok(false), "the descendant was reaped");
        assert_tree_counted(child, containment);
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_descendant_that_leaves_its_group_and_session_is_still_settled() {
    // A group cannot see a descendant that leaves it (examples.md, 6; shell.md, 9).
    for containment in [Containment::Walk, Containment::Best] {
        let outcome = tree_run("escape-exit", containment, skein_world::end_to_end::Expectation::Measure);
        let child = tree_child(&outcome);
        assert_eq!(
            skein_shell::process_live(descendant(child.stderr())),
            Ok(false),
            "the escaped descendant was reaped"
        );
        assert_tree_counted(child, containment);
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn an_already_exited_descendant_that_left_its_session_is_still_reaped() {
    use skein_world::end_to_end::Expectation;

    // A group cannot see a descendant that leaves it (examples.md, 6; shell.md, 9).
    for containment in [Containment::Walk, Containment::Best] {
        let binary = Binary::start_contained(
            fixture("escape-zombie-exit"),
            Mode::Pipes,
            4096,
            Expectation::Measure,
            containment,
        )
        .expect("the exited-descendant fixture starts");
        let pidfd = binary.descriptors()[0];
        let clock = Clock::new();
        let until = clock.now().now.saturating_add(Duration::from_secs(2));
        // No Binary iteration runs until the parent and its escaped descendant
        // exited. Discovery must handle the exit-before-first-refresh race.
        while skein_shell::poll_child(pidfd, false).expect("the binary remains a child").is_none() {
            assert!(clock.now().now < until, "the fixture exits before its deadline");
        }
        let mut world = real::World::new(TreeJudge { end: false, sent: false });
        world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
        let outcome = world.run(&clock, Duration::from_secs(2));
        let child = tree_child(&outcome);
        assert_tree_counted(child, containment);
        assert_eq!(
            skein_shell::process_live(descendant(child.stderr())),
            Ok(false),
            "an already-exited escaped descendant is reaped before tree settlement"
        );
    }
}

#[test]
fn a_binary_that_exits_at_once_settles_with_counts_above_zero() {
    for containment in ALL {
        let outcome = tree_run("exit", containment, skein_world::end_to_end::Expectation::EndsWithBinary);
        assert_tree_counted(tree_child(&outcome), containment);
    }
}

#[test]
fn an_expected_end_fails_naming_a_process_still_running() {
    for containment in ALL {
        let binary = Binary::start_contained(
            fixture("fork-exit"),
            Mode::Pipes,
            4096,
            skein_world::end_to_end::Expectation::EndsWithBinary,
            containment,
        )
        .expect("the fixture starts");
        let pid = skein_shell::child_id(binary.descriptors()[0]).expect("the leader's pid");
        let failure =
            std::panic::catch_unwind(|| drive_tree(binary, false)).expect_err("the fixture left a living descendant");
        let text = failure.downcast_ref::<String>().expect("the keeper fails with its diagnostic");
        if containment == Containment::Group {
            assert!(text.contains("process group"), "the diagnostic names the group: {text}");
            assert!(text.contains(&format!("process group {pid} ")), "the diagnostic names the binary's group: {text}");
            assert!(!skein_shell::group_live(pid).expect("the group can be probed"));
        } else {
            assert!(text.contains("tree processes still ran: ["), "the diagnostic names the leftover PID: {text}");
        }
    }
}

#[test]
fn a_measured_binary_is_killed_and_counted() {
    for containment in ALL {
        let outcome = tree_run("fork-live", containment, skein_world::end_to_end::Expectation::Measure);
        let child = tree_child(&outcome);
        assert_eq!(child.exit_status(), Some(Exit::Signal(9)));
        assert_eq!(skein_shell::process_live(descendant(child.stderr())), Ok(false));
        assert_tree_counted(child, containment);
    }
}

#[test]
fn a_dropped_observer_ends_its_tree() {
    for cancels_first in [true, false] {
        abandoned_observer(None, cancels_first);
    }
}

#[test]
fn an_abandoned_observer_drains_on_readiness() {
    for cancels_first in [true, false] {
        abandoned_observer(Some(skein_shell::Backend::Readiness), cancels_first);
    }
}

fn abandoned_observer(backend: Option<skein_shell::Backend>, cancels_first: bool) {
    use skein_shell::{Config, Kernel, Wait};
    for containment in ALL {
        let mut child = Binary::start_contained(
            fixture("fork-live"),
            Mode::Pipes,
            4096,
            skein_world::end_to_end::Expectation::Measure,
            containment,
        )
        .expect("the fixture starts");
        let config = Config { operations: child.operations() };
        let mut kernel = match backend {
            Some(backend) => Kernel::open_on(config, backend),
            None => Kernel::open(config),
        }
        .expect("a real kernel");
        let clock = Clock::new();
        let until = clock.now().now.saturating_add(Duration::from_secs(2));
        while ready_descendant(child.stderr()).is_none() {
            let now = clock.now();
            assert!(now.now < until, "the fixture became ready before its deadline");
            kernel.reap(child.completions());
            child.iterate(now.now, now.wall);
            kernel.submit(child.submissions(), Wait::Until(now.now.saturating_add(Duration::from_millis(1))));
        }
        let pid = descendant(child.stderr());
        let descriptors = child.descriptors();
        assert!(kernel.in_flight() > 0, "abandonment interrupts observation");
        child.abandon();
        let cancels = child.submissions().len();
        assert_eq!(cancels, 2, "the pending wait and stderr read each get a cancel");
        child.abandon();
        assert_eq!(child.submissions().len(), cancels, "repeated abandonment adds no cancels");
        assert!(!child.is_empty(), "abandonment waits for its completions");
        for fd in &descriptors {
            assert!(is_open(*fd), "pending descriptors stay open");
        }
        kernel.submit(child.submissions(), Wait::No);
        let mut completed = Queue::with_capacity(child.operations());
        while kernel.in_flight() > 0 {
            kernel.reap(&mut completed);
            if kernel.in_flight() > 0 {
                kernel.submit(&mut Queue::with_capacity(0), Wait::Until(until));
            }
            assert!(clock.now().now < until, "the killed tree completes its interrupted operations");
        }
        let mut held = Queue::with_capacity(child.operations());
        while let Some(complete) = completed.pop() {
            if matches!(complete.kind, Op::Cancel { .. }) == cancels_first {
                child.completions().push(complete);
            } else {
                held.push(complete);
            }
        }
        let now = clock.now();
        child.iterate(now.now, now.wall);
        assert!(child.submissions().is_empty(), "neither target nor cancel answers alone permit closure");
        assert!(!child.is_empty(), "the other answers still belong to the observer");
        for fd in &descriptors {
            assert!(is_open(*fd), "settling descriptors stay open");
        }
        while let Some(complete) = held.pop() {
            child.completions().push(complete);
        }
        drain_abandoned(&mut child, &mut kernel, &clock, until);
        drop(child);
        assert_eq!(skein_shell::process_live(pid), Ok(false), "the abandoned descendant was reaped");
        for fd in descriptors {
            assert!(!is_open(fd));
        }
    }
}

fn drain_abandoned(child: &mut Binary, kernel: &mut skein_shell::Kernel, clock: &Clock, until: Time) {
    while !child.is_empty() || kernel.in_flight() > 0 {
        kernel.reap(child.completions());
        let now = clock.now();
        child.iterate(now.now, now.wall);
        if !child.is_empty() || kernel.in_flight() > 0 {
            kernel.submit(child.submissions(), skein_shell::Wait::Until(until));
        }
        assert!(clock.now().now < until, "the killed tree completes its interrupted operations");
    }
}

#[test]
fn an_observer_can_be_abandoned_before_its_first_submission() {
    use skein_shell::{Backend, Config, Kernel};
    for backend in [None, Some(Backend::Readiness)] {
        for mode in [Mode::Pipes, Mode::Terminal] {
            for queued in [false, true] {
                let mut child = Binary::start_contained(
                    fixture("never"),
                    mode,
                    4096,
                    skein_world::end_to_end::Expectation::Measure,
                    Containment::Group,
                )
                .expect("the fixture starts");
                let descriptors = child.descriptors();
                let config = Config { operations: child.operations() };
                let mut kernel = match backend {
                    Some(backend) => Kernel::open_on(config, backend),
                    None => Kernel::open(config),
                }
                .expect("a real kernel");
                let clock = Clock::new();
                let now = clock.now();
                let until = now.now.saturating_add(Duration::from_secs(2));
                if queued {
                    child.iterate(now.now, now.wall);
                }
                child.abandon();
                assert!(child.work_pending(now.now), "abandonment has submission or settlement work");
                drain_abandoned(&mut child, &mut kernel, &clock, until);
                for fd in descriptors {
                    assert!(!is_open(fd));
                }
            }
        }
    }
}

#[test]
fn emergency_drop_leaves_outstanding_descriptors_and_the_waited_leader_owned() {
    use skein_shell::{Backend, Config, Kernel, Wait};
    for backend in [None, Some(Backend::Readiness)] {
        for containment in ALL {
            let mut child = Binary::start_contained(
                fixture("never"),
                Mode::Pipes,
                4096,
                skein_world::end_to_end::Expectation::Measure,
                containment,
            )
            .expect("the fixture starts");
            let leader = child.descriptors()[0];
            let config = Config { operations: child.operations() };
            let mut kernel = match backend {
                Some(backend) => Kernel::open_on(config, backend),
                None => Kernel::open(config),
            }
            .expect("a real kernel");
            let clock = Clock::new();
            let now = clock.now();
            let until = now.now.saturating_add(Duration::from_secs(2));
            child.iterate(now.now, now.wall);
            kernel.submit(child.submissions(), Wait::No);
            let descriptors = child.descriptors();
            drop(child);
            for fd in &descriptors {
                assert!(is_open(*fd), "an outstanding descriptor stays open");
            }
            let mut completions = Queue::with_capacity(8);
            while kernel.in_flight() > 0 {
                kernel.reap(&mut completions);
                while completions.pop().is_some() {}
                if kernel.in_flight() > 0 {
                    kernel.submit(&mut Queue::with_capacity(0), Wait::Until(until));
                }
                assert!(clock.now().now < until, "the killed binary completes observation through open descriptors");
            }
            assert_eq!(
                skein_shell::poll_child(leader, false),
                Ok(Some(Exit::Signal(9))),
                "Drop retained the unreaped leader"
            );
            skein_shell::abandon_binary(Some(leader), &descriptors);
            for fd in descriptors {
                assert!(!is_open(fd));
            }
        }
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_walk_keeps_an_existing_parts_child_untouched() {
    let mut other = std::process::Command::new("/bin/sleep").arg("60").spawn().expect("another part starts its child");
    let outcome = tree_run("fork-exit", Containment::Walk, skein_world::end_to_end::Expectation::Measure);
    assert_tree_counted(tree_child(&outcome), Containment::Walk);
    assert!(other.try_wait().expect("the other child can be checked").is_none(), "the keeper left its child alone");
    other.kill().expect("the other part ends its child");
    other.wait().expect("the other part reaps its child");
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_walk_refuses_another_observer_during_its_accounting_interval() {
    let child = Binary::start_with_tree(
        fixture("fork-live"),
        Mode::Pipes,
        4096,
        skein_world::end_to_end::Expectation::Measure,
        true,
    )
    .expect("the first observer starts");
    let second = Binary::start_with_tree(
        fixture("exit"),
        Mode::Pipes,
        4096,
        skein_world::end_to_end::Expectation::Measure,
        false,
    );
    assert!(matches!(second, Err(StartError::Tree(skein_io::kernel::Error::Other(16)))));
    drop(child);
    let outcome = tree_run("exit", Containment::Walk, skein_world::end_to_end::Expectation::Measure);
    assert_tree_counted(tree_child(&outcome), Containment::Walk);
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn delegated_cgroups_keep_two_binaries_separate_until_both_are_removed() {
    use skein_world::end_to_end::{Expectation, Method};
    let first = Binary::start_with_tree(fixture("fork-live"), Mode::Pipes, 4096, Expectation::Measure, false)
        .expect("the first observer starts");
    let second = Binary::start_with_tree(fixture("exit"), Mode::Pipes, 4096, Expectation::Measure, false);
    let second = match second {
        Ok(second) => second,
        Err(StartError::Tree(skein_io::kernel::Error::Other(16))) => {
            drop(first);
            eprintln!("no delegated cgroup: concurrent observers require containment");
            return;
        }
        Err(error) => panic!("second tree startup failed: {error:?}"),
    };
    drop(first);
    let mut world = real::World::new(TreeJudge { end: false, sent: false });
    world.spawn_with_fds(second.descriptors(), || Process::Child(second));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(tree_child(&outcome).counts().expect("the second tree settled").method, Method::Cgroup);
    assert_tree_counted(tree_child(&outcome), Containment::Best);
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn the_last_observer_restores_the_original_subreaper_setting() {
    let original = skein_shell::subreaper().expect("the test's subreaper setting");
    for previous in [false, true] {
        skein_shell::set_subreaper(previous).expect("the scenario sets its prior adoption policy");
        for force_walk in [true, false] {
            let outcome = tree_run(
                "exit",
                if force_walk { Containment::Walk } else { Containment::Best },
                skein_world::end_to_end::Expectation::Measure,
            );
            assert_tree_counted(tree_child(&outcome), if force_walk { Containment::Walk } else { Containment::Best });
            assert_eq!(skein_shell::subreaper().expect("the restored setting"), previous);
            let binary = Binary::start_with_tree(
                fixture("fork-live"),
                Mode::Pipes,
                4096,
                skein_world::end_to_end::Expectation::Measure,
                force_walk,
            )
            .expect("the abandoned observer starts");
            assert!(skein_shell::subreaper().expect("adoption while a keeper is active"));
            drop(binary);
            assert_eq!(skein_shell::subreaper().expect("the setting after abandonment"), previous);
        }
    }
    skein_shell::set_subreaper(original).expect("the test restores its initial adoption policy");
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn concurrent_observers_keep_adoption_enabled_until_the_last_one_ends() {
    use skein_world::end_to_end::Expectation;

    let original = skein_shell::subreaper().expect("the test's subreaper setting");
    skein_shell::set_subreaper(false).expect("no prior adopter");
    let first = Binary::start_with_tree(fixture("fork-live"), Mode::Pipes, 4096, Expectation::Measure, false)
        .expect("the first observer starts");
    let second = Binary::start_with_tree(fixture("fork-live"), Mode::Pipes, 4096, Expectation::Measure, false);
    let second = match second {
        Ok(second) => second,
        Err(StartError::Tree(skein_io::kernel::Error::Other(16))) => {
            drop(first);
            assert!(!skein_shell::subreaper().expect("the final keeper restores adoption"));
            skein_shell::set_subreaper(original).expect("the test restores its initial policy");
            eprintln!("no delegated cgroup: concurrent observers require containment");
            return;
        }
        Err(error) => panic!("second observer startup failed: {error:?}"),
    };
    drop(first);
    assert!(skein_shell::subreaper().expect("the remaining keeper still adopts"));
    drop(second);
    assert!(!skein_shell::subreaper().expect("the last keeper restores adoption"));
    skein_shell::set_subreaper(original).expect("the test restores its initial policy");
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_failed_binary_start_restores_the_original_subreaper_setting() {
    let original = skein_shell::subreaper().expect("the test's subreaper setting");
    let mut missing = fixture("exit");
    missing.program = "/missing-skein-binary".into();
    assert!(matches!(Binary::start(missing, Mode::Pipes, 1024), Err(StartError::Child(_))));
    assert_eq!(skein_shell::subreaper().expect("the setting after failed startup"), original);
}

#[derive(Clone, Copy)]
enum SupervisedTrigger {
    // The detached-descendant cases use Linux's tree keeper (shell.md, 9).
    #[cfg(target_os = "linux")]
    Ready,
    // The detached-descendant case uses Linux's tree keeper (shell.md, 9).
    #[cfg(target_os = "linux")]
    StderrOverflow,
    // The detached-descendant cases use Linux's tree keeper (shell.md, 9).
    #[cfg(target_os = "linux")]
    LeaderExit,
    Bound,
    Never,
}

struct SupervisedJudge {
    trigger: SupervisedTrigger,
    grace: Duration,
    term_at: Option<Time>,
    kill_at: Option<Time>,
    next: Option<Time>,
    saw_retained_exit: bool,
    saw_grace: bool,
}

impl Referee<Process> for SupervisedJudge {
    fn act(&mut self, now: Time, procs: &mut [Process]) {
        let Process::Child(child) = &mut procs[0] else { panic!("binary observer") };
        if self.term_at.is_none() {
            let ready = match self.trigger {
                #[cfg(target_os = "linux")]
                SupervisedTrigger::Ready => ready_descendant(child.stderr()).is_some(),
                #[cfg(target_os = "linux")]
                SupervisedTrigger::StderrOverflow => child.stderr_overflowed(),
                #[cfg(target_os = "linux")]
                SupervisedTrigger::LeaderExit => child.exit_status().is_some(),
                SupervisedTrigger::Bound => now >= Time::ZERO.saturating_add(Duration::from_millis(10)),
                SupervisedTrigger::Never => false,
            };
            if ready {
                child.signal_tree(Signal::Terminate);
                self.term_at = Some(now);
            }
        } else if let Some(term_at) = self.term_at
            && child.counts().is_none()
        {
            let grace = term_at.saturating_add(self.grace);
            if now >= grace && self.kill_at.is_none() {
                child.signal_tree(Signal::Kill);
                self.kill_at = Some(now);
            }
        }
    }

    fn observe(&mut self, now: Time, procs: &[Process]) {
        #[cfg(target_os = "macos")]
        let _ = now;
        let Process::Child(child) = &procs[0] else { panic!("binary observer") };
        if child.exit_status().is_some() && !child.tree_status().live_pids.is_empty() {
            assert!(child.counts().is_none(), "a living tree retains its keeper after leader exit");
            self.saw_retained_exit = true;
            if let Some(term_at) = self.term_at
                && now < term_at.saturating_add(self.grace)
            {
                self.saw_grace = true;
                assert!(child.tree_status().killed_pids.is_empty(), "the keeper leaves escalation to its referee");
            }
        }
        self.next = if child.counts().is_some() {
            None
        } else if let Some(term_at) = self.term_at {
            self.kill_at.is_none().then_some(term_at.saturating_add(self.grace))
        } else {
            match self.trigger {
                #[cfg(target_os = "linux")]
                SupervisedTrigger::Ready => ready_descendant(child.stderr()).is_some().then_some(now),
                #[cfg(target_os = "linux")]
                SupervisedTrigger::StderrOverflow => child.stderr_overflowed().then_some(now),
                #[cfg(target_os = "linux")]
                SupervisedTrigger::LeaderExit => child.exit_status().is_some().then_some(now),
                SupervisedTrigger::Bound => Some(Time::ZERO.saturating_add(Duration::from_millis(10))),
                SupervisedTrigger::Never => None,
            }
        };
    }

    fn next_deadline(&self) -> Option<Time> {
        self.next
    }
    fn overdue(&self, _now: Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        if self.kill_at.is_some() {
            self.saw_grace && self.saw_retained_exit
        } else {
            match self.trigger {
                #[cfg(target_os = "linux")]
                SupervisedTrigger::LeaderExit => self.saw_retained_exit,
                #[cfg(target_os = "linux")]
                SupervisedTrigger::Ready | SupervisedTrigger::StderrOverflow => true,
                SupervisedTrigger::Bound | SupervisedTrigger::Never => true,
            }
        }
    }
}

fn supervised_run(mode: &str, containment: Containment, trigger: SupervisedTrigger) -> real::Outcome<Process> {
    supervised_run_with_grace(mode, containment, trigger, Duration::from_millis(20))
}

fn supervised_run_with_grace(
    mode: &str,
    containment: Containment,
    trigger: SupervisedTrigger,
    grace: Duration,
) -> real::Outcome<Process> {
    use skein_world::end_to_end::Expectation;
    let binary = Binary::start_contained(fixture(mode), Mode::Pipes, 4096, Expectation::Supervise, containment)
        .expect("a supervised fixture starts");
    let judge = SupervisedJudge {
        trigger,
        grace,
        term_at: None,
        kill_at: None,
        next: None,
        saw_retained_exit: false,
        saw_grace: false,
    };
    let mut world = real::World::new(judge);
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    world.run(&Clock::new(), WORLD_PATIENCE)
}

// The detached descendant is retained by Linux's tree keeper.
#[cfg(target_os = "linux")]
#[test]
fn a_retained_exit_observation_records_grace_before_the_next_act() {
    struct Observed {
        judge: SupervisedJudge,
        checked: bool,
    }
    impl Referee<Process> for Observed {
        fn act(&mut self, now: Time, procs: &mut [Process]) {
            self.judge.act(now, procs);
        }
        fn observe(&mut self, now: Time, procs: &[Process]) {
            let Process::Child(child) = &procs[0] else { panic!("binary observer") };
            if !self.checked && child.exit_status().is_some() && !child.tree_status().live_pids.is_empty() {
                // Sample the retained exit at the start of a grace, with no subsequent act.
                let mut probe = SupervisedJudge {
                    trigger: SupervisedTrigger::LeaderExit,
                    grace: Duration::from_millis(20),
                    term_at: Some(now),
                    kill_at: None,
                    next: None,
                    saw_retained_exit: false,
                    saw_grace: false,
                };
                probe.observe(now, procs);
                assert!(probe.saw_retained_exit, "observe records the exit while its keeper is retained");
                assert!(probe.saw_grace, "observe records the live un-killed tree inside the grace");
                self.checked = true;
            }
            self.judge.observe(now, procs);
        }
        fn next_deadline(&self) -> Option<Time> {
            self.judge.next_deadline()
        }
        fn overdue(&self, now: Time) -> Option<String> {
            self.judge.overdue(now)
        }
        fn passed(&self) -> bool {
            self.checked && self.judge.passed()
        }
    }
    let judge = Observed {
        judge: SupervisedJudge {
            trigger: SupervisedTrigger::LeaderExit,
            grace: Duration::from_millis(20),
            term_at: None,
            kill_at: None,
            next: None,
            saw_retained_exit: false,
            saw_grace: false,
        },
        checked: false,
    };
    let binary = Binary::start_contained(
        fixture("escape-exit"),
        Mode::Pipes,
        4096,
        skein_world::end_to_end::Expectation::Supervise,
        Containment::Walk,
    )
    .expect("a supervised fixture starts");
    let mut world = real::World::new(judge);
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert!(tree_child(&outcome).counts().is_some(), "the retained tree drains");
}

#[test]
fn a_supervised_natural_tree_settles_without_cleanup() {
    use skein_world::end_to_end::Cleanup;
    for containment in ALL {
        // A group cannot see escaped descendants (examples.md, 6; shell.md, 9).
        let modes: &[&str] =
            if containment == Containment::Group { &["exit"] } else { &["exit", "escape-zombie-exit"] };
        for mode in modes {
            let outcome = supervised_run(mode, containment, SupervisedTrigger::Never);
            let child = tree_child(&outcome);
            assert_tree_counted(child, containment);
            assert_eq!(child.tree_status().cleanup(), Cleanup::None);
            assert!(child.tree_status().live_pids.is_empty());
            assert_eq!(child.tree_status().live_at_leader_exit, Some(Vec::new()));
        }
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_supervised_tree_retains_detached_descendants_after_natural_leader_exit() {
    use skein_world::end_to_end::Cleanup;
    for containment in [Containment::Walk, Containment::Best] {
        let outcome = supervised_run("escape-exit", containment, SupervisedTrigger::LeaderExit);
        let child = tree_child(&outcome);
        let pid = descendant(child.stderr());
        assert_eq!(child.tree_status().live_at_leader_exit, Some(vec![pid]));
        assert_eq!(child.tree_status().cleanup(), Cleanup::Terminate);
        assert!(child.tree_status().terminated_pids.contains(&pid));
        assert!(child.tree_status().killed_pids.is_empty());
        assert_eq!(skein_shell::process_live(pid), Ok(false));
        assert_tree_counted(child, containment);
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn whole_tree_term_reaches_detached_descendants_before_the_leader_exits() {
    use skein_world::end_to_end::Cleanup;
    for containment in [Containment::Walk, Containment::Best] {
        for mode in ["supervise-term", "supervise-grandchild"] {
            let outcome = supervised_run_with_grace(mode, containment, SupervisedTrigger::Ready, WORLD_PATIENCE);
            let child = tree_child(&outcome);
            assert_eq!(child.tree_status().cleanup(), Cleanup::Terminate);
            assert!(child.tree_status().terminated_pids.contains(&descendant(child.stderr())));
            assert!(child.stderr().windows(b"tree-term".len()).any(|bytes| bytes == b"tree-term"));
            assert!(child.tree_status().killed_pids.is_empty());
            if mode == "supervise-grandchild" {
                let grandchild: u32 = std::str::from_utf8(child.stderr())
                    .expect("fixture text")
                    .lines()
                    .find_map(|line| line.strip_prefix("grandchild:")?.parse().ok())
                    .expect("the fixture reports a detached grandchild");
                assert!(child.tree_status().terminated_pids.contains(&grandchild));
                assert_eq!(skein_shell::process_live(grandchild), Ok(false));
            }
            assert_tree_counted(child, containment);
        }
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_supervised_tree_honors_grace_after_leader_exit_then_reports_forced_kill() {
    use skein_world::end_to_end::Cleanup;
    for containment in [Containment::Walk, Containment::Best] {
        let outcome = supervised_run("supervise-ignore", containment, SupervisedTrigger::Ready);
        let child = tree_child(&outcome);
        let pid = descendant(child.stderr());
        assert_eq!(child.tree_status().live_at_leader_exit, Some(vec![pid]));
        assert_eq!(child.tree_status().cleanup(), Cleanup::Kill);
        assert!(child.tree_status().terminated_pids.contains(&pid));
        assert_eq!(child.tree_status().killed_pids, vec![pid]);
        assert_eq!(skein_shell::process_live(pid), Ok(false));
        assert_tree_counted(child, containment);
    }
}

#[test]
fn a_supervised_never_ending_leader_receives_the_referees_term_bound() {
    use skein_world::end_to_end::Cleanup;
    for containment in ALL {
        let outcome = supervised_run("never", containment, SupervisedTrigger::Bound);
        let child = tree_child(&outcome);
        assert_eq!(child.exit_status(), Some(Exit::Signal(15)));
        assert_eq!(child.tree_status().cleanup(), Cleanup::Terminate);
        assert_tree_counted(child, containment);
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn supervised_tree_signals_leave_an_existing_unrelated_child_running() {
    for containment in [Containment::Walk, Containment::Best] {
        let mut unrelated = std::process::Command::new("sleep").arg("60").spawn().expect("an unrelated child starts");
        let outcome = supervised_run("supervise-ignore", containment, SupervisedTrigger::Ready);
        assert!(unrelated.try_wait().expect("inspect the unrelated child").is_none());
        assert!(!tree_child(&outcome).tree_status().terminated_pids.contains(&unrelated.id()));
        assert!(!tree_child(&outcome).tree_status().killed_pids.contains(&unrelated.id()));
        unrelated.kill().expect("the unrelated owner ends its child");
        unrelated.wait().expect("the unrelated owner reaps its child");
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn supervised_tree_signals_stay_inside_their_own_delegated_cgroup() {
    use skein_world::end_to_end::{Expectation, Method};
    let first = Binary::start_with_tree(fixture("supervise-ignore"), Mode::Pipes, 4096, Expectation::Supervise, false)
        .expect("the first supervised observer starts");
    let second = Binary::start_with_tree(fixture("never"), Mode::Pipes, 4096, Expectation::Supervise, false);
    let Ok(mut second) = second else {
        drop(first); // A walk deliberately refuses a concurrent observer.
        return;
    };
    let pidfd = second.descriptors()[0];
    let mut world = real::World::new(SupervisedJudge {
        trigger: SupervisedTrigger::Ready,
        grace: Duration::from_millis(20),
        term_at: None,
        kill_at: None,
        next: None,
        saw_retained_exit: false,
        saw_grace: false,
    });
    world.spawn_with_fds(first.descriptors(), || Process::Child(first));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(tree_child(&outcome).counts().expect("the first tree settled").method, Method::Cgroup);
    assert_eq!(skein_shell::poll_child(pidfd, false).expect("the second leader remains owned"), None);
    second.signal_tree(Signal::Terminate);
    let mut world = real::World::new(TreeJudge { end: false, sent: false });
    world.spawn_with_fds(second.descriptors(), || Process::Child(second));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(tree_child(&outcome).exit_status(), Some(Exit::Signal(15)));
    assert_eq!(tree_child(&outcome).counts().expect("the second tree settled").method, Method::Cgroup);
}

#[test]
fn a_supervised_natural_stream_drains_before_terminal_counts() {
    use skein_world::end_to_end::{Cleanup, Expectation};
    let mut binary = Binary::start_with_tree(
        command("printf answer; printf diagnostic >&2"),
        Mode::Pipes,
        4096,
        Expectation::Supervise,
        false,
    )
    .expect("a supervised stream fixture starts");
    let streams = binary.take_streams().expect("the person's streams");
    let mut world = real::World::new(Judge { signal: false, sent: false, act_at: None });
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    world.spawn_with_fds(streams.descriptors(), || {
        Process::Person(Script::pipes(streams.descriptors(), &[Act::Close(0), Act::Read(0), Act::Read(0)]))
    });
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    let (child, person) = observations(&outcome);
    assert_eq!(person.received, b"answer");
    assert_eq!(child.stderr(), b"diagnostic");
    assert_eq!(child.tree_status().cleanup(), Cleanup::None);
    assert_tree_counted(child, Containment::Best);
}

fn capture_run(command: Command, limit: usize, trigger: SupervisedTrigger, force_walk: bool) -> real::Outcome<Process> {
    use skein_world::end_to_end::Expectation;

    let binary = Binary::start_with_tree(command, Mode::Pipes, limit, Expectation::Supervise, force_walk)
        .expect("a supervised capture starts");
    let mut world = real::World::new(SupervisedJudge {
        trigger,
        grace: Duration::from_millis(20),
        term_at: None,
        kill_at: None,
        next: None,
        saw_retained_exit: false,
        saw_grace: false,
    });
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    world.run(&Clock::new(), Duration::from_secs(2))
}

#[test]
fn supervised_stderr_overflow_keeps_a_bounded_prefix_and_drains_a_natural_tree() {
    use skein_world::end_to_end::Cleanup;

    for &force_walk in WALKS {
        let outcome = capture_run(
            command("printf 'prefix:' >&2; head -c 262144 /dev/zero >&2; exit 17"),
            8,
            SupervisedTrigger::Never,
            force_walk,
        );
        let child = tree_child(&outcome);
        assert_eq!(child.stderr(), b"prefix:\0");
        assert!(child.stderr_overflowed());
        assert_eq!(child.exit_status(), Some(Exit::Code(17)));
        assert_eq!(child.tree_status().cleanup(), Cleanup::None);
        assert!(child.counts().is_some(), "draining never leaves the writer blocked at its pipe cap");
        assert!(child.is_empty());
    }
}

// Under the walk on Linux, the group on macOS (`WALKS`).
#[test]
fn supervised_stderr_limits_distinguish_exact_capture_empty_capture_and_overflow() {
    for (bytes, limit, prefix, overflow) in
        [("", 0, "", false), ("12345678", 8, "12345678", false), ("123456789", 8, "12345678", true), ("x", 0, "", true)]
    {
        let outcome = capture_run(
            command(&format!("printf '{bytes}' >&2")),
            limit,
            SupervisedTrigger::Never,
            cfg!(target_os = "linux"),
        );
        let child = tree_child(&outcome);
        assert_eq!(child.stderr(), prefix.as_bytes());
        assert_eq!(child.stderr_overflowed(), overflow);
        assert!(child.counts().is_some());
    }
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn supervised_stderr_overflow_is_observed_before_the_referees_term_grace_and_whole_tree_kill() {
    use skein_world::end_to_end::Cleanup;

    for force_walk in [true, false] {
        let outcome = capture_run(fixture("supervise-ignore"), 8, SupervisedTrigger::StderrOverflow, force_walk);
        let child = tree_child(&outcome);
        assert_eq!(child.stderr().len(), 8);
        assert!(child.stderr_overflowed());
        assert_eq!(child.exit_status(), Some(Exit::Signal(15)));
        assert_eq!(child.tree_status().cleanup(), Cleanup::Kill);
        assert!(child.tree_status().forced_cleanup);
        assert!(!child.tree_status().killed_pids.is_empty());
        assert!(child.counts().is_some(), "the keeper and all descendants genuinely settled");
        assert!(child.is_empty());
    }
}

#[test]
#[should_panic(expected = "binary stderr exceeds its scenario limit of 8 bytes")]
fn legacy_stderr_capture_still_fails_the_scenario_at_overflow() {
    let binary = Binary::start(command("printf '123456789' >&2"), Mode::Pipes, 8).expect("a legacy capture starts");
    let mut world = real::World::new(Judge { signal: false, sent: false, act_at: None });
    world.spawn_with_fds(binary.descriptors(), || Process::Child(binary));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert!(outcome.procs.iter().all(Host::is_empty));
}

fn terminal_fixture() -> Command {
    Command {
        program: skein_scratch::paths::binary("world_terminal_fixture"),
        arguments: vec![],
        environment: vec![],
        directory: "/tmp".into(),
    }
}

#[test]
fn input_is_read_a_key_at_a_time() {
    let outcome = run(
        terminal_fixture(),
        Mode::Terminal,
        &[Act::AwaitStderr(b"ready\n"), Act::Write(0, b"aq"), Act::Read(0), Act::Read(0)],
        false,
    );
    let (child, _) = observations(&outcome);
    assert_eq!(child.stderr(), b"ready\nkey a\nkey q\n");
    assert_eq!(child.exit_status(), Some(Exit::Code(0)));
}

#[test]
fn the_modes_are_found_again_once_the_binary_exits() {
    let outcome = run(
        terminal_fixture(),
        Mode::Terminal,
        &[Act::AwaitStderr(b"ready\n"), Act::Modes(0), Act::Write(0, b"q"), Act::Read(0), Act::Read(0), Act::Modes(0)],
        false,
    );
    let (child, person) = observations(&outcome);
    assert_eq!(child.exit_status(), Some(Exit::Code(0)));
    assert_eq!(
        person.modes,
        [
            skein_shell::Modes { echo: false, lines: false, signals: false },
            skein_shell::Modes { echo: true, lines: true, signals: true }
        ]
    );
}

#[test]
fn a_size_change_is_heard() {
    let outcome = run(
        terminal_fixture(),
        Mode::Terminal,
        &[
            Act::AwaitStderr(b"ready\n"),
            Act::Resize(0, skein_io::kernel::Size { rows: 30, columns: 100 }),
            Act::AwaitStderr(b"size 30x100\n"),
            Act::Write(0, b"q"),
            Act::Read(0),
            Act::Read(0),
        ],
        false,
    );
    let (child, _) = observations(&outcome);
    assert!(child.stderr().windows(b"size 30x100\n".len()).any(|line| line == b"size 30x100\n"));
    assert_eq!(child.exit_status(), Some(Exit::Code(0)));
}

// A cgroup, the subreaper and `/proc` are Linux's (examples.md, 6; shell.md, 9).
#[cfg(target_os = "linux")]
#[test]
fn a_group_refuses_beside_a_subreaper() {
    use skein_world::end_to_end::Expectation;
    let previous = skein_shell::subreaper().expect("the test's prior adoption policy");
    skein_shell::set_subreaper(true).expect("the test adopts orphaned descendants");
    let result = Binary::start_contained(fixture("exit"), Mode::Pipes, 4096, Expectation::Measure, Containment::Group);
    skein_shell::set_subreaper(previous).expect("the test restores its prior adoption policy");
    assert!(matches!(result, Err(StartError::Tree(skein_io::kernel::Error::Other(16)))));
}
