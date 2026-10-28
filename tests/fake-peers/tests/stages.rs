//! Drive the shared production face with a kernel played by hand.

// Keep private stage state accessible without adding a public peer control.
#[path = "../../../crates/skein-fake-peers/src/face.rs"]
#[expect(dead_code, reason = "this world exercises the shared up stage, not peer admission and shutdown")]
mod face;

use skein_io::{self as io, Event, Request, kernel};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Duration, Env, Queue, Time, Token, Wall};

#[test]
fn the_face_tells_ready_results_before_completions_and_timers() {
    // The production kernel gate needs two free submissions. One held write
    // in a queue of two blocks all three stages until the kernel takes it.
    const KERNEL_ROOM: u32 = 2;
    const OWNER: Token = Token::new(99);
    const DEADLINE: Duration = Duration::from_secs(1);
    let limits = face::Limits {
        io: io::Limits {
            sockets: 3,
            refusals: 1,
            intake: 32_768,
            receive: 1024,
            output: 32_768,
            sends: 2,
            accepts: 1,
            backlog: 1,
            close_timeout: DEADLINE,
            names: 0,
            retry: Duration::from_millis(1),
        },
        connections: 2,
        queue: 64,
        plaintext: 32_768,
        ciphertext: 32_768,
        observations: 1,
        observation_bytes: 1,
    };
    let mut face = face::Face::new(([127, 0, 0, 1], 0).into(), limits).expect("face limits");
    drop(face.requests.pop());
    face.submissions = Queue::with_capacity(KERNEL_ROOM);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.io };
    let timed = face.io.adopt_append(kernel::Fd::new(40), DEADLINE).expect("append slot");
    let ready = face.io.adopt_write_pipe(kernel::Fd::new(41), None).expect("pipe slot");
    io::down(
        &mut face.io,
        &env,
        Request::Stream { stream: timed, down: Down::Demand { read: Read::Nothing, room: 1 } },
        &mut face.submissions,
    );
    face.io.reclaim();
    face.up(Time::ZERO, Wall::EPOCH);
    drop(face.events.pop());
    io::down(
        &mut face.io,
        &env,
        Request::Stream { stream: timed, down: Down::Send(Box::from(b"x".as_slice())) },
        &mut face.submissions,
    );
    let write = face.submissions.pop().expect("append write");
    io::down(&mut face.io, &env, Request::Usage { owner: OWNER }, &mut face.submissions);
    let usage = face.submissions.pop().expect("usage query");
    face.completions.push(kernel::Complete {
        op: usage.op,
        kind: usage.kind,
        result: Ok(kernel::Done::Usage(kernel::Usage::ZERO)),
    });
    face.submissions.push(write);
    io::down(
        &mut face.io,
        &env,
        Request::Stream { stream: ready, down: Down::Demand { read: Read::Nothing, room: 1 } },
        &mut face.submissions,
    );
    face.io.reclaim();
    let now = Time::ZERO.saturating_add(DEADLINE);
    face.up(now, Wall::EPOCH);
    let blocked = face.events.is_empty() && face.io.is_ready() && face.io.is_due(now) && !face.completions.is_empty();
    let write = face.submissions.pop().expect("kernel takes the held write");
    face.up(now, Wall::EPOCH);
    let events: Vec<_> = face.events.iter().collect();
    assert!(blocked, "all stages wait at the kernel edge");
    assert_eq!(
        events,
        [
            &Event::Stream { owner: ready, up: Up::Room },
            &Event::Usage { owner: OWNER, usage: io::Measured::Read(kernel::Usage::ZERO) },
            &Event::Stream { owner: timed, up: Up::Failed(skein_lib::stream::Fault::Other) }
        ]
    );
    assert!(!face.io.is_ready() && face.completions.is_empty() && !face.io.is_due(now));
    assert_eq!(
        face.submissions.pop().expect("deadline cancels the write").kind,
        kernel::Op::Cancel { target: write.op }
    );
}
