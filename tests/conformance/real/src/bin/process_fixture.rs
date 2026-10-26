//! A child executable for the ring's process conformance scenarios.

#[path = "../../tests/support/given_listener.rs"]
mod given_listener;

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, ExitCode, Stdio};

#[cfg(target_os = "linux")]
const DESCRIPTORS: &str = "/proc/self/fd";
#[cfg(target_os = "macos")]
const DESCRIPTORS: &str = "/dev/fd";

fn main() -> ExitCode {
    skein_fail_stop::install();
    let mut args = std::env::args();
    let _program = args.next();
    let mode = args.next();
    #[cfg(target_os = "linux")]
    {
        match mode.as_deref() {
            Some("parent-death-parent") => return parent_death_parent(&args.next().expect("exec mode")),
            Some("parent-death-child") => {
                assert_eq!(args.next().as_deref(), Some("--parent"));
                let parent = args.next().expect("parent PID").parse().expect("numeric parent PID");
                return parent_death_child(parent, &args.next().expect("exec mode"));
            }
            _ => {}
        }
    }
    fixture(mode.as_deref(), &mut args)
}

#[expect(
    clippy::zombie_processes,
    reason = "the process-tree fixture deliberately leaves a descendant for the keeper to settle"
)]
fn fixture(mode: Option<&str>, args: &mut std::env::Args) -> ExitCode {
    match mode {
        Some("echo") => {
            let input = args.next().expect("input descriptor");
            let output = args.next().expect("output descriptor");
            let mut reader = File::open(format!("{DESCRIPTORS}/{input}")).expect("child input pipe");
            let mut writer =
                OpenOptions::new().write(true).open(format!("{DESCRIPTORS}/{output}")).expect("child output pipe");
            let mut bytes = [0_u8; 4096];
            loop {
                let n = reader.read(&mut bytes).expect("read child input");
                if n == 0 {
                    break;
                }
                writer.write_all(&bytes[..n]).expect("write child output");
            }
            ExitCode::SUCCESS
        }
        Some("given-listener") => given(&args.next().expect("held address")),
        Some("megabyte") => megabyte(),
        Some("usage") => {
            let mut accumulator = 1_u64;
            for _ in 0..2_000_000 {
                accumulator = std::hint::black_box(accumulator.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
            std::hint::black_box(accumulator);
            ExitCode::SUCCESS
        }
        Some("exit") => {
            let code = args.next().expect("exit code").parse::<u8>().expect("numeric exit code");
            ExitCode::from(code)
        }
        Some(mode @ ("fork-exit" | "fork-live" | "escape-exit" | "escape-zombie-exit")) => {
            let mut command = if matches!(mode, "escape-exit" | "escape-zombie-exit") {
                let mut command = Command::new("/usr/bin/setsid");
                command.arg("/bin/sh");
                command
            } else {
                Command::new("/bin/sh")
            };
            let script = if mode == "escape-zombie-exit" {
                "printf 'ready\n' >&2; exit 0"
            } else {
                "printf 'ready\n' >&2; exec sleep 60"
            };
            let mut child =
                command.args(["-c", script]).stderr(Stdio::piped()).spawn().expect("the fixture starts its descendant");
            let mut ready = String::new();
            BufReader::new(child.stderr.take().expect("readiness pipe"))
                .read_line(&mut ready)
                .expect("descendant readiness");
            assert_eq!(ready, "ready\n");
            if mode == "escape-zombie-exit" {
                // Leave the exited descendant unreaped, after it has left its
                // original group/session and the cgroup's live population.
                let status = format!("/proc/{}/status", child.id());
                while !std::fs::read_to_string(&status).expect("unreaped descendant status").contains("State:\tZ") {
                    std::hint::spin_loop();
                }
            }
            eprintln!("descendant:{}", child.id());
            if mode == "fork-live" {
                loop {
                    std::thread::park();
                }
            }
            ExitCode::SUCCESS
        }
        Some("never") => loop {
            std::thread::park();
        },
        Some(mode @ ("supervise-term" | "supervise-ignore" | "supervise-grandchild")) => {
            let mut command = Command::new("/usr/bin/setsid");
            command.arg("/bin/sh");
            let script = match mode {
                "supervise-term" => {
                    "trap 'printf tree-term\\n >&2; exit 0' TERM; printf 'ready\\n'; while :; do :; done"
                }
                "supervise-ignore" => "trap '' TERM; printf 'ready\\n'; exec sleep 60",
                "supervise-grandchild" => {
                    // The leaf reports its PID before readiness can prompt TERM.
                    r#"/usr/bin/setsid /bin/sh -c "trap 'printf tree-term\\n >&2; exit 0' TERM; printf 'grandchild:%s\\n' \"\$\$\" >&2; printf 'ready\\n'; while :; do :; done" & wait"#
                }
                _ => unreachable!("the fixture selected its supervised mode"),
            };
            let mut child = command
                .args(["-c", script])
                .stdout(Stdio::piped())
                .spawn()
                .expect("the fixture starts its detached descendant");
            let mut ready = String::new();
            BufReader::new(child.stdout.take().expect("readiness pipe"))
                .read_line(&mut ready)
                .expect("descendant readiness");
            assert_eq!(ready, "ready\n");
            eprintln!("descendant:{}", child.id());
            loop {
                std::thread::park();
            }
        }
        other => panic!("unknown fixture program {other:?}"),
    }
}

#[cfg(target_os = "linux")]
struct ChildGuard(std::process::Child);

#[cfg(target_os = "linux")]
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
fn parent_death_parent(mode: &str) -> ExitCode {
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().expect("fixture path"))
            .args(["parent-death-child", "--parent", &std::process::id().to_string(), mode])
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn exec child"),
    );
    std::fs::write("child-pid", child.0.id().to_string()).expect("report child PID");
    let mut start = [0_u8; 1];
    std::io::stdin().read_exact(&mut start).expect("observer registered child");
    child.0.stdin.take().expect("child input").write_all(&start).expect("release exec child");
    let status = child.0.wait().expect("wait for exec child");
    if status.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

#[cfg(target_os = "linux")]
fn parent_death_child(parent: u32, mode: &str) -> ExitCode {
    use std::os::unix::process::CommandExt;

    let mut start = [0_u8; 1];
    std::io::stdin().read_exact(&mut start).expect("observer ready for exec");
    let mut command = Command::new("/bin/sleep");
    command.arg("30");
    let error = match mode {
        "protected" => skein_shell::exec_dying_with_parent(command, parent, skein_io::kernel::Signal::Kill),
        "plain" => command.exec(),
        other => panic!("unknown parent-death mode {other:?}"),
    };
    eprintln!("parent-death exec refused: {error}");
    ExitCode::FAILURE
}

fn megabyte() -> ExitCode {
    let pattern: [u8; 4096] = std::array::from_fn(|at| u8::try_from(at % 256).expect("pattern byte"));
    let mut stdout = std::io::stdout().lock();
    for _ in 0..256 {
        stdout.write_all(&pattern).expect("write patterned megabyte");
    }
    ExitCode::SUCCESS
}

fn given(address: &str) -> ExitCode {
    let address = address.parse().expect("socket address");
    let inherited = skein_shell::inherited_listeners().expect("inherited listening sockets");
    assert_eq!(&*inherited, &[(skein_io::kernel::Fd::new(3), address)]);
    let limits = given_listener::limits();
    let mut state = skein_io::Io::new(&limits);
    let token = state.adopt_listener(inherited[0].0, address).expect("adopt inherited listener");
    let mut kernel = skein_shell::Kernel::open(skein_shell::Config {
        operations: skein_io::operations(&limits).expect("operation bound"),
    })
    .expect("child kernel");
    given_listener::claim_and_accept(&mut kernel, &mut state, token, address);
    ExitCode::SUCCESS
}
