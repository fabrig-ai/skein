use skein_oauth_accounts::Failure;
use skein_oauth_accounts_world::{
    sign_out::{self, Story},
    world::Fact,
};
use skein_world::Memory;
#[global_allocator]
static HEAP: skein_heap::Counting = skein_heap::Counting;
#[test]
fn a_sign_out_settles_for_both_keepers() {
    for (seed, (story, revoked)) in
        (101..).zip([(Story::Confirmed, true), (Story::Unconfirmed, false), (Story::NoRevocation, false)])
    {
        let settled = [Fact::SignedOut(revoked), Fact::Closed];
        for private in [false, true] {
            let outcome = sign_out::run(seed, story, private, Memory::Checked);
            assert_eq!(sign_out::facts(&outcome.procs), &settled, "{story:?}");
        }
        assert_eq!(sign_out::protocol(seed, story).0, settled, "{story:?} on streams");
    }
}
#[test]
fn a_sign_out_under_close_and_abort_settles_once() {
    for (story, terminal) in [
        (Story::Close, Fact::SignedOut(true)),
        (Story::Abort, Fact::Failed(Failure::Exchange(skein_oauth::Failure::Cancelled))),
    ] {
        let expected = [terminal, Fact::Closed];
        for private in [false, true] {
            let outcome = sign_out::run(103, story, private, Memory::Checked);
            assert_eq!(sign_out::facts(&outcome.procs), &expected, "{story:?}");
        }
        assert_eq!(sign_out::protocol(103, story).0, expected);
    }
}

use skein_lib::stream::{Down, Read, Up};
use skein_lib::{CryptoRng, Duration, Env, List, Queue, Time, Token, Wall, bytes};
use skein_oauth as oauth;
use skein_oauth_accounts as accounts;

const ACCOUNT: u32 = 0;
const SOCKET: Token = Token::new(100);
// Enough owner turns to route every pending request without advancing time.
const DRAIN_TURNS: usize = 64;
const CREDIT_PAUSE: usize = 16;
const ONE_TERMINAL: usize = 1;

struct ExchangeOwner {
    component: accounts::Component,
    env: Env<accounts::Limits>,
    up: Queue<accounts::Event>,
    io: Queue<accounts::Lower>,
    files: Queue<accounts::FileLower>,
}
impl ExchangeOwner {
    fn new() -> Self {
        let profile = skein_oauth_accounts_world::world::limits();
        let registration =
            skein_oauth_accounts_world::sign_in::registration(skein_oauth_accounts_world::sign_in::Story::Confidential);
        let mut configured = List::with_capacity(1);
        configured
            .push(accounts::Account::SignIn {
                registration,
                endpoint: accounts::Endpoint {
                    address: ([127, 0, 0, 1], 31000).into(),
                    transport: accounts::Transport::Plaintext,
                },
                keeper: accounts::Keeper::Owner {
                    kept: Some(oauth::SavedToken {
                        key: ACCOUNT,
                        generation: 0,
                        access_token: bytes::copy_of(b"old"),
                        refresh_token: Some(bytes::copy_of(b"seed")),
                        metadata: None,
                        expires_at: Wall::from_nanos(Duration::from_secs(5).as_nanos()),
                        issued_client_id: None,
                        granted_scopes: None,
                        subject: None,
                        id_token: None,
                        earliest_refresh_at: None,
                    }),
                },
                shared: false,
                origins: List::with_capacity(0),
            })
            .ok()
            .expect("one signed-in account");
        Self {
            component: accounts::Component::new(
                configured,
                &profile,
                CryptoRng::new(&skein_world::crypto_seed(17), 0).expect("world seed"),
            )
            .expect("refreshable account"),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: profile },
            up: Queue::uncapped(),
            io: Queue::uncapped(),
            files: Queue::uncapped(),
        }
    }
    fn ask(&mut self, request: accounts::Request) {
        self.component.down(&self.env, request, &mut self.up, &mut self.io, &mut self.files);
    }
    fn socket(&mut self, event: accounts::LowerEvent) {
        self.component.up(&self.env, event, &mut self.up, &mut self.io, &mut self.files);
    }
    fn fire(&mut self) {
        self.component.fire(&self.env, &mut self.up, &mut self.io, &mut self.files);
        self.component.reclaim();
    }
    fn refreshing(&mut self) -> Token {
        self.ask(accounts::Request::Grant { account: ACCOUNT });
        self.fire();
        let Some(accounts::Lower::Connect { owner, .. }) = self.io.pop() else { panic!("refresh opens its socket") };
        self.socket(accounts::LowerEvent::Connecting { owner, socket: SOCKET });
        self.socket(accounts::LowerEvent::Connected { owner });
        self.fire();
        self.fire();
        owner
    }
}

#[test]
fn a_cancelled_exchange_sends_none_of_its_queued_output_and_signs_out_clean() {
    let mut world = ExchangeOwner::new();
    let owner = world.refreshing();
    let held_credit = world.io.pop();
    // Withhold the head's credit; the upload demand waits beside its head.
    for _ in 0..CREDIT_PAUSE {
        world.fire();
    }
    // Grant without routing: HTTP queues its head Send and response Demand.
    // Cancel supersedes both before either can reach the socket.
    world.socket(accounts::LowerEvent::Stream { owner, up: Up::Room });
    world.ask(accounts::Request::Cancel { account: ACCOUNT });
    world.ask(accounts::Request::Cancel { account: ACCOUNT });
    let mut cancelled = Vec::new();
    let mut aborted = 0;
    let mut stale_output = Vec::new();
    for _ in 0..DRAIN_TURNS {
        world.fire();
        while let Some(event) = world.up.pop() {
            cancelled.push(event);
        }
        while let Some(request) = world.io.pop() {
            match request {
                accounts::Lower::Abort { entity } => {
                    aborted += 1;
                    assert_eq!(entity, SOCKET);
                    // A late credit answer cannot restart cancelled output.
                    world.socket(accounts::LowerEvent::Stream { owner, up: Up::Room });
                    world.socket(accounts::LowerEvent::Closed { owner });
                }
                request @ (accounts::Lower::Listen { .. }
                | accounts::Lower::Connect { .. }
                | accounts::Lower::Bind { .. }
                | accounts::Lower::Reject { .. }
                | accounts::Lower::Stream { .. }
                | accounts::Lower::Output { .. }
                | accounts::Lower::Spawn { .. }
                | accounts::Lower::Window { .. }
                | accounts::Lower::Usage { .. }
                | accounts::Lower::Signal { .. }
                | accounts::Lower::Close { .. }) => stale_output.push(request),
            }
        }
    }
    assert!(
        matches!(held_credit, Some(accounts::Lower::Stream { stream: SOCKET, down: Down::Demand { read: Read::Nothing, room } }) if room > 0)
    );
    assert!(stale_output.is_empty(), "no queued output reaches the cancelled socket: {stale_output:?}");
    assert_eq!(aborted, ONE_TERMINAL);
    assert!(
        cancelled
            == [accounts::Event::Failed {
                account: ACCOUNT,
                ends: accounts::Ends::Grant,
                failure: Failure::Exchange(oauth::Failure::Cancelled),
            }]
    );
    world.ask(accounts::Request::SignOut { account: ACCOUNT });
    for _ in 0..DRAIN_TURNS {
        world.fire();
    }
    let mut signed_out = Vec::new();
    while let Some(event) = world.up.pop() {
        signed_out.push(event);
    }
    assert!(signed_out == [accounts::Event::SignedOut { account: ACCOUNT, revoked: false }]);
    assert!(world.io.is_empty() && world.files.is_empty());
    world.ask(accounts::Request::SignIn { account: ACCOUNT, subject: None });
    for _ in 0..DRAIN_TURNS {
        world.fire();
    }
    let mut visits = Vec::new();
    while let Some(event) = world.up.pop() {
        visits.push(event);
    }
    assert!(matches!(visits.as_slice(), [accounts::Event::Visit { account: ACCOUNT, .. }]));
    assert!(world.io.is_empty() && world.files.is_empty(), "the next exchange carries no old socket output");
    world.ask(accounts::Request::Cancel { account: ACCOUNT });
    world.ask(accounts::Request::Close);
    for _ in 0..DRAIN_TURNS {
        world.fire();
    }
    assert!(
        world.up.pop()
            == Some(accounts::Event::Failed {
                account: ACCOUNT,
                ends: accounts::Ends::SignIn,
                failure: Failure::Exchange(oauth::Failure::Cancelled),
            })
    );
    assert!(world.up.pop() == Some(accounts::Event::Closed));
    assert!(world.up.is_empty() && world.io.is_empty() && world.files.is_empty());
}
