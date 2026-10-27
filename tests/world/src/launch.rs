//! Descriptor-relative hosted launch stories shared by both backends.
use crate::hosted::{Act, Script};
use skein_fake_machine::{How, Item, Machine as Files, Opened, Refusal};
use skein_io::kernel::{Error, Fd, OpenHow, Spawn};
use skein_lib::Queue;
use skein_sim::{Answer, Call, Handle};
use skein_world::{HostedProgram, Inherited, Machine, StartupRoot};

pub const CONTENT: &[u8] = b"from launch directory";
/// Two pipes, two startup roots, a signal and a launch directory.
pub const CHILD_DESCRIPTORS: u32 = 6;

#[must_use]
pub fn program() -> HostedProgram<Script> {
    HostedProgram { program: Box::from(&b"hosted"[..]), make: child, instances: 1, operations: 8 }
}

fn child(_spawn: &Spawn, inherited: &Inherited) -> Script {
    assert!(inherited.dir.is_some(), "a hosted spawn opens its launch directory");
    Script::child(
        inherited,
        &[Act::Read(0), Act::OpenLaunch(b"note", OpenHow::Create { mode: Some(0o600) }), Act::WriteFile(2, CONTENT)],
    )
}

/// The parent opens a root distinct from the scenario's namespace root.
#[must_use]
pub fn parent(root: Fd, namespace: &[u8], dir: &[u8], exchange: bool) -> Script {
    let mut acts = vec![Act::OpenLaunch(b"run", OpenHow::Directory), Act::Spawn];
    if exchange {
        acts.extend([Act::CloseLaunch, Act::Write(0, b"go"), Act::Read(1), Act::Wait]);
    }
    let mut script = Script::parent(root, &acts);
    script.args = Box::new([Box::from(namespace)]);
    script.dir = Box::from(dir);
    script
}

#[must_use]
pub fn roots(spawn: &Spawn) -> Vec<StartupRoot> {
    vec![
        StartupRoot { name: Box::from(&b"namespace"[..]), path: spawn.args[0].clone() },
        StartupRoot { name: Box::from(&b"other"[..]), path: spawn.args[0].clone() },
    ]
}

#[derive(Debug)]
pub struct LaunchFiles {
    files: Files,
    base: Opened,
}

impl Default for LaunchFiles {
    fn default() -> Self {
        let mut files = Files::new();
        let base = files.lay(&[Item::directory(b"run"), Item::directory(b"run/work"), Item::directory(b"work")]);
        Self { files, base }
    }
}

impl LaunchFiles {
    pub fn parent_root(&mut self) -> Handle {
        Handle::new(self.files.open(self.base, b".", How::Directory).expect("parent root").raw())
    }

    /// Reads where the child should have written and checks plausible wrong levels.
    pub fn check_file(&mut self) {
        let file = self.files.open(self.base, b"run/work/note", How::Read).expect("child output");
        // The read room is exactly this story's output length.
        let room = u32::try_from(CONTENT.len()).expect("short fixture");
        assert_eq!(self.files.read(file, 0, room).expect("read child output"), CONTENT);
        self.files.close(file);
        for path in [b"note".as_slice(), b"run/note", b"work/note"] {
            assert_eq!(self.files.open(self.base, path, How::Read), Err(Refusal::NotFound), "{path:?}");
        }
    }

    pub fn finish(&mut self) {
        self.files.close(self.base);
        assert_eq!(self.files.open_handles(), 0, "all launch, startup, parent and file handles closed");
    }
}

impl Machine for LaunchFiles {
    fn open_root(&mut self, path: &[u8]) -> Result<Handle, Error> {
        self.files
            .open(self.base, path, How::Directory)
            .map(|fd| Handle::new(fd.raw()))
            .map_err(skein_fake_machine::kernel_error)
    }

    fn open_beneath(&mut self, root: Handle, path: &[u8]) -> Option<Result<Handle, Error>> {
        Some(
            self.files
                .open(Opened::new(root.raw()), path, How::Directory)
                .map(|fd| Handle::new(fd.raw()))
                .map_err(skein_fake_machine::kernel_error),
        )
    }

    fn close_root(&mut self, root: Handle) {
        self.files.close(Opened::new(root.raw()));
    }

    fn step(&mut self, call: Call, answers: &mut Queue<Answer>) {
        skein_fake_machine::step(&mut self.files, call, answers);
    }
}
