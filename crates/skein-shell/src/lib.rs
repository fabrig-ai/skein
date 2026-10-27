//! The shell kit (shell.md): what a service's `main` runs its
//! loop on (programming-model.md, section 2). Host and settings are portable;
//! system access and the drive loop are native, under `cfg(unix)` (shell.md, 14).
//!
//! - [`Kernel`]: the ring or readiness backend of io's kernel records
//!   (`skein_io::kernel`; shell.md, 3), opened once, then submitted to
//!   and reaped from once per iteration.
//! - [`open_append`]: a regular file opened beneath a root for output at startup.
//! - [`inherited_listeners`], [`listener_at`] and [`open_listener`]: startup listeners.
//! - [`open_root`]: a directory opened at startup as a root for io's files.
//! - [`effective_user`]: the startup user ID for io's private-file configuration.
//! - [`directory_access`]: the kernel's startup answer on writing and searching a directory.
//! - [`process_id`]: the startup process ID for the service's report.
//! - [`read_file`] and [`read_trust_roots`]: startup reads within configured bounds.
//! - [`resolve`] and [`help`]: a command's settings, their sources and help (shell.md, 6.5).
//! - [`open_termination_signals`]: the SIGINT/SIGTERM source io adopts.
//! - [`Clock`]: monotonic and wall time, read together once per iteration.
//! - [`seed`]: the random seed, drawn from the system once at startup.
//!
//! [`Host`] adapts the service; [`drive`] runs it until settlement (shell.md, 12 and 13).
//!
//! This is ordinary Rust (programming-model.md, 10.2). System calls live
//! in `linux`, including its ring adapter, and `darwin`. The crate's
//! lints confine the audited boundary to these modules (testing.md, 6).

#[cfg(unix)]
mod calls;
#[cfg(unix)]
mod clock;
#[cfg(all(unix, target_os = "macos"))]
mod darwin;
mod drive;
#[cfg(unix)]
mod group;
#[cfg(unix)]
mod kernel;
#[cfg(unix)]
mod listeners;
// Linux's calls, ring adapter and their unit tests are Linux's alone.
#[cfg(all(unix, target_os = "linux"))]
mod linux;

#[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
compile_error!(
    "Apple Silicon only: aarch64-apple-darwin; supported targets are Linux, aarch64-apple-darwin and wasm32-unknown-unknown"
);
#[cfg(not(any(target_os = "linux", target_os = "macos", all(target_arch = "wasm32", target_os = "unknown"))))]
compile_error!("skein-shell supports Linux, aarch64-apple-darwin and wasm32-unknown-unknown");
#[cfg(unix)]
mod process;
#[cfg(unix)]
mod readiness;
#[cfg(unix)]
mod seed;
mod settings;
#[cfg(unix)]
mod startup;
#[cfg(unix)]
mod terminal;
#[cfg(all(test, unix))]
mod tests;

#[cfg(unix)]
pub use clock::{Clock, Now};
#[cfg(all(unix, target_os = "macos"))]
pub(crate) use darwin as system;
pub use drive::Host;
#[cfg(unix)]
pub use drive::drive;
#[cfg(all(unix, target_os = "linux"))]
pub(crate) use linux as system;

#[cfg(unix)]
pub use group::{child_id, group_live, process_live, signal_group};
#[cfg(unix)]
pub use kernel::{Backend, Chosen, Config, Kernel, OpenError, Wait};
#[cfg(unix)]
pub use listeners::{ListenersRefusal, MAX_INHERITED_LISTENERS, inherited_listeners, listener_at, open_listener};
#[cfg(unix)]
pub use process::{
    HostedPipes, abandon_binary, close_keeper_fd, exec_dying_with_parent, hosted_pipes, open_signal_pipe,
    open_termination_signals, poll_child, signal_current_thread, signal_kept_child, start_binary, write_service_signal,
};
#[cfg(unix)]
pub use seed::{crypto_seed, seed};
pub use settings::{Base, Fallback, Origin, Reason, Refusal, Resolved, Setting, Settings, Value, help, resolve};
#[cfg(unix)]
pub use settings::{arguments, environment};
#[cfg(unix)]
pub use startup::{
    DirectoryAccess, OutputKind, RootBounds, Roots, RootsRefusal, Trust, Unread, directory_access, effective_user,
    make_root, open_append, open_beneath, open_root, prepare_output, process_id, read_file, read_private_file,
    read_trust_roots,
};
#[cfg(unix)]
pub use system::wait_readable;

#[cfg(unix)]
pub use terminal::{Input, Kept, Keys, Modes, Watch, open_signals, raw_input, resize_terminal, terminal_modes};

#[cfg(all(unix, target_os = "linux"))]
pub use process::{
    make_subreaper, open_cgroup, open_pidfd, pidfd_exited, set_subreaper, start_binary_in_cgroup, subreaper,
    wait_cgroup_change,
};
