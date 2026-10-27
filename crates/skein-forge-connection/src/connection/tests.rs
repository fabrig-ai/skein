//! The complete GitHub handoff, held at simultaneous peak and released at its terminals.
use super::*;
use crate::tests::fixtures::{self as f, ConnectionHarness};
use github::plans::plan::{Comparison, Exchange as GitHubExchange, Retention};

fn peak(h: &mut ConnectionHarness) {
    let size = usize::try_from(h.env.limits.string_bytes).expect("string bound");
    let request = github::request::Request::Raw {
        owner: f::filled(size, b'o'),
        name: f::filled(size, b'n'),
        path: f::filled(size, b'p'),
        commit: [0; 32],
    };
    h.connection
        .start(
            &h.env,
            &h.forge,
            f::USER_AGENT,
            Exchange::GitHub {
                exchange: GitHubExchange {
                    request,
                    retention: Some(Retention::Whole { bytes: f::shared().answer_bytes }),
                    comparison: Comparison::MergeBase,
                },
                authorization: Some(Secret::new(Box::from(b"Bearer test-token".as_slice()))),
            },
        )
        .expect("maximum exchange encodes");
}
fn retained(h: &ConnectionHarness) -> &GitHubExchange {
    match h.connection.request.as_ref().expect("retained request") {
        Request::GitHub(exchange) => exchange,
        Request::Forgejo(_) => unreachable!("GitHub connection"),
    }
}
#[test]
fn github_exchange_handoff_fits_on_every_connection_at_peak() {
    let count = f::limits().connections + f::github_settings().forges.len();
    let mut connections = List::with_capacity(count);
    for _ in 0..count {
        let mut h = ConnectionHarness::github();
        peak(&mut h);
        connections.push(h).expect("all call and own connections");
    }
    let mut heap = 0;
    for h in &connections {
        let exchange = retained(h);
        assert_eq!(exchange.retention, Some(Retention::Whole { bytes: f::shared().answer_bytes }));
        assert_eq!(exchange.comparison, Comparison::MergeBase);
        let github::request::Request::Raw { owner, name, path, .. } = &exchange.request else { unreachable!("raw") };
        assert_eq!(owner.len(), usize::try_from(h.env.limits.string_bytes).expect("bound"));
        assert_eq!((name.len(), path.len()), (owner.len(), owner.len()));
        heap += size_of_val(exchange);
    }
    // Request payloads already have the per-connection body/target term. The
    // allocation is one 136-byte exchange envelope per connection.
    assert_eq!(heap, usize::try_from(count).expect("connections") * 136);
    let bound = limits::worst_case(&f::limits(), &f::shared(), limits::Forges { forgejo: 0, github: 1 }, 0)
        .expect("complete bound");
    assert!(bound >= u64::try_from(heap).expect("handoff heap"));
}
#[derive(Clone, Copy, Debug)]
enum Terminal {
    Loss,
    Close,
    Replacement,
    Answer,
}
#[test]
fn github_exchange_handoff_drops_on_loss_close_replacement_and_answer() {
    for terminal in [Terminal::Loss, Terminal::Close, Terminal::Replacement, Terminal::Answer] {
        let mut h = ConnectionHarness::github();
        peak(&mut h);
        assert_eq!(retained(&h).comparison, Comparison::MergeBase);
        match terminal {
            Terminal::Loss => h.connection.failed(Failure::Seen),
            Terminal::Close => h.connection.close(true, &mut h.io),
            Terminal::Replacement => {
                // A completed request is replaced on the reusable connection.
                h.github_answer(&f::ok(b""));
                h.connection
                    .start(
                        &h.env,
                        &h.forge,
                        f::USER_AGENT,
                        Exchange::GitHub {
                            exchange: GitHubExchange {
                                request: github::request::Request::Raw {
                                    owner: Box::from(b"a".as_slice()),
                                    name: Box::from(b"b".as_slice()),
                                    path: Box::from(b"c".as_slice()),
                                    commit: [0; 32],
                                },
                                retention: Some(Retention::Head { bytes: 1 }),
                                comparison: Comparison::Full,
                            },
                            authorization: Some(Secret::new(Box::from(b"Bearer test-token".as_slice()))),
                        },
                    )
                    .expect("replacement request");
                let exchange = retained(&h);
                assert_eq!(exchange.retention, Some(Retention::Head { bytes: 1 }));
                assert_eq!(exchange.comparison, Comparison::Full);
                h.connection.close(true, &mut h.io);
            }
            Terminal::Answer => {
                h.github_answer(&f::ok(b""));
            }
        }
        assert!(h.connection.request.is_none(), "{terminal:?}: handoff released");
    }
}

#[test]
fn github_followed_request_sends_exact_bytes_without_authorization_or_range() {
    let redirect = github::redirect::parse(b"http://logs.example.test/job%2Fpart?signature=invented", 128, 1024)
        .expect("invented log origin");
    let mut h = ConnectionHarness::github();
    h.forge.transport = Transport::Plaintext { name: Some(redirect.origin.host.clone()) };
    h.forge.address = skein_io::Addr::Ip(([127, 0, 0, 1], redirect.origin.port).into());
    h.connection
        .start(
            &h.env,
            &h.forge,
            f::USER_AGENT,
            Exchange::GitHub {
                exchange: GitHubExchange {
                    request: github::redirect::request(&redirect),
                    retention: Some(Retention::Tail { bytes: 8 }),
                    comparison: Comparison::Full,
                },
                authorization: None,
            },
        )
        .expect("followed request encodes");
    h.github_answer(&f::ok(b"log"));
    assert_eq!(h.sent.as_ref(), b"GET /job%2Fpart?signature=invented HTTP/1.1\r\nHost: logs.example.test\r\nUser-Agent: kit/0.0.0\r\nAccept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2026-03-10\r\n\r\n");
}
