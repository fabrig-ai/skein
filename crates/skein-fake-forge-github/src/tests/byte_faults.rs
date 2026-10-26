//! Byte faults surround a real HTTP exchange and the shared domain's write.
use super::fixtures as f;
use crate::{Faults, server};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_lib::{
    Env, Intake, List, Queue, ReplyTo, Token,
    stream::{Down, Up},
};

fn exchange(faults: Faults, seed: u64) -> (f::Harness, Box<[u8]>, u32) {
    let config = crate::Config { faults, ..f::config() };
    let mut h = f::Harness::with(f::limits(), config);
    // Seed the byte draws independently of domain setup.
    h.service.rng = skein_lib::Rng::new(seed);
    fake::grant(&mut h.domain, b"acme/widgets", f::BOT, a::Permission::Write);
    let token = f::bearer(h.mint(1, false).token.expose());
    let body = br#"{"title":"faulted issue","body":"body"}"#;
    let wire = f::parts(&[
        b"POST /repos/acme/widgets/issues HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: example-app\r\nX-GitHub-Api-Version: 2026-03-10\r\nAuthorization: ",
        token.expose(), b"\r\nConnection: close\r\nContent-Length: 39\r\n\r\n", body,
    ]);
    let mut input = Intake::with_capacity(4096);
    input.append(&wire).unwrap();
    let original = input.len();
    let env = Env { limits: h.limits, now: h.env.now, wall: h.now };
    let mut server = server::Server::new(Token::new(77), &h.limits).unwrap();
    let mut above = Queue::with_capacity(server::MAX_UP);
    let mut below = Queue::with_capacity(server::MAX_DOWN);
    let mut output = List::with_capacity(h.limits.response);
    server::start(&mut server, &mut h.service, &env, &mut above, &mut below);
    let mut close = false;
    for _ in 0..512_u32 {
        if close && below.is_empty() {
            break;
        }
        drop(server.take_head());
        if let Some(event) = above.pop() {
            match event {
                server::Event::Rate { key, limit, window } => {
                    let result = fake::rate(&mut h.domain, &h.env, key, limit, window);
                    server::admitted(&mut server, &mut h.service, &env, result, &mut above, &mut below);
                }
                server::Event::Domain(event) => {
                    h.calls = h.calls.checked_add(1).unwrap();
                    let result = h.drive(event);
                    server::down(
                        &mut server,
                        &mut h.service,
                        &env,
                        fake::Request::Reply { to: ReplyTo::new(Token::new(77)), result },
                        &h.domain,
                        &mut above,
                        &mut below,
                    );
                }
                server::Event::Close => {
                    close = true;
                }
                server::Event::Closed => panic!("host closes only after Close"),
            }
        } else if let Some(request) = below.pop() {
            match request {
                Down::Demand { read, room } => {
                    let event = if room > 0 {
                        Some(Up::Room)
                    } else {
                        let Some(bytes) = input.meet(read) else { continue };
                        Some(Up::Bytes(bytes))
                    };
                    if let Some(event) = event {
                        server::up(&mut server, &mut h.service, &env, event, &mut above, &mut below);
                    }
                }
                Down::Send(chunk) => {
                    for byte in chunk {
                        output.push(byte).unwrap();
                    }
                }
                Down::Finish => {}
            }
        } else {
            server::resume(&mut server, &mut h.service, &env, &mut above, &mut below);
        }
    }
    assert!(close, "fault or Connection: close reaches one Close");
    server::closed(&mut server, &mut h.service, &env, &mut above, &mut below);
    let Some(server::Event::Closed) = above.pop() else { panic!("one Closed") };
    let read = original.checked_sub(input.len()).unwrap();
    (h, output.into_boxed(), read)
}
#[test]
fn github_fake_byte_faults_reset_drop_and_server_error() {
    for (faults, applied, status) in [
        (Faults { reset: 1000, drop: 0, server_error: 0 }, false, None),
        (Faults { reset: 0, drop: 1000, server_error: 0 }, true, None),
        (Faults { reset: 0, drop: 0, server_error: 1000 }, true, Some(b"HTTP/1.1 500 ".as_slice())),
        (Faults { reset: 0, drop: 0, server_error: 0 }, true, Some(b"HTTP/1.1 201 ".as_slice())),
    ] {
        let (h, output, read) = exchange(faults, 23);
        let result = h.domain.inspect(&h.env.limits, b"acme/widgets", &a::Read::Item { number: 1, after: 0 });
        if applied {
            let Ok(a::Answer::Item { item, .. }) = result else { panic!("write applied before reply loss") };
            assert_eq!(item.title.as_ref(), b"faulted issue");
            assert_eq!(h.calls, 1);
            assert!(read > 0);
        } else {
            assert_eq!(result, Err(a::Error::Missing(a::What::Item)));
            assert_eq!(h.calls, 0);
            assert_eq!(read, 0, "reset closes before a read demand");
        }
        match status {
            Some(prefix) => {
                assert!(output.starts_with(prefix));
                assert!(skein_lib::bytes::find(&output, b"X-RateLimit-Used: 1\r\n").is_some());
            }
            None => assert!(output.is_empty(), "no reply bytes after reset or drop"),
        }
        let (replayed, replay, replay_read) = exchange(faults, 23);
        assert_eq!((replayed.calls, replay_read), (h.calls, read));
        assert_eq!(replay, output, "one seed replays byte faults");
    }
}
#[test]
fn github_fake_byte_fault_chances_are_bounded() {
    for faults in [
        Faults { reset: 1001, drop: 0, server_error: 0 },
        Faults { reset: 0, drop: 1001, server_error: 0 },
        Faults { reset: 0, drop: 0, server_error: 1001 },
    ] {
        assert_eq!(
            crate::Service::new(crate::Config { faults, ..f::config() }, &f::limits(), 1).unwrap_err(),
            crate::Problem::Config
        );
    }
}
