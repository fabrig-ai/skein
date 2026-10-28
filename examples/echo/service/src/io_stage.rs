//! The service's io stage: ready work, completions, then deadlines.

use skein_io::{self as io, kernel::{Complete, Submit}};
use skein_lib::{Env, Queue, Time};

pub(crate) fn up(machine: &mut io::Io, env: &Env<io::Limits>, completions: &mut Queue<Complete>, told: &mut Queue<io::Event>, submissions: &mut Queue<Submit>, now: Time) {
    let limits = env.limits;
    // Each entity is resumed at most once a stage, and each refusal told once.
    let entries = limits.sockets.checked_add(limits.refusals).expect("io's limits fit a u32");
    for _ in 0..entries {
        if !machine.is_ready() || !room(submissions, io::MAX_OUT_RESUME) {
            break;
        }
        io::resume(machine, env, told, submissions);
    }
    if machine.is_ready() {
        return;
    }
    for _ in 0..completions.len() {
        if !room(submissions, io::MAX_OUT_UP) {
            break;
        }
        let Some(complete) = completions.pop() else {
            break;
        };
        io::up(machine, env, complete, told, submissions);
    }
    if !completions.is_empty() {
        return;
    }
    // A close's deadline and a retry's, per socket.
    let timers = limits.sockets.checked_mul(2).expect("io's limits fit a u32");
    for _ in 0..timers {
        if !machine.is_due(now) || !room(submissions, io::MAX_OUT_FIRE) {
            break;
        }
        io::fire(machine, env, told, submissions);
    }
}

fn room(submissions: &Queue<Submit>, max: io::MaxOut) -> bool {
    submissions.room() >= max.submissions
}

