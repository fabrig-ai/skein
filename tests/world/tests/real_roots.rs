//! The same startup-root factories over a real scratch directory and one ring.

use std::os::unix::ffi::OsStrExt;

use skein_io::kernel::{Error, Exit};
use skein_lib::Duration;
use skein_scratch::Scratch;
use skein_shell::{Clock, open_root};
use skein_world::{HostedProgram, real};
use skein_world_tests::hosted::{Act, Judge, Script};
use skein_world_tests::roots::{file_child, missing_roots, parent, roots, waiting_child};

#[cfg(target_os = "linux")]
const DESCRIPTORS: &str = "/proc/self/fd";
#[cfg(target_os = "macos")]
const DESCRIPTORS: &str = "/dev/fd";

#[test]
fn child_files_survive_parent_root_close_in_a_real_scratch_directory() {
    let scratch = Scratch::new("hosted-roots");
    let descriptors = descriptor_count();
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_roots(
        HostedProgram { program: Box::from(&b"hosted"[..]), make: file_child, instances: 1, operations: 8 },
        roots,
    );
    world.spawn_with_fds(vec![root], || parent(root, scratch.path().as_os_str().as_bytes(), false));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_scratch_closed(descriptors);
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
    assert_eq!(outcome.procs[1].received, b"gofrom child");
    assert_eq!(std::fs::read(scratch.path().join("note")).expect("child output file"), b"from child");
}

#[test]
fn kill_closes_real_child_roots_and_settles_the_parent_wait() {
    let scratch = Scratch::new("killed-roots");
    let descriptors = descriptor_count();
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_roots(
        HostedProgram { program: Box::from(&b"hosted"[..]), make: waiting_child, instances: 1, operations: 8 },
        roots,
    );
    world.spawn_with_fds(vec![root], || parent(root, scratch.path().as_os_str().as_bytes(), true));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_scratch_closed(descriptors);
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Signal(9)));
    assert_eq!(outcome.killed.len(), 1);
}

#[test]
fn partial_real_startup_failure_closes_roots_without_calling_the_factory() {
    let scratch = Scratch::new("failed-roots");
    let descriptors = descriptor_count();
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_roots(
        HostedProgram { program: Box::from(&b"hosted"[..]), make: file_child, instances: 1, operations: 8 },
        missing_roots,
    );
    world.spawn_with_fds(vec![root], || {
        let mut script = Script::parent(root, &[Act::Spawn]);
        script.args = Box::new([Box::from(scratch.path().as_os_str().as_bytes())]);
        script
    });
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_scratch_closed(descriptors);
    assert_eq!(outcome.procs.len(), 1);
    assert!(outcome.procs[0].results.contains(&Err(Error::NotFound)));
}

fn descriptor_count() -> usize {
    std::fs::read_dir(DESCRIPTORS).expect("descriptor observations").count()
}

fn assert_scratch_closed(before: usize) {
    // The world releases its kernel before returning and the settled hosts
    // hold no descriptors: counting checks that no descriptor at all is kept,
    // including one for a scratch entry that has already been removed.
    assert_eq!(descriptor_count(), before, "no parent, child, rollback or kernel descriptor remains open");
}

#[test]
fn child_append_files_preserve_existing_bytes_and_close_after_parent_root_close() {
    use skein_world_tests::roots::{append_child, appends};
    let scratch = Scratch::new("hosted-appends");
    let descriptors = descriptor_count();
    std::fs::write(scratch.path().join("trace"), b"prefix:").expect("existing trace");
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_startup(
        HostedProgram { program: Box::from(&b"hosted"[..]), make: append_child, instances: 1, operations: 8 },
        roots,
        appends,
    );
    world.spawn_with_fds(vec![root], || parent(root, scratch.path().as_os_str().as_bytes(), false));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(outcome.procs[0].received, b"ready");
    assert_eq!(std::fs::read(scratch.path().join("trace")).expect("trace"), b"prefix:onetwo");
    assert_eq!(std::fs::read(scratch.path().join("side")).expect("side file"), b"side");
    assert_scratch_closed(descriptors);
}

#[test]
fn a_real_later_append_open_failure_rolls_back_every_file_and_root() {
    use skein_world_tests::roots::{append_child, missing_appends};
    let scratch = Scratch::new("failed-hosted-appends");
    let descriptors = descriptor_count();
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_startup(
        HostedProgram { program: Box::from(&b"hosted"[..]), make: append_child, instances: 1, operations: 8 },
        roots,
        missing_appends,
    );
    world.spawn_with_fds(vec![root], || {
        let mut script = Script::parent(root, &[Act::Spawn]);
        script.args = Box::new([Box::from(scratch.path().as_os_str().as_bytes())]);
        script
    });
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(outcome.procs.len(), 1);
    assert!(outcome.procs[0].results.contains(&Err(Error::NotFound)));
    assert_scratch_closed(descriptors);
}

#[test]
fn a_killed_real_host_releases_its_startup_append_descriptors() {
    use skein_world_tests::roots::appends;
    let scratch = Scratch::new("killed-hosted-appends");
    let descriptors = descriptor_count();
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_startup(
        HostedProgram { program: Box::from(&b"hosted"[..]), make: waiting_child, instances: 1, operations: 8 },
        roots,
        appends,
    );
    world.spawn_with_fds(vec![root], || parent(root, scratch.path().as_os_str().as_bytes(), true));
    let outcome = world.run(&Clock::new(), Duration::from_secs(2));
    assert_eq!(outcome.killed.len(), 1);
    assert_scratch_closed(descriptors);
}

fn launch_directory(dir: &[u8], exchange: bool) -> (real::Outcome<Script>, Scratch) {
    use skein_world_tests::launch;
    let scratch = Scratch::new("launch-directory");
    std::fs::create_dir_all(scratch.path().join("run/work")).expect("launch tree");
    std::fs::create_dir(scratch.path().join("work")).expect("other directory");
    let root = open_root(scratch.path()).expect("parent root");
    let mut world = real::World::new(Judge);
    world.host_roots(launch::program(), launch::roots);
    world.spawn_with_fds(vec![root], || launch::parent(root, scratch.path().as_os_str().as_bytes(), dir, exchange));
    (world.run(&Clock::new(), Duration::from_secs(2)), scratch)
}

#[test]
fn a_hosted_child_inherits_its_launch_directory() {
    let descriptors = descriptor_count();
    let (outcome, scratch) = launch_directory(b"work", true);
    assert_eq!(outcome.procs[0].child_exit, Some(Exit::Code(0)));
    assert_eq!(
        std::fs::read(scratch.path().join("run/work/note")).expect("child output"),
        skein_world_tests::launch::CONTENT
    );
    for path in ["note", "run/note", "work/note"] {
        assert!(!scratch.path().join(path).exists(), "no file at {path}");
    }
    assert_scratch_closed(descriptors);
}

#[test]
fn a_launch_directory_beneath_nothing_refuses_the_spawn() {
    for (dir, error) in [(b"missing".as_slice(), Error::NotFound), (b"..", Error::Escape), (b"/", Error::Escape)] {
        let descriptors = descriptor_count();
        let (outcome, _scratch) = launch_directory(dir, false);
        assert_eq!(outcome.procs.len(), 1, "{dir:?}: no child admitted");
        assert!(outcome.procs[0].results.contains(&Err(error)), "{dir:?}: the open's error");
        assert_scratch_closed(descriptors);
    }
}
