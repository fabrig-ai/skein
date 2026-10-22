# The kernel boundary

Provisional, 2026-10-03, revised 2026-10-19. The design of the boundary
below io: the records io submits to a backend, the completions the
backend hands back, and what every backend promises. The records live in
`skein-io`'s `kernel` module, whose module documentation states each rule
of the contract exactly; this document is the design behind it.

## 1. In one page

- **Records, not calls.** An operation goes down, and its completion
  comes up, handing the operation back. This is all the ring backend, the
  simulator and any later backend share, and all they implement.
- **Memory moves with the operation.** A buffer the kernel reads or
  writes is a `Box` inside the record, held by the backend until the
  completion hands it back.
- **Plain values only.** Kernel structures and error numbers stay in the
  backend; io sees addresses, counts and a skein error enum, and needs no
  `libc`.
- **Every operation completes exactly once,** cancelled or not, and a
  cancel is an operation of its own.
- **Sockets behave as Linux's do,** and the simulator matches them; one
  conformance suite holds every backend to the contract.
- **Files stay beneath a root.** A root is an open directory. Only `Open`
  takes a path, which the kernel resolves beneath its root; every other
  operation on a name acts on one entry of an open directory.
- **A tree's metadata, and its filesystem.** Links, modes, owners and
  extended attributes are read and written by name in an open directory,
  never through a link (section 6.4); a filesystem's status, its sync and
  btrfs's subvolumes take a descriptor, the subvolumes through a closed
  list of btrfs's ioctls (section 6.5).
- **Cgroups, and a helper for contained trees.** A cgroup is a directory
  and its events file, made, stopped and proved empty through records
  that name its descriptor; a spawn that asks for other ids or
  containment starts the service's own executable as a helper, which
  sets the tree up and reports how far it got (section 6.6).

## 2. In skein

```
                    io (step code)
           Submit ↓              ↑ Complete
   ┌───────────────┬────────────────┬───────────────────────────────┐
   │ ring          │ sim            │ readiness (later)             │
   │ io_uring      │ the simulated  │ wait for readiness, make the  │
   │               │ kernel         │ syscall, complete             │
   │ production    │ tests          │ hosts without io_uring        │
   └───────────────┴────────────────┴───────────────────────────────┘
```

io (io.md) is the only user of the records. The ring and the readiness
backend live in the shell (shell.md); the simulator is simulator.md. The
records are step-code types, in `skein-io`, so that io can name them and
the backends can depend on io.

## 3. The records

```rust
// skein-io's kernel module, a sketch
// io -> kernel
pub struct Submit   { pub op: Token, pub kind: Op }
// kernel -> io: the operation handed back, and what came of it
pub struct Complete { pub op: Token, pub kind: Op, pub result: Result<Done, Error> }

// one success shape per operation
pub enum Done {
    Nothing, Count(u32), Fd(Fd), Accepted { fd: Fd, peer: Addr }, Bound(Addr),
    Stat(Stat),                       // kind, size, mode, owners, links, inode, times (6.1)
    Subvolume(Subvolume), Filesystem(Filesystem),  // btrfs's, and a filesystem's status (6.5)
    Spawned { pidfd: Fd, report: Option<Fd> },  // report: the helper's (6.6)
    Exit(Exit), ServiceSignal(ServiceSignal),
    Cgroup { dir: Fd, events: Fd }, Populated(bool), Counts(Counts),  // cgroups (6.6)
    Usage(Usage), Window(Window),     // the process's own (6.3)
}

pub enum Op {
    // sockets
    Socket   { family: Family },
    Bind     { fd: Fd, addr: Addr },
    Listen   { fd: Fd, backlog: u32 },
    Accept   { fd: Fd },
    Connect  { fd: Fd, addr: Addr },
    Recv     { fd: Fd, buf: Box<[u8]> },
    Send     { fd: Fd, bytes: Box<[u8]>, from: u32 },
    Shutdown { fd: Fd },
    Close    { fd: Fd },
    // files, beneath a root (section 6.1)
    Open          { root: Fd, path: Box<[u8]>, how: OpenHow },   // Read, Directory, Create
    Read          { fd: Fd, buf: Box<[u8]>, at: u64 },
    Write         { fd: Fd, bytes: Box<[u8]>, from: u32, at: u64 },
    Append        { fd: Fd, bytes: Box<[u8]>, from: u32 },       // at the end (6.1)
    Truncate      { fd: Fd, length: u64 },                       // ftruncate (6.1)
    Sync          { fd: Fd },
    Stat          { fd: Fd },
    StatEntry     { dir: Fd, name: Box<[u8]> },                  // the entry itself, never a link's target
    Rename        { from_dir: Fd, from: Box<[u8]>, to_dir: Fd, to: Box<[u8]>, no_replace: bool },
    Remove        { dir: Fd, name: Box<[u8]>, directory: bool },
    MakeDirectory { dir: Fd, name: Box<[u8]>, mode: u32 },     // less the umask (io.md, 5.3)
    List          { fd: Fd, entries: Box<[Entry]>, names: Box<[u8]> },  // synchronous
    Lock          { fd: Fd },                                    // exclusive, without waiting; synchronous
    // a tree's metadata, by name in an open directory (section 6.4)
    ReadLink        { dir: Fd, name: Box<[u8]>, buf: Box<[u8]> },                          // synchronous
    MakeLink        { dir: Fd, name: Box<[u8]>, target: Box<[u8]> },
    HardLink        { from_dir: Fd, from: Box<[u8]>, to_dir: Fd, to: Box<[u8]> },
    SetMode         { dir: Fd, name: Box<[u8]>, mode: u32 },                               // synchronous
    SetOwner        { dir: Fd, name: Box<[u8]>, user: u32, group: u32 },                   // synchronous
    ListAttributes  { dir: Fd, name: Box<[u8]>, names: Box<[u8]> },                        // synchronous
    GetAttribute    { dir: Fd, name: Box<[u8]>, attribute: Box<[u8]>, value: Box<[u8]> },  // synchronous
    SetAttribute    { dir: Fd, name: Box<[u8]>, attribute: Box<[u8]>, value: Box<[u8]> },  // synchronous
    RemoveAttribute { dir: Fd, name: Box<[u8]>, attribute: Box<[u8]> },                    // synchronous
    // the filesystem a descriptor lies on, and btrfs's subvolumes (section 6.5); all synchronous
    FilesystemStatus  { fd: Fd },
    SyncFilesystem    { fd: Fd },
    SubvolumeCreate   { dir: Fd, name: Box<[u8]> },
    Snapshot          { source: Fd, dir: Fd, name: Box<[u8]> },
    SubvolumeDestroy  { dir: Fd, name: Box<[u8]> },
    SubvolumeReadOnly { fd: Fd, read_only: bool },
    SubvolumeInfo     { fd: Fd },
    SubvolumeChildren { fd: Fd, from: u64, children: Box<[Child]> },
    SubvolumePath     { fd: Fd, directory: u64, tree: u64, path: Box<[u8]> },
    // processes (section 6.2)
    Spawn     { spawn: Box<Spawn> },                         // synchronous
    Wait      { pidfd: Fd, reap: bool },
    Signal    { pidfd: Fd, signal: Signal, to: Target },     // synchronous
    PipeRead  { fd: Fd, buf: Box<[u8]> },
    PipeWrite { fd: Fd, bytes: Box<[u8]>, from: u32 },
    // cgroups (section 6.6); all synchronous but CgroupChanged
    MakeCgroup        { parent: Fd, name: Box<[u8]>, setup: CgroupSetup },  // limits, bounds, owner
    EnableControllers { cgroup: Fd, controllers: Controllers },
    EnterCgroup       { cgroup: Fd },                         // the process itself
    CgroupCounts      { cgroup: Fd },
    SignalCgroup      { cgroup: Fd, signal: Signal },         // each process, through a pidfd
    KillCgroup        { cgroup: Fd },
    Populated         { events: Fd },
    CgroupChanged     { events: Fd },                         // the ring's poll
    RemoveCgroup      { cgroup: Fd, name: Box<[u8]> },        // with those beneath, deepest first
    // the process's own (section 6.3)
    ReadSignal { fd: Fd },
    Usage,                                                   // synchronous
    Window     { fd: Fd },                                   // synchronous
    Cancel     { target: Token },
}

// what the records on files answer (6.1, 6.5)
pub struct Stat {
    pub kind: Kind, pub size: u64,
    pub mode: u32,                   // 0o7777 at most; a Create's or a MakeDirectory's, 0o777
    pub owner: u32, pub group: u32, pub links: u32,
    pub inode: u64, pub device: Device, pub special: Device,   // special: what a device is
    pub changed: FileTime, pub modified: FileTime,
}
pub enum Kind { File, Directory, Symlink, CharDevice, BlockDevice, Fifo, Socket, Other }
pub struct Device   { pub major: u32, pub minor: u32 }
pub struct FileTime { pub seconds: i64, pub nanos: u32 }
pub struct Child    { pub tree: u64, pub directory: u64 }
pub struct Subvolume {
    pub tree: u64, pub uuid: [u8; 16], pub parent_uuid: Option<[u8; 16]>,
    pub read_only: bool, pub generation: u64,
}
pub struct Filesystem { pub kind: FilesystemKind, pub size: u64, pub available: u64 }
pub enum FilesystemKind { Btrfs, Other(u64) }   // Other: the kernel's magic number
```

The completion hands back the operation it answers, buffers and all,
beside its result: a `Done` of the one shape that operation succeeds
with, or an `Error`. Every record comes back up, whatever happened. An
error means the operation did nothing usable: a failed `Socket` or
`Accept` made no descriptor.

## 4. Memory and values

- **Memory moves with the operation.** io moves a buffer down inside the
  record, and the backend holds the record until the operation's
  completion, which moves the buffer back up: filled, or with the count
  sent. A `Recv` comes back with its buffer filled up to the count, a
  `Send` with its bytes untouched.
  - While the kernel holds the buffer, no other code can name it: the
    compiler's half of the rule (programming-model.md, 6.2).
  - The backend's `unsafe` is the other half. It takes addresses only
    from records it holds, and gives a record back only after the
    operation's completion. It never drops, copies or replaces a `Box`.
  - **The backend allocates nothing that goes up.** A `Spawn` comes back
    with the parent's end of each requested pipe written into its own
    pipe table, and its completion carries only the child's descriptor.
    What a process owns is then always allocated within its own calls
    (simulator.md, 5).
- **Kernel structures belong to the backend.** A socket address, a
  `statx` buffer, a `siginfo`, an `rusage` and an `open_how` live in the
  backend's in-flight table, beside the record. They are decoded into
  plain values (`Addr`, `Stat`, `Exit`, `Usage`) before they go up. A
  backend for another kernel translates.
- **A path or a name is bytes,** a `Box<[u8]>` in the record with no NUL
  in it. The NUL-terminated string the kernel reads is the backend's own
  copy, held beside the record until its completion.
- **Errors cross as a skein enum:** the errors io handles by name, plus an
  `Other` code. Each backend maps its kernel's error numbers onto it, per
  operation: the same number can mean different things on a cancel and
  on a receive.
- **Socket options are backend defaults, not records,** until a service
  pulls one: every descriptor is close-on-exec, a socket that binds gets
  `SO_REUSEADDR`, an IPv6 socket gets `IPV6_V6ONLY` (families never mix:
  it neither binds nor reaches an IPv4-mapped address), connected and
  accepted sockets get `TCP_NODELAY`, and a send never raises `SIGPIPE`.

## 5. Completion and cancelling

- **Every operation completes exactly once,** cancelled or not, with its
  token. Completions arrive in any order, even on one descriptor.
- **Single-shot operations only, to start:** one submission, one
  completion, and every cancel takes an operation slot of its own. The
  completion queue can then be sized from io's operation slab, so it
  cannot overflow, and every receive and accept is a choice io made while
  it had room.
- **A cancelled operation still completes:** as cancelled, or with what it
  did before the cancel landed. Until then, io keeps the entity
  *settling* (programming-model.md, 5.3).
- **A cancel completes too,** before or after its target:
  - it stopped the target, which then completes cancelled. A stopped
    receive, send or accept took nothing: what had arrived waits for the
    next one. A stopped connect may still have reached its peer, which
    sees the connection end when io closes it;
  - too late: the target had completed or could not be stopped, and
    completes with its own result, or cancelled if the kernel interrupted
    it;
  - not submitted at all, when the backend failed, the target running on.

## 6. What every backend does

- **Sockets behave as Linux's do,** and the simulator matches them:
  - a bind answers with the address bound, its port chosen when it asked
    for port 0; two sockets may bind one address, and the second listen
    fails;
  - a full accept queue delays a connect, never refuses it (refused means
    nothing listened there), until it times out; a failed accept takes no
    waiting connection, and a connection closed or reset while it waits
    is still accepted;
  - a send's count is what the kernel accepted, and a half-close goes out
    behind every completed send; receiving still works after it;
  - a receive of zero bytes means the stream ended, which is not proof of
    a graceful close;
  - a reset fails one operation: a send at once, a receive after the
    bytes already received; after it, receives give zero bytes and sends
    and half-closes fail. An end that already received the peer's end of
    stream hears of no reset, and a send after the peer closed may
    succeed, its bytes lost, until the peer's reset arrives, then fails
    without a reset, the connection closed;
  - a descriptor is closed only by a close. Closing with unread data
    resets the peer; closing a listener resets the connections waiting on
    it.
- **Some operations are synchronous:** spawning, signalling, every
  operation on a cgroup but the wait for its change (6.6), listing a
  directory, locking a file without waiting, reading the process's
  resource usage and its terminal's size, reading a link, setting a mode
  or an owner, the extended attributes, and every operation on the
  filesystem (6.5). None of these is a ring
  operation at the kernel floor (shell.md), and some are not ring
  operations at all. The backend performs them when
  they are submitted and completes them at the next reap, in the same
  records, so io cannot tell the difference.
- **A synchronous operation blocks the loop for its call,** which no
  `Cancel` stops: microseconds on a local filesystem, without bound on
  one that stalls. Some are long even on a healthy disk: btrfs's writing
  ioctls commit a transaction, and `SyncFilesystem` lasts until the
  filesystem's dirty data is written (6.5). A service that must not
  stall makes them in a process of its own, and talks to it as to any
  child.
- **Timers are not operations.** The shell waits for completions with one
  timeout: the earliest deadline over every layer.

### 6.1 Files beneath a root

What the records promise is stated exactly in the kernel module's
documentation, with each operation's errors; the decisions behind it:

- **A root is an open directory:** one the shell opened at startup
  (shell.md, 6), or one an `Open` of a directory made beneath another. A
  root opened beneath a root is a root like any other: nothing it opens
  leaves it, though its parent's other entries lie just above.
- **`Open` resolves beneath its root, and follows the symbolic links that
  stay there.** It is `openat2` with `RESOLVE_BENEATH` and
  `RESOLVE_NO_MAGICLINKS`: `..` above the root, an absolute path, and a
  link leading out of the root fail with `Escape` (`EXDEV`), checked by
  the kernel as it resolves, so no race with a rename gets out. Links that
  stay beneath the root are followed, since a workspace holds them (a
  repository's `CLAUDE.md -> AGENTS.md`); `RESOLVE_NO_SYMLINKS` would
  refuse those too and keep nothing more in. Magic links (`/proc/*/fd/*`)
  are refused by name, with `RESOLVE_NO_MAGICLINKS`, as the man page
  asks: `RESOLVE_BENEATH` refuses them today, but may not always. That
  answers `ELOOP` before the escape is looked at, so a magic link is
  `TooManyLinks`, not `Escape`.
- **A `..` that races is tried again.** `RESOLVE_BENEATH` cannot tell a
  `..` from an escape when a rename or a mount anywhere on the system
  moves under it, and answers `EAGAIN` for the caller to retry. The
  backend does, not io: the ring pushes the same `Open` again from its
  slot, up to 16 times, and only one still racing goes up, as
  `Other(11)`. io_uring's worker meets the race only once a quick attempt
  inline has too, so it is rare, about one `Open` in a thousand under a
  storm of renames; the ring's own tests provoke it with renames in its
  workers beside creates through `..`. The simulator has no race, and
  never answers it.
- **Every other operation on a name acts on one entry of an open
  directory.** `renameat`, `unlinkat`, `mkdirat` and `statx` take no
  `RESOLVE_*` flags, and a path with a `/` in it could leave the root
  through `..` or a link. So `Rename`, `Remove` and `MakeDirectory` take a
  name, never a path (no `/`, not `.` or `..`, which `Op::is_valid`
  holds), and `Stat` takes a descriptor. io opens the directory a name
  lies in first, beneath the root (io.md, 5). None of them follows the
  entry it names: removing a link removes the link.
- **The opens io makes, not `open`'s flags.** `OpenHow` is `Read` (an
  existing file or directory), `Directory` (an existing directory: a
  root, or one to list), `Create` (a new file, exclusively, to write) or
  `Write` (an existing regular file, to read and write, never created),
  each with a form that refuses a symbolic link, as `Read`'s does.
  Each is one well-defined case for the simulator and the suite to hold,
  and more come when a user pulls them.
- **Cutting a file** is `Truncate` on a descriptor opened to write
  (`ftruncate`): the file's length becomes the one given, and what lay
  past it is gone. Like a write, it is durable once the file is synced;
  the simulator's crashing disk keeps the old length past a crash until
  then (simulator.md, 3.1).
- **A path-only open, to stat what cannot be read.** `PathNoFollow` is
  `O_PATH | O_NOFOLLOW`, beneath the root with links refused in every
  parent, as the other no-follow cases. It answers a descriptor for the
  entry itself, a symbolic link included, and needs search permission on
  the directories above it and none on the entry, so it opens another
  user's `0o700` directory or `0o600` file that `Read` cannot (io.md,
  5.3). Only `Stat` and `Close` take it; any other operation on it is an
  invalid record. Its `Stat` may answer a symbolic link or a special
  file (a device, a FIFO, a socket), which otherwise only `List` and
  `StatEntry` show. It does no I/O, so the ring keeps whatever
  it opened, without the `NotAFile` check, and leaves it as it is (a
  path-only descriptor takes no `fcntl` flags). The simulator holds the
  same permission rule: search on the parents, nothing on the entry.
  `StatEntry` (below) states an entry with no open at all.
- **Reads and writes are at an offset,** never at the descriptor's
  position, so concurrent ones need no order. Both may be short: a `Read`
  at the end of the file, and whenever the backend says (POSIX allows it;
  Linux does not cut a regular file's read short but at its end, and the
  simulator draws it); a `Write` likewise, continued by io from where it
  stopped, as a `Send` is. An offset that would pass `i64::MAX` is an
  invalid record: the kernel's offsets are signed, and the ring would read
  `u64::MAX` as the descriptor's position.
- **`Append` is the one write at the descriptor's position.** It is for a
  file written as a stream (io.md, 5.1): a descriptor the shell opened at
  startup to write at its end (`O_WRONLY | O_APPEND`, created with the
  mode its configuration gives), or a standard output that is a regular
  file. The ring writes at the position (offset −1): with `O_APPEND` the
  kernel moves it to the file's end before each write, so each `Append`
  lands at the end, after whatever another process appended meanwhile;
  without it, after the last write. It may be short, and io continues
  from its count, as a `Write`. One is in flight on a descriptor at a
  time, so the pieces of one stream land in order. No `Open` makes such
  a descriptor yet: opening one beneath a root while the service runs
  comes when a user needs it.
- **`Stat` and `StatEntry` answer what an entry is,** from one `statx`:
  its kind; its size; its mode, the permission bits with setuid, setgid
  and sticky (`0o7777` at most); its owner and its group (user and group
  IDs); its number of links; its inode and the device it lies on; the
  device it is, for a device (`special`); and when its metadata and its
  content last changed (`changed`, `modified`), in seconds and
  nanoseconds as the filesystem keeps them, before 1970 included. A value
  its filesystem does not report is zero. `Stat` takes a descriptor.
  `StatEntry` takes a name in an open directory, and states the entry
  itself, never a link's target (`AT_SYMLINK_NOFOLLOW`), nor what an
  automount point would mount (`AT_NO_AUTOMOUNT`): it needs search
  permission on the directory and none on the entry. It is the ring's
  `statx` on the directory and the name. The checks of a private file
  need the owner and the links: one link, and the owner of the directory
  it lies in (io.md, 5.3); a tree's walk needs the rest (6.4).
- **A kind for each thing a name may be:** a file, a directory, a
  symbolic link, a character or a block device, a FIFO, a socket. `Other`
  is left for a `List` entry whose kind neither its directory nor a stat
  of it told, because it went in between.
- **`Rename` is the atomic step.** With `no_replace: false`, it replaces
  its target whole. With `no_replace: true`, it uses `RENAME_NOREPLACE`
  through the ring's `RenameAt.flags` and, once both directories are
  searched and the source is found, fails with `Exists` if any entry names
  the target, including a dangling link, an unreadable file or a directory.
  The check and move are one atomic operation, including for the same
  name or two names of one inode: those fail with `Exists` too.
  A filesystem rejecting the flag with `EINVAL` answers `InvalidArgument`;
  there is no fallback to a replacing rename. With replacement, the
  idiom that replaces a file is: `Stat` the old file for its permission
  bits, `Create` a temporary in the same directory with them, `Write` it,
  `Sync` it, `Close` it, `Rename` it over the old name, `Sync` the
  directory. A reader sees the old file or the new, and after the last
  `Sync` the new one survives a crash. A `Rename` between two
  directories is the same step: the entry leaves one and lands in the
  other whole, both on one filesystem (`Other(EXDEV)` otherwise), and
  it survives a crash once the directory it lands in is synced; until
  then, a crash keeps it in one or the other, never both or neither.
  Its pieces are io's decisions
  (io.md, 5): the temporary's name drawn from the seed, drawn again on
  `Exists`; the old file's bits kept, which is why `Create` takes a mode
  and `Stat` answers one (and why `MakeDirectory` takes one too: a private
  directory is made `0o700`, io.md, 5.3); a symbolic link at the target replaced by the
  file, as `Rename` replaces a link and does not follow it, since writing
  through links is not offered.
- **`List` hands back entries as plain values,** `getdents64`'s structures
  staying in the backend: each entry's kind and its name, packed into the
  record's `names`, at most `entries.len()` of them, `.` and `..` left
  out, from where the last `List` of the descriptor stopped. It is
  synchronous: `getdents64` is not a ring operation. A `names` of at
  least 255 bytes always takes the next entry, so a `List` stops short but
  never makes no progress while entries are left; `Count(0)` is the end.
  A filesystem whose names may be longer than 255 bytes (one storing them
  in another encoding) fails a `List` with `NameTooLong` when its next
  name fits in none of `names`, rather than answer the end; entries a
  `List` took are handed back even if the directory's position could not
  be set back after them.
- **`Lock` takes an exclusive lock without waiting.** It is `flock` with
  `LOCK_EX | LOCK_NB` on a descriptor `Open` made of a file or a
  directory (a root among them), never a path-only one. It completes
  with `Nothing` when the lock is taken, or already held through the
  same open; or fails with the error `Busy` (`EWOULDBLOCK`) when another
  open of the same file holds it, in this process or another. It never waits, so the
  loop never blocks on it: an owner that must wait asks again on a timer
  of its own (io.md, 5). The lock belongs to the open, not the process:
  it goes with the descriptor's close, or with the process's end, and
  never passes to a child, every descriptor being close-on-exec. There
  is no unlock and no shared lock until a user pulls one. `flock` is not
  a ring operation, so `Lock` runs at submit and completes at the next
  reap, as `List` does. A filesystem with no `flock` (some network
  filesystems) fails it with its error, as `Other`.
- **An `Open`, `Read`, `Write`, `Append` or `Sync` may be cancelled.**
  None waits on a peer, but on a filesystem that can stall (NFS whose
  server went away, a FUSE daemon that hangs) one may wait for good, and
  io must be able to give up on it. So a `Cancel` may target one, with a socket's
  outcomes (section 5): it stops one still queued for io_uring's worker,
  or interrupts one running, or comes too late, the target answering its
  own result. Even on tmpfs a `Read` goes to the worker, and the ring's
  conformance run sees its `Cancel` stop it in some runs and come too late
  in others. The ring reads `ECANCELED` and `EINTR` as `Cancelled` on an
  operation a `Cancel` was submitted for. A `Stat`, `StatEntry`,
  `Rename`, `Remove`, `MakeDirectory`, `List` or `Lock` completes without
  one, as does every operation of sections 6.4 and 6.5: a `Cancel` of one
  is a broken invariant, until a filesystem that stalls them pulls it.
- **Only files and directories open.** A FIFO beneath a root opened to
  read opens at once, and its first `Read` then waits for a writer's
  bytes for good, which nothing above can stop. So every `Open` goes down
  with `O_NONBLOCK | O_NOCTTY`, and the ring stats what it opened, at once,
  in its own call, as a `List` makes its own: anything but a regular file
  or a directory (a FIFO, a device) is closed and answered `NotAFile`,
  and a regular file is made blocking again, so that io_uring sends a read
  it cannot do at once to its worker rather than answer `EAGAIN`. A
  socket, or a device with no driver, the kernel itself refuses (`ENXIO`,
  `NotAFile` too). A device's own `open` may still act (a tape rewinds),
  so roots belong on filesystems mounted `nodev`, where the kernel refuses
  every device with `Permission`; the shell's startup says so
  (shell.md, 6).
- **Errors are named per operation,** on a table the module documentation
  keeps and `Complete::is_valid` checks: an operation on files answers its
  own errors, never a socket's or `Cancelled`, and an operation on sockets
  never answers a file's. The same number means different things per
  operation: `EXDEV` is `Escape` on an `Open` and `Other` on a `Rename`
  (two filesystems); `ENOENT` is `NotFound` on a file and `TooLate` on a
  cancel; `EEXIST` is `Exists` on a `Create` or a no-replace `Rename`,
  and `NotEmpty` on a replacing `Rename` over a directory; `EINVAL` is
  `NotALink` on a `ReadLink` and `InvalidArgument` elsewhere; `ENOTTY` is
  `NotSupported` on a btrfs record, off btrfs.
- **Modes are backend defaults,** as socket options are, but for the one
  a `Create` asks for: a new file is `0o666` unless asked otherwise, and a
  new directory `0o777`, less the process's umask, and every descriptor
  is close-on-exec. The fake machine keeps a umask of `0o022`; the suite
  compares a created file's owner bits, which no usual umask takes.

### 6.2 Processes

- **A child is a pidfd from its spawn,** and the leader of a process
  group of its own (io.md, 6). `Spawn` is synchronous: glibc's
  `pidfd_spawn`, with the group set in its attributes, and the cgroup if
  the spawn names one. `Spawn` carries io.md 6.3's fields, each io token
  its descriptor; one that asks for ids or containment starts the helper
  (6.6). Its Debug prints each env entry's name, never its value.
- **`Wait` observes an exit, or reaps it.** Both are the ring's `waitid`
  on the pidfd. A `Wait` that does not reap (`WEXITED | WNOWAIT`)
  completes with the child's exit and leaves it a zombie, which keeps its
  PID, and so its group's ID, from being given to another process. A
  `Wait` that reaps (`WEXITED`) follows one that observed the exit, at
  io's close of the child, and completes at once. Until it is reaped, a
  child's resource usage is not among the process's children's (6.3).
- **`Signal` goes to the child, or to its group,** both through the
  child's pidfd: `pidfd_send_signal`, with no flag for the child, and
  with `PIDFD_SIGNAL_PROCESS_GROUP` for the group the child leads, which
  the kernel floor has. No record carries a PID: the pidfd names the
  child, and through it the group the child leads, so no signal reaches a
  stranger. `Interrupt` sends `SIGINT`, `Terminate` sends `SIGTERM`,
  `Kill` sends `SIGKILL`, `Stop` suspends with `SIGSTOP`, and `Continue`
  resumes with `SIGCONT` (a no-op unless stopped); an interrupt, like a
  termination, may be handled by its recipient. The unreaped leader is still a member of its group, so a
  signal to a group whose other members are gone succeeds and changes
  nothing.
- **Pipes** are the parent's ends of what `Spawn` made, read and written
  through the ring, as sockets are, with a pipe's errors.

### 6.3 The process's own

- **`ReadSignal`** reads one blocked signal from the signalfd the shell
  opened at startup: `SIGINT`, `SIGTERM`, and `SIGWINCH` for a service at
  a terminal (io.md, 7; shell.md, 6.3).
- **`Usage` reads what the process has used,** in one synchronous call:
  `getrusage` for the process itself (`RUSAGE_SELF`) and for its reaped
  children (`RUSAGE_CHILDREN`). Each part answers user and system CPU
  time, as `Duration`s, and the peak resident size, in bytes (the kernel
  counts it in KiB, which the backend multiplies, checked). The
  children's CPU is the sum over every child reaped so far, and over what
  each of them reaped in turn; their peak is the largest any one of them
  reached. A child that has exited but is not yet reaped is in neither
  part.
- **`Window` reads a terminal's size,** rows and columns, from the
  descriptor it names (`TIOCGWINSZ`). The shell found at startup that the
  descriptor is a terminal (shell.md, 6.3); one that has since hung up
  fails with the kernel's error, as `Other`.

### 6.4 A tree's metadata

- **What a tree's walk and its making need,** for a store of trees
  (hearth's): to read each entry without following it (`StatEntry`,
  `ReadLink`, the attributes), and to make each entry as a tree records it
  (`MakeDirectory`, `Create`, `MakeLink`, `HardLink`, `SetMode`,
  `SetOwner`, `SetAttribute`, `Truncate`). Each takes a name in an open
  directory, never a path, as `Rename` does (6.1), and acts on the entry
  itself, never a link's target: a link is read, owned and given
  attributes as a link. Setting an entry's times comes with its first
  user, hearth's shift of a slot (its M3).
- **`ReadLink`** reads a symbolic link's target, as bytes, into `buf`,
  and answers its length: `readlinkat`, which has no ring operation.
  `buf` holds at least 4,096 bytes, more than the longest target Linux
  keeps, so a target is never cut; a shorter `buf` is an invalid record.
  An entry that is not a link fails `NotALink`.
- **`MakeLink`** makes a symbolic link at a name, to the target given:
  the ring's `symlinkat`. The target is 1 to 4,095 bytes with no NUL,
  else an invalid record. It is kept as given and never resolved, so it
  may lead anywhere, out of the root included: what holds a root in is
  `Open`, which follows no link out of it (`Escape`, 6.1). A name taken
  fails `Exists`.
- **`HardLink`** gives an existing entry a second name: the ring's
  `linkat`, with no flags, so it links the entry itself, a symbolic link
  included, never a link's target. Both names lie in open directories on
  one filesystem (`Other(EXDEV)` otherwise). A name taken fails `Exists`;
  an entry with as many links as its filesystem allows, `TooManyLinks`
  (`EMLINK`); a directory, or a file the caller neither owns nor may read
  and write (Linux's `protected_hardlinks`), `Permission`.
- **`SetMode`** sets the mode, setuid, setgid and sticky included
  (`0o7777` at most, else an invalid record): `fchmodat2` with
  `AT_SYMLINK_NOFOLLOW`, which the kernel floor has. A symbolic link has
  no mode of its own and fails `NotSupported`. The kernel drops setgid
  from a file whose group the caller is not in, unless it may keep it
  (`CAP_FSETID`); a caller that must know states the entry again.
- **`SetOwner`** sets the user and the group: `fchownat` with
  `AT_SYMLINK_NOFOLLOW`, a link's own for a link. The kernel then clears
  setuid and setgid from a file that may run, and removes its capability
  attribute (`security.capability`), so a caller that makes an entry
  whole sets its owner first, then its mode, then its attributes. An ID
  the caller's user namespace does not map fails `InvalidArgument`; an
  owner the caller may not give, `Permission`.
- **Extended attributes,** by name: `ListAttributes` packs the entry's
  attribute names into `names`, each followed by a NUL, as the kernel
  gives them, and answers their length; `GetAttribute` reads one into
  `value` and answers its length; `SetAttribute` creates or replaces one;
  `RemoveAttribute` removes one. An attribute's name is 1 to 255 bytes
  with no NUL; `names` and `value` hold at least 65,536 bytes, Linux's
  largest list and value, so neither is ever cut; a value to set holds at
  most 65,536; each else an invalid record. An attribute the entry lacks
  fails `NoAttribute` (`ENODATA`); one its filesystem does not keep,
  `NotSupported`; one the caller may not touch, `Permission` (a `user.`
  attribute on a link, a FIFO or a device; `security.capability` without
  `CAP_SETFCAP`; a `trusted.` one without `CAP_SYS_ADMIN`). A list longer
  than Linux hands back fails `Other(E2BIG)`.
- **The attributes of the entry, never of a link's target.** At the
  kernel floor no attribute call takes a directory and a name, and the
  `l*xattr` calls take a path, which could leave the root. So the backend
  opens the entry path-only, without following it (`O_PATH |
  O_NOFOLLOW`, the name in the directory), calls `*xattr` on
  `/proc/self/fd/<n>`, a magic link to the entry itself, a link
  included, and closes it, all in the one synchronous call. This needs
  procfs at `/proc`: the ring checks at its start that `/proc/self/fd`
  names its own descriptors, and answers every attribute record
  `NotSupported` when it does not. Once the kernel floor has
  `getxattrat` and its siblings (Linux 6.13), the backend calls those,
  and the records stay as they are.
- **Truncating and making directories** are `Truncate` and
  `MakeDirectory` (6.1), and a file is made with `Create`: they need no
  record of their own.
- **Durability.** A new link and a new name for a file (`MakeLink`,
  `HardLink`) are entries of their directory: durable once it is synced,
  as a `Create`'s name is. A mode, an owner and attributes are the
  entry's: durable once its file or its filesystem is synced (`Sync`,
  `SyncFilesystem`); a link's own, which cannot be opened to sync, only
  with its filesystem.
- **Errors** are named per operation, on the module's table (6.1): each
  of these answers `NotFound` for an entry or a directory gone,
  `Permission`, `ReadOnly` and `NoSpace` for those that write, and
  `NameTooLong`, beside its own above.

### 6.5 The filesystem: its status, its sync, btrfs's subvolumes

- **`FilesystemStatus`** reads what the filesystem a descriptor lies on
  is, and its space: `fstatfs`, answering `Filesystem { kind, size,
  available }`. `kind` is `Btrfs`, or `Other` with the kernel's magic
  number; `size` and `available` are bytes, the filesystem's and what a
  user without privilege may still write (`f_bavail`).
- **`SyncFilesystem`** makes everything written to the filesystem a
  descriptor lies on durable: `syncfs`, answering `Nothing`. It is the
  one sync of a tree of many entries, whose links and modes have no
  descriptor to sync. A write the kernel failed to put on disk since the
  descriptor was opened fails it, with the kernel's error, as `Other`.
- **btrfs's subvolumes, through a closed list of its ioctls.** A store of
  trees on btrfs makes each tree a subvolume, snapshots it, makes it
  read-only, finds them all again at its start, and destroys them. Each
  of these records is one btrfs ioctl, and no other ioctl is offered:
  the record names what it does, and the backend holds the ioctl's
  structure. A record names a subvolume by a descriptor of its root
  directory (inode 256), one `Open` made with `Directory`; another
  directory may fail `InvalidArgument`.
  - `SubvolumeCreate` makes an empty subvolume at a name in an open
    directory (`BTRFS_IOC_SUBVOL_CREATE`); it needs only write
    permission there.
  - `Snapshot` makes a writable snapshot of the subvolume `source`, at a
    name in an open directory on the same filesystem
    (`BTRFS_IOC_SNAP_CREATE_V2`); Linux allows it to the source's owner.
  - `SubvolumeDestroy` destroys the subvolume at a name in an open
    directory, whatever it holds (`BTRFS_IOC_SNAP_DESTROY_V2`). Without
    privilege, the filesystem must be mounted `user_subvol_rm_allowed`,
    and the caller may write the subvolume. One that holds another
    subvolume fails `NotEmpty`.
  - `SubvolumeReadOnly` makes the subvolume `fd` read-only, or writable
    again (`BTRFS_IOC_SUBVOL_SETFLAGS`), for its owner.
  - `SubvolumeInfo` reads the subvolume `fd` (`BTRFS_IOC_GET_SUBVOL_INFO`),
    answering `Subvolume { tree, uuid, parent_uuid, read_only,
    generation }`: its tree ID, its UUID, the UUID of the subvolume it is
    a snapshot of (`None` for one that is not), whether it is read-only,
    and the generation of its last change. Anyone may read it.
  - `SubvolumeChildren` lists the subvolumes directly beneath the
    subvolume `fd`, from tree ID `from` up
    (`BTRFS_IOC_GET_SUBVOL_ROOTREF`): each child's tree ID and the inode
    of the directory it lies in (`Child { tree, directory }`), into
    `children`, which holds at least 255, Linux's page, else an invalid
    record; it answers how many. A page shorter than 255 is the last;
    after a full one, the caller asks again from the last tree ID plus
    one, which may answer none. The kernel's `EOVERFLOW`, which it gives
    when more children follow a full page, is that full page, not a
    failure. Anyone may list.
  - `SubvolumePath` says where a child lies
    (`BTRFS_IOC_INO_LOOKUP_USER`): from the child's tree ID and its
    directory's inode, as `SubvolumeChildren` gave them, it writes the
    child's path beneath the subvolume `fd`, the directory's path and the
    child's name joined by `/`, into `path`, of at least 4,096 bytes,
    else an invalid record, and answers its length. It needs read and
    search permission on each directory of the path (`Permission`).
  - Off btrfs, each fails `NotSupported` (`ENOTTY`).
- **The worst case.** The writing ones (create, snapshot, destroy,
  read-only) commit a btrfs transaction, milliseconds on an idle
  filesystem and seconds on a busy one; a destroy leaves the space to a
  cleaner the kernel runs later. They and `SyncFilesystem` are the long
  synchronous calls of section 6, which a service that must not stall
  makes in a process of its own.
- **Durability.** A subvolume made, snapshotted, made read-only or
  destroyed is durable once its filesystem is synced; btrfs may make it
  so sooner.

### 6.6 Cgroups, and the helper

- **A cgroup is two descriptors:** its directory, which a spawn into it
  and every record name, and its `cgroup.events`, which `Populated` reads
  and `CgroupChanged` polls. Each file a record touches is opened beneath
  the directory by its fixed name. No record carries a path or a PID.
- **`MakeCgroup`** makes the directory beneath its parent and opens both.
  It writes the limits (`memory.max`, `pids.max`), `memory.oom.group`
  where the memory controller is enabled, and `cgroup.max.depth` and
  `cgroup.max.descendants`, which only the parent's owner may change, so
  what is made beneath it stays within what `RemoveCgroup` walks. To
  delegate, it changes the owner of the directory and of each file the
  kernel lists in `/sys/kernel/cgroup/delegate`, which the backend reads
  once. A make that fails after its directory was made removes it before
  it answers: a failed make leaves nothing.
- **`EnableControllers`** writes them, `+cpu`, `+memory`, `+pids`, to the
  cgroup's `cgroup.subtree_control`; the kernel answers `Busy` while the
  cgroup holds a process. **`EnterCgroup`** writes `0` to its
  `cgroup.procs`, which moves the whole process, every thread.
- **`CgroupCounts`** reads `cpu.stat`'s user and system time, and, where
  the files are, `memory.peak` and `memory.events`'s `oom_kill`.
- **`SignalCgroup`** takes any of `Interrupt`, `Terminate`, `Kill`,
  `Stop`, or `Continue`, and reads the `cgroup.procs` of the cgroup and of each
  cgroup beneath it. For each PID it opens a pidfd, then checks by the
  process's `/proc/<pid>/cgroup`, compared a path segment at a time, that
  it is in the tree, then signals it
  through the pidfd. A process alive when the signal goes was the PID's
  owner throughout the check, which its pidfd held; one gone meanwhile is
  not signalled, and is no failure.
- **`KillCgroup`** writes `1` to `cgroup.kill`: the kernel kills every
  process in the cgroup and beneath it, those forked meanwhile included.
- **`Populated` and `CgroupChanged`** read `cgroup.events`, each read
  whole from its start (`pread` at 0), and poll it
  for the kernel's next change: the ring's poll (`POLLPRI`) on the same
  descriptor, the one cgroup record that is a ring operation, and the
  one that may be cancelled. The kernel marks each read on the
  descriptor, and the poll completes at once if the file changed since
  the last, so a read, a poll, and a read again lose no change. A read
  that does not start at 0 neither reads the state nor marks it read.
  The kernel may delay a change's wake by some milliseconds, and wake
  with no change: each wake reads again. An exited process not yet
  reaped no longer counts: `populated` falls as a process exits, before
  its parent hears of it, and a cgroup that holds only such processes
  may be removed. `pids.current` counts them until they are reaped.
- **`RemoveCgroup`** removes the cgroups beneath it, deepest first, then
  removes it from its parent (`..` of its directory) by the name io keeps.
  A cgroup that still holds a process answers `Busy`, and what was
  removed stays removed.

**The helper.** A `Spawn` that asks for ids or containment (io.md, 6.3)
starts the helper as the child, and the helper starts the command. The
helper is the service's own executable, `/proc/self/exe`, with a marker
argument, which `main` hands first to `skein-contain`, a crate of the
shell's (shell.md, 6.4): one file to ship, and no version between the
service and its helper to skew. A spawn that asks for neither is
`pidfd_spawn` alone, into its cgroup (`POSIX_SPAWN_SETCGROUP`) if it
names one.

- **Its descriptors.** The backend spawns the helper as any child, in
  its group and its cgroup, the pipes and given roots at their numbers,
  and two of its own above them, named in its arguments:
  - **the spec,** a pipe the backend wrote the whole spawn into before
    the spawn, at most 64 KiB, a pipe's capacity, so the write never
    waits;
  - **the report,** a pipe whose read end is `Done::Spawned`'s `report`.
    It closes when the command executes (close-on-exec), and its end of
    file tells io the command started. A process of the helper that fails
    writes one record (its stage, the kernel's error) and exits. A spawn
    the kernel refuses before the helper runs fails as any spawn does.

**The setup, in order.** H is the child io holds; M maps; I is the pid
namespace's first process; C is the command.

1. **H** reads the spec whole, and blocks every signal it will relay, so
   that one sent before it relays is queued, not lost. Asked for ids
   alone, it sets the supplementary groups, then the group, then the
   user (`setgroups`, `setresgid`, `setresuid`), and executes the
   command at `dir`.
2. Contained, **H forks M** first. M closes every descriptor it
   inherited, sets `PR_SET_PDEATHSIG`, exits if its parent is no longer
   H (whose PID it was given before the fork), and waits to be told.
3. **H unshares,** in one call, a user namespace and the mount, ipc and
   cgroup namespaces, with the pid, network and uts ones as asked: the
   user namespace owns them all, H holds every capability in them, and
   the cgroup namespace's root is the tree's cgroup, which H was spawned
   into.
4. **M writes H's maps,** from the service's namespaces, against H's
   PID, which cannot be reused while H lives: itself, to `uid_map`,
   `setgroups` and `gid_map`, where it may (one range of its own id, or
   with `CAP_SETUID` and `CAP_SETGID`), and otherwise by running
   `newuidmap`, then `newgidmap`, each a child it waits for, with H's PID
   and the maps. Both check that the caller owns H, by the owner of
   `/proc/<H>`, which a process freshly executed from a readable file
   keeps. `newgidmap` denies `setgroups` in the namespace when the map is
   one id, the caller's own primary group, which `/etc/subgid` does not
   give it. M exits with what came of it, and H, which reaps it, fails at
   `Namespaces` unless both maps were written. A setuid `newuidmap` gains
   no privilege for a service that runs with `no_new_privs`: the shell
   checks at startup (shell.md, 6.4).
5. **H takes what the view is made of,** still the service's ids on the
   host, so with its rights over each path: for the base and each bind,
   it opens the host path in the new mount namespace (`openat2`,
   `O_PATH`, `RESOLVE_NO_SYMLINKS`), checks its device and inode against
   the io root's descriptor, and clones the mount there
   (`open_tree`, `OPEN_TREE_CLONE`, `AT_RECURSIVE`). What was checked is
   what is mounted; a path that is not the io root's directory fails the
   view. Each host path is the one io was given for the root: one with a
   link on the way fails.
6. **H takes the namespace's root:** supplementary groups dropped where
   `setgroups` is allowed, then `setresgid` and `setresuid` 0. Without a
   pid namespace, H goes on as I does, from step 7, and executes the
   command itself. With one, **H forks I**, then relays: it reads the
   signals it blocked from a signalfd and passes each to I, waits for
   I, and exits with the status I sent it over a pipe, or with I's own
   if none came. A command that ended by a signal, H ends by the same:
   its default action restored, unblocked, dumping off
   (`PR_SET_DUMPABLE` 0). I sets `PR_SET_PDEATHSIG` (`SIGKILL`) and
   then checks, by a pidfd of H opened before the fork, that H still
   lives; I's ids never change, which would clear it.
7. **I composes the view** (io.md, 6.3), if one is asked: every mount
   private, recursively; the new root, an empty `tmpfs` or the base's
   clone, attached on `/tmp` of the namespace's copy of the host's tree,
   and made the working directory; `/proc`, new (`nosuid`, `nodev`,
   `noexec`), mounted while the host's `/proc` is still in the
   namespace, which the kernel requires of a user namespace's; each
   bind's clone attached (`move_mount`) on its target, which is opened
   beneath the new root following no link (`RESOLVE_IN_ROOT`,
   `RESOLVE_NO_SYMLINKS`), made if missing where the root may be
   written; read-only binds, and `read_only` paths bound on themselves,
   made read-only recursively (`mount_setattr`, `MOUNT_ATTR_RDONLY`,
   `AT_RECURSIVE`), which keeps each mount's other flags, the locked
   ones among them; the scratches and `/tmp`, `tmpfs`; `/sys`, new
   with a network namespace of its own, else the host's, read-only;
   `/dev`, a `tmpfs` with the host's devices bound in, its own `devpts`
   (`newinstance`, `ptmxmode=0666`, `mode=0620`, no `gid`) and links.
   Then `pivot_root(".", ".")`, the old root detached, `/` the working
   directory, and the root read-only, not recursively, unless given
   writable.
8. **I finishes:** the hostname; loopback up, in a network namespace of
   its own; the rlimits, lowered. It **forks C**, relays to it as H does
   to I, reaps whatever ends in the namespace, and once C has ended sends
   H C's status and exits 0: the first process of a pid namespace cannot
   end by a signal it raises on itself. Its end ends the pid namespace:
   the kernel kills what is left in it.
9. **C** sets the ids inside, its supplementary groups cleared, or set
   to `ids.groups`, where `setgroups` is allowed; then `no_new_privs`;
   its working directory; its signals' mask and handlers reset; every
   descriptor closed but the pipes, the given roots and the report; and
   executes the command.

Each process closes what only its child needs once it has forked it, so
that the report's end of file and the pipes' ends are the command's. A
step that fails writes its stage and exits: `Ids` (1, 6, 9's ids),
`Namespaces` (3, 4), `View` (5, 7), `Command` (9). io refuses, at its
entrance, a view without a pid namespace of its own, for its `/proc`,
and a view whose maps leave the namespace's 0 unmapped, which could
create nothing in its `tmpfs`.

The report's end of file with no record is also what a helper killed
during its setup leaves, by a deadline or for want of memory: io then
answers `Spawned`, and the child's exit is by that signal (io.md, 6.3).

The command runs with the capabilities its ids have in the namespace,
with no seccomp filter (io.md, 10), and with no terminal.

## 7. Broken invariants

Some mistakes io never makes, so a backend may assume they never happen:

- an invalid record (an empty receive buffer, a send with nothing left,
  a name with a `/` in it, a buffer below its operation's least);
- a token already in flight, or a cancel of a cancel;
- an address of the wrong family;
- more than one receive, send or accept in flight on a socket, or
  anything beside a connect;
- any operation but close after a failed connect, a connect on a socket
  that is not fresh, a listen on an unbound socket;
- a half-close on a socket that is not a connection, or during a send;
- an operation on files on a socket's descriptor, or one on sockets on a
  file's; a read on a descriptor not opened to read, a write on one not
  opened to create, a list on one opened to create;
- a cancel of a stat, a rename, a removal, a new directory, a listing, a
  lock, or any operation of sections 6.4 and 6.5;
- a lock, or an operation of sections 6.4 and 6.5, on a socket, a pipe
  or a path-only descriptor;
- a close with anything else in flight on its descriptor, an operation on
  files being on every descriptor it names;
- an `Append` beside another in flight on its descriptor, or on one not
  open to write;
- a `Wait` beside another on the same pidfd, or one that reaps before a
  `Wait` has reported the exit; any operation on a pidfd after its reap
  but its close;
- a cancel of a spawn, a signal, a usage or a window, or of a cgroup
  record but `CgroupChanged`;
- a `CgroupChanged` beside another on the same descriptor; a given
  descriptor or a pipe at a number another has, or at one the helper's
  own would take; a view without a pid namespace.

The simulator fails the world on each; the ring checks the first two at
submit.

## 8. Testing

The conformance suite is the contract's executable form
(testing-strategy.md, 5). It is a crate of its own,
`testing/skein-conformance`, over a small backend interface (open a
process, submit, reap, enter, let time pass), and runs against the
simulator (`tests/conformance/sim`) and against the ring on the real
kernel (`tests/conformance/ring`), each of which implements the interface
for its backend.

- **Each scenario is a scripted sequence of records** that returns what it
  saw, and its check names the rule of the contract behind each
  assertion. An answer that waits on the peer's reset or acknowledgement
  is retried until it changes.
- **A driver checks every completion on the way:** valid for its
  operation, one per submission, the record handed back with its buffer
  in the same `Box`, and every descriptor closed at the end.
- **On the simulator,** each scenario runs over calm seeds and seeds of
  every fault loopback can show: a few of chaos in the focused suite, many
  in the fuzzy one (testing-strategy.md, 8). Every outcome a race allows
  must appear over the fuzzy suite's seeds, and a calm world must pair
  each race as the ring does.
- **On the ring,** each scenario runs once, and fails, saying so, if
  io_uring is not usable.
- What no record can observe (close-on-exec, `TCP_NODELAY`, the modes of
  new files) is not checked; `SO_REUSEADDR` and `IPV6_V6ONLY` are, by
  their effects.
- **Files beneath a root of the scenario's own.** Each scenario on files
  lays out its root from a tree in the suite's own vocabulary (files,
  directories, symbolic links, each with its mode): on the ring, a scratch
  directory made per scenario beneath the system's temporary directory,
  with a directory beside it for a link to lead out to, and removed with
  the backend; in the simulator, the minimal fake machine (testing.md, 4).
  The scenarios: a file made, written at offsets over itself and past its
  end, synced, read back and stated; a `0o000` file and directory of the
  scenario's own user that `Read` is denied and `PathNoFollow` opens and
  stats (on the ring, the denial is checked only without
  `CAP_DAC_OVERRIDE`, which skips it), and a link it opens as a link; renames over a file, across
  directories, of a file over a directory and the other way, over a full
  and an empty directory, beneath itself, over itself, and the idiom that
  replaces a file whole; removals of files, directories, a link and a file
  open; new directories, and one removed while open; listings whole, one
  entry at a time, and cut short by long names; a root beneath a root;
  forty-four paths that leave their root, stay beneath it, or name a FIFO,
  which opens as no file; locks: a directory locked through one open,
  `Busy` through a second open of it, locked again through the first,
  and taken through the second once the first is closed; and what the
  owner may not do. Each names every error its operations can be made to
  answer on a healthy scratch directory, and, where two could answer, the
  one Linux checks first: a removed directory before a name's length, a
  final `/` before the last name on a create, both directories of a
  `Rename` before its source, its source before its target, a name
  looked up before its directory is written; a no-replace rename, once
  both directories are searched and the source is found, checks for a
  taken target before ancestor, same-inode and write-permission checks.
  The driver checks that every path, name and buffer comes back in its
  `Box`, written only where the count says.
- **What only the simulator shows:** a disk beyond a healthy one (no
  space, a filesystem gone read-only, an I/O error) is the simulator's
  own tests' (simulator.md, 6), and an `Open` past the descriptor limit a
  scenario on the simulator only: lowering a process's limit on the real
  kernel takes `unsafe` outside the ring adapter, or a child process, and
  neither is allowed. A `Cancel` of a file's `Read` runs on
  both: the ring stops it or comes too late, from one run to the next,
  and requires a pairing the simulator draws; the simulator's `hung`
  fault, a filesystem gone away, adds the `Read` that waits for good,
  which no scratch directory shows, and every pairing must appear over
  the fuzzy seeds. `EMLINK` (`TooManyLinks` on a `Rename`, a
  `MakeDirectory` or a `HardLink`) neither backend provokes. A `Rename` across
  filesystems (`Other(EXDEV)`) is the ring's own test, between the
  temporary directory and `/dev/shm` where they are two mounts. Short
  reads and writes are an outcome the simulator draws and the ring never
  gives on a regular file: the fuzzy suite asserts both counts appear over its
  seeds, and a calm world counts every byte, as the ring does.
- **Appending.** A scenario opens a file to append as the shell does, in
  its own root: `Append`s land at the end in order, after bytes another
  descriptor appended between them, and a file that was not empty keeps
  what it held; a short `Append` is continued from its count; a `Cancel`
  stops one or comes too late, as for a `Write`. The suite's tree gains a
  second name for a file (a hard link), so that `Stat` answers two links
  for it, one for the rest, and the owner every scenario's files have.
- **A tree's metadata,** on both backends, in the scenario's own root,
  whose tree gains FIFOs, modes with setuid, setgid and sticky, and
  `user.` attributes: `StatEntry` of each kind it lays out, a link
  stated as a link, its inode, device and links those `Stat` answers
  through an open, its group the scenario's, and `changed` moving on
  after a `SetMode`, never back; `ReadLink` of a link, whole, and
  `NotALink` of a file; `MakeLink` to a target out of the root, which
  `ReadLink` reads back as written and an `Open` through it refuses with
  `Escape`; `HardLink` of a file and of a link, each counted in its
  links, and `Exists`; `SetMode` with setuid, setgid and sticky, read
  back, and `NotSupported` on a link; `SetOwner` to the scenario's own
  user and group, the only ones a run without `CAP_CHOWN` may give, and
  `Permission` for another's, checked only without it; a `user.`
  attribute set, listed, read, replaced and removed, then `NoAttribute`;
  a value of 65,536 bytes; `Permission` for a `user.` attribute on a
  link. On the ring, the scratch directory's filesystem keeps `user.`
  attributes (tmpfs and ext4 do); a scenario that finds it does not
  fails, saying so.
- **The filesystem,** on both backends: `FilesystemStatus` of the root
  answers a kind, and an available space no larger than its size;
  `SyncFilesystem` answers `Nothing`; and where the kind is not btrfs,
  each subvolume record answers `NotSupported`.
- **btrfs, on the ring alone,** in the directory `SKEIN_BTRFS_SCRATCH`
  names, on a btrfs filesystem mounted `user_subvol_rm_allowed`, where
  the run may write; without it, each such scenario is skipped by name,
  saying so. A subvolume made, a file written in it, a snapshot of it
  whose parent UUID is its UUID, made read-only (a write in it then fails
  `ReadOnly`) and writable again; the scratch subvolume's children, both
  among them, and each one's path; `from` past the first child's tree,
  which leaves it out; both destroyed, and `NotFound` after. The
  simulator has no btrfs: the minimal fake machine answers these records
  `NotSupported` (testing.md, 4), and a service's own fake machine that
  models btrfs holds itself to these scenarios (hearth's, its
  domain/testing.md, 3.3).
- **Processes,** with the fixture program of both backends (echo its
  input, exit with a status, never exit): pipes through chosen
  descriptors, an exit status kept, a `Wait` that waits while the child
  lives, a signal, a kill. To these the groups add: the fixture starting
  a child of its own in its group and exiting; a `Wait` that observes the
  exit, then a signal to the group, which ends the grandchild and its
  hold on a pipe; then the reaping `Wait`. Usage, in a process that has
  reaped no child before: the children's part zero, still zero after the
  observing `Wait`, and with a peak above zero after the reap; and the
  process's own CPU and peak never falling between two reads.
- **Cgroups,** on both backends, beneath a cgroup the run may write: on
  the ring, the one `SKEIN_CGROUP_SCRATCH` names, or else a delegated
  one above the run's own, found as the end-to-end kit finds it
  (examples.md, 6); without one, each scenario is skipped by name, saying
  so. A cgroup made, with limits, counted, removed; one made twice
  (`Exists`); a limit without its controller; controllers refused while
  the cgroup holds a process; the run's fixture spawned into one, with a
  grandchild that leaves its group (`setsid`) and stays in the cgroup; a
  terminate that reaches both; `Populated` true, `CgroupChanged` pending,
  then a kill, after which it completes and `Populated` is false, with
  the first child exited but unreaped; a removal refused while populated,
  then deepest first once empty.
- **Contained trees, on the ring alone,** where the run's user has
  subordinate ids and `newuidmap`; else skipped by name, saying so. Ids
  alone, read back by the fixture (run as root, or skipped). A tree in a
  user namespace whose maps name subordinate ids only, whose own user
  and group then read as the overflow ids, and one whose maps name the
  run's own group, where `setgroups` is denied; each stage failed in turn
  and reported as it; a view whose git directory is read-only inside a
  writable bind, the write refused; a bind whose path was swapped for a
  link, which fails the view; `/proc` showing the tree's processes alone;
  loopback alone; the hostname; a double fork contained, and killed with
  the cgroup; a polite signal relayed to the command, and the command's
  exit status and its death by a signal, each relayed to the first
  process, and a signal sent before the relay was ready still relayed;
  a bind beneath a directory only the service's user may search; a
  service run with `no_new_privs` refused the maps that need
  `newuidmap`.

## 9. Open questions

- **Fixed files** change what an `Fd` names: a slot in the ring's table
  instead of a descriptor. Decide when it has been measured.

## 10. Not built yet

What of this document is not built is `STATUS.md`'s (its row in section 2,
and section 3).
