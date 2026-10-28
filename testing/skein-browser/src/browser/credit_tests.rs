//! Queued commands consume stream credit once, in order.

use super::{Browser, Phase, Queued, fire, up};
use crate::Limits;
use crate::boundary::{Below, Down};
use skein_lib::stream;
use skein_lib::{Env, Queue, Time, Token, Wall, bytes};

#[test]
fn each_command_waits_for_fresh_stream_credit() {
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: Limits::default() };
    let mut browser = Browser::new(Token::new(1), &env.limits);
    browser.phase = Phase::Ready;
    for (id, bytes) in [(0, b"first".as_slice()), (1, b"second".as_slice())] {
        browser.outgoing.push_back(Queued { id, bytes: bytes::copy_of(bytes), read: None, person: None });
    }
    let mut above = Queue::uncapped();
    let mut below = Queue::uncapped();
    fire(&mut browser, &env, env.now, &mut above, &mut below);
    let without_credit = below.len();
    up(&mut browser, &env, Below::Commands(stream::Up::Room), &mut above, &mut below);
    fire(&mut browser, &env, env.now, &mut above, &mut below);
    let awaiting_credit = below.len();
    up(&mut browser, &env, Below::Commands(stream::Up::Room), &mut above, &mut below);
    assert_eq!(without_credit, 0);
    assert_eq!(awaiting_credit, 2, "one send and its fresh credit demand");
    assert_eq!(below.pop(), Some(Down::Commands(stream::Down::Send(bytes::copy_of(b"first")))));
    assert_eq!(
        below.pop(),
        Some(Down::Commands(stream::Down::Demand {
            read: stream::Read::Nothing,
            room: env.limits.command.saturating_add(1)
        }))
    );
    assert_eq!(below.pop(), Some(Down::Commands(stream::Down::Send(bytes::copy_of(b"second")))));
    assert_eq!(
        below.pop(),
        Some(Down::Commands(stream::Down::Demand {
            read: stream::Read::Nothing,
            room: env.limits.command.saturating_add(1)
        }))
    );
    assert!(below.is_empty() && browser.outgoing.is_empty());
}
