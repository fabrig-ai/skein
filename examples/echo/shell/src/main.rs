//! `skein-echo`, the echo example's main (examples.md, 4): its
//! configuration, startup (shell.md, 6), and the loop of
//! programming-model.md, section 2, over the shell kit's kernel and clock.
//!
//! ```text
//! skein-echo [ADDRESS]
//! ```
//!
//! It listens at `ADDRESS` (127.0.0.1:7007 unless given; port 0 picks one)
//! and answers each line with itself. SIGINT and SIGTERM arrive through
//! io (io.md, section 7): it stops admitting clients, drains its sessions
//! and exits successfully. A listener failure exits with a diagnostic.

use std::env;
use std::net::SocketAddr;
use std::process::ExitCode;

use skein_echo_service::{self as service, Limits};
use skein_echo_shell::{Echo, limits};
use skein_shell::{Backend, Clock, Config, Kernel, drive};

#[cfg(test)]
mod tests;

const USAGE: &str = "usage: skein-echo [ADDRESS]";

/// Where it listens unless told.
const ADDRESS: &str = "127.0.0.1:7007";

/// What main is configured with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Configuration {
    addr: SocketAddr,
    limits: Limits,
}

fn main() -> ExitCode {
    skein_fail_stop::install();
    let configuration = match configure(env::args().skip(1)) {
        Ok(configuration) => configuration,
        Err(why) => {
            eprintln!("skein-echo: {why}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(&configuration) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("skein-echo: {why}");
            ExitCode::FAILURE
        }
    }
}

/// The configuration from the arguments, and the limits of every layer.
fn configure(args: impl Iterator<Item = String>) -> Result<Configuration, String> {
    let mut addr = None;
    for arg in args {
        if addr.is_none() {
            addr = Some(arg.parse().map_err(|_| format!("{arg:?} is not an address and port"))?);
        } else {
            return Err(format!("unexpected argument {arg:?}"));
        }
    }
    let addr = match addr {
        Some(addr) => addr,
        None => ADDRESS.parse().map_err(|_| "the default address parses".to_owned())?,
    };
    Ok(Configuration { addr, limits: limits() })
}

/// Startup, then the loop, until a shutdown or listener failure settles.
fn run(configuration: &Configuration) -> Result<(), String> {
    startup(configuration)?;
    let limits = &configuration.limits;
    // 3. Block termination signals before opening the ring, then adopt their
    // source into io (shell.md, section 6; io.md, section 7).
    let signals = skein_shell::open_termination_signals()
        .map_err(|errno| format!("cannot open termination signals (errno {errno})"))?;
    // 7. The seed, and the kernel, which probes the ring for the floor.
    let seed = skein_shell::seed().map_err(|errno| format!("the kernel refused a seed (errno {errno})"))?;
    let operations = service::operations(limits).ok_or("the ring's size is past a u32")?;
    let mut kernel = Kernel::open(Config { operations }).map_err(|error| error.to_string())?;
    let chosen = kernel.chosen();
    if chosen.backend != Backend::Ring {
        eprintln!("skein-echo: kernel: {chosen}");
    }
    let clock = Clock::new();
    let mut host = Echo::new(*limits, skein_io::Addr::Ip(configuration.addr), seed);
    host.svc.adopt_signals(signals).map_err(|_| "the configured io has no slot for termination signals")?;
    drive(&mut kernel, &clock, &mut host);
    match host.svc.failure() {
        Some(error) => Err(format!("the listener at {} failed: {error:?}", configuration.addr)),
        None => Ok(()),
    }
}

/// Startup's checks (shell.md, 6), before anything is opened.
fn startup(configuration: &Configuration) -> Result<(), String> {
    let limits = &configuration.limits;
    // 1. The configuration, the limits of every layer among it, read; and
    // each machine's largest demand within the cap below it (io.md, 2).
    limits.check().map_err(|unusable| format!("the limits cannot run: {unusable:?}"))?;
    // 3. Termination signals are blocked and adopted in run, before the ring.
    // 4. Roots for io's files: the echo opens none.
    // 5. Peer names: the echo dials no one; its address is given as one.
    // 6. TLS: none.
    Ok(())
}
