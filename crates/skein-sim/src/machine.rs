//! The machine seam (simulator.md, 3): operations on files and program
//! selection passed to the embedder's fake machine, and its answers. Both
//! are data. The world owns the
//! simulator and the machine, takes the calls from [`Sim::calls`], hands
//! each to its machine, and gives the answers back with [`Sim::answer`],
//! as it moves records between its processes and the simulator: nothing
//! calls into the machine, and nothing in the machine calls back.
//!
//! The simulator keeps what is the kernel's: descriptors, which it maps to
//! the machine's handles; the checks of the contract; and the faults it
//! draws, before a call or instead of one (a short read asks the machine
//! for fewer bytes; a failure asks it nothing). The machine keeps what is
//! the filesystem's: what lies beneath its roots, and what each handle has
//! open. It answers every call, exactly as asked, once.
//!
//! [`Sim::calls`]: crate::Sim::calls
//! [`Sim::answer`]: crate::Sim::answer

use alloc::boxed::Box;
use alloc::vec::Vec;

use skein_io::kernel::{Error, Kind, OpenHow, Pipe, Stat};

/// The machine's name for a file or a directory it opened, which the
/// simulator holds behind a process's descriptor. Only the machine gives
/// it meaning, and it issues each once.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Handle(u64);

impl Handle {
    #[must_use]
    pub const fn new(raw: u64) -> Handle {
        Handle(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// The simulator's name for one call, which the answer echoes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Ticket(pub(crate) u64);

/// An operation on files, for the machine to answer.
#[derive(PartialEq, Eq, Debug)]
pub struct Call {
    pub ticket: Ticket,
    pub ask: Ask,
}

/// What the machine is asked, in its handles and with the bytes it needs.
/// Each is answered with the [`Reply`] it names, or an error the
/// operation may answer (`skein_io::kernel`'s table).
#[derive(PartialEq, Eq)]
pub enum Ask {
    /// An exclusive lock through this handle, or Busy while another holds it.
    Lock { file: Handle },
    /// Identify a program beneath `root`, in `dir`, and start its modeled
    /// behavior with exactly the requested child descriptors.
    Spawn {
        root: Handle,
        program: Box<[u8]>,
        args: Box<[Box<[u8]>]>,
        env: Box<[Box<[u8]>]>,
        dir: Box<[u8]>,
        pipes: Box<[Pipe]>,
    },
    /// `path`, resolved beneath the directory `root` names as `openat2`
    /// resolves with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS`, opened as
    /// `how` says: [`Reply::Opened`], a handle never issued before.
    Open { root: Handle, path: Box<[u8]>, how: OpenHow },
    /// The file's bytes from `at`, `len` of them or as many as there are:
    /// [`Reply::Read`], empty at or past its end.
    Read { file: Handle, at: u64, len: u32 },
    /// `bytes`, every one, written at `at`: [`Reply::Done`].
    Write { file: Handle, at: u64, bytes: Box<[u8]> },
    /// `bytes` written whole at the file's end when this call is delivered: [`Reply::Done`].
    Append { file: Handle, bytes: Box<[u8]> },
    /// Sets an existing writable file's length: [`Reply::Done`].
    Truncate { file: Handle, length: u64 },
    /// [`Reply::Done`].
    Sync { file: Handle },
    /// [`Reply::Stat`].
    Stat { file: Handle },
    /// Moves the entry atomically; with `no_replace`, a taken target
    /// answers `Exists`, otherwise it replaces it: [`Reply::Done`].
    Rename { from_dir: Handle, from: Box<[u8]>, to_dir: Handle, to: Box<[u8]>, no_replace: bool },
    /// [`Reply::Done`].
    Remove { dir: Handle, name: Box<[u8]>, directory: bool },
    /// [`Reply::Done`].
    MakeDirectory { dir: Handle, name: Box<[u8]>, mode: u32 },
    /// The next entries of the directory, from where the last `List` of
    /// this handle stopped: at most `most`, their names `room` bytes
    /// together at most, and at least one while any is left. `room` is at
    /// least the longest name: [`Reply::Listed`].
    List { dir: Handle, most: u32, room: u32 },
    /// The handle is closed, and forgotten: [`Reply::Done`].
    Close { file: Handle },
}

impl core::fmt::Debug for Ask {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Lock { file } => f.debug_struct("Lock").field("file", file).finish(),
            Self::Spawn { root, program, args, env, dir, pipes } => f
                .debug_struct("Spawn")
                .field("root", root)
                .field("program", program)
                .field("args", args)
                .field("env", &EnvNames(env))
                .field("dir", dir)
                .field("pipes", pipes)
                .finish(),
            Self::Open { root, path, how } => {
                f.debug_struct("Open").field("root", root).field("path", path).field("how", how).finish()
            }
            Self::Read { file, at, len } => {
                f.debug_struct("Read").field("file", file).field("at", at).field("len", len).finish()
            }
            Self::Write { file, at, bytes } => {
                f.debug_struct("Write").field("file", file).field("at", at).field("bytes", bytes).finish()
            }
            Self::Append { file, bytes } => f.debug_struct("Append").field("file", file).field("bytes", bytes).finish(),
            Self::Truncate { file, length } => {
                f.debug_struct("Truncate").field("file", file).field("length", length).finish()
            }
            Self::Sync { file } => f.debug_struct("Sync").field("file", file).finish(),
            Self::Stat { file } => f.debug_struct("Stat").field("file", file).finish(),
            Self::Rename { from_dir, from, to_dir, to, no_replace } => f
                .debug_struct("Rename")
                .field("from_dir", from_dir)
                .field("from", from)
                .field("to_dir", to_dir)
                .field("to", to)
                .field("no_replace", no_replace)
                .finish(),
            Self::Remove { dir, name, directory } => {
                f.debug_struct("Remove").field("dir", dir).field("name", name).field("directory", directory).finish()
            }
            Self::MakeDirectory { dir, name, mode } => {
                f.debug_struct("MakeDirectory").field("dir", dir).field("name", name).field("mode", mode).finish()
            }
            Self::List { dir, most, room } => {
                f.debug_struct("List").field("dir", dir).field("most", most).field("room", room).finish()
            }
            Self::Close { file } => f.debug_struct("Close").field("file", file).finish(),
        }
    }
}

struct EnvNames<'a>(&'a [Box<[u8]>]);

impl core::fmt::Debug for EnvNames<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut list = f.debug_list();
        for entry in self.0 {
            let end = skein_lib::bytes::find(entry, b"=").unwrap_or(entry.len());
            let name = entry.get(..end).expect("name ends within entry");
            list.entry(&name);
        }
        list.finish()
    }
}

/// The machine's answer to the call its ticket names.
#[derive(PartialEq, Eq, Debug)]
pub struct Answer {
    pub ticket: Ticket,
    pub result: Result<Reply, Error>,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Reply {
    Program(Program),
    Opened(Handle),
    Read(Box<[u8]>),
    Stat(Stat),
    /// Each entry's kind and name, in the order to hand them back.
    Listed(Vec<(Kind, Box<[u8]>)>),
    Done,
}

/// A fake program's behavior. The machine selects it; the simulator runs
/// it against the child's pipes and keeps the process lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Program {
    /// Copy bytes from one child descriptor to another until input ends.
    Echo { input: u32, output: u32 },
    /// Exit at startup with the given code.
    Exit(u8),
    /// Stay alive until signalled.
    Never,
    /// Start a child that holds the output pipes in the leader's group; optionally exit the leader.
    Fork { exit_leader: bool },
    /// Run a service hosted by the world in a process bound to these pipes.
    Service,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_call_debug_prints_env_names_without_values() {
        let call = Call {
            ticket: Ticket(1),
            ask: Ask::Spawn {
                root: Handle::new(1),
                program: Box::from(&b"git"[..]),
                args: Box::new([]),
                env: Box::new([Box::from(&b"NAME=secret-value"[..]), Box::from(&b"NO_EQUALS"[..])]),
                dir: Box::from(&b"."[..]),
                pipes: Box::new([]),
            },
        };
        let debug = alloc::format!("{call:?}");
        assert!(debug.contains(&alloc::format!("{:?}", b"NAME".as_slice())));
        assert!(debug.contains(&alloc::format!("{:?}", b"NO_EQUALS".as_slice())));
        assert!(!debug.contains("secret-value"));
        assert!(!debug.contains("115, 101, 99, 114, 101, 116"));
    }
}
