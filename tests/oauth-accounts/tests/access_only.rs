use skein_lib::{CryptoRng, Duration, Env, List, Queue, Time, Wall, bytes};
use skein_oauth as oauth;
use skein_oauth_accounts as accounts;
use skein_oauth_accounts_world::sign_in::{Story, registration};
use skein_oauth_accounts_world::world::limits;

fn time(seconds: u64) -> Time {
    Time::from_nanos(Duration::from_secs(seconds).as_nanos())
}
fn wall(seconds: u64) -> Wall {
    Wall::from_nanos(Duration::from_secs(seconds).as_nanos())
}
fn step(client: &mut oauth::Client, event: oauth::Event) -> Option<oauth::Request> {
    let mut out = Queue::uncapped();
    client.step(event, &mut out);
    out.pop()
}
fn signed_in() -> oauth::SavedToken {
    let mut client = oauth::Client::new(limits().client).expect("client");
    let registration = registration(Story::Public);
    let uri = registration.redirect_uri.clone();
    let _visit = step(
        &mut client,
        oauth::Event::SignIn {
            registration,
            key: 0,
            generation: 0,
            state: bytes::copy_of(b"0123456789abcdef"),
            verifier: Some(bytes::copy_of(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")),
            nonce: None,
            port: None,
            issued_client_id: None,
            id_token: None,
            subject: None,
            now: time(0),
        },
    );
    let Some(oauth::Request::Http(post)) = step(
        &mut client,
        oauth::Event::Redirected {
            client_id: None,
            uri,
            state: bytes::copy_of(b"0123456789abcdef"),
            code: Some(bytes::copy_of(b"code")),
            error: None,
            now: time(0),
        },
    ) else {
        panic!("token request")
    };
    let Some(oauth::Request::Tokens { record, .. }) = step(
        &mut client,
        oauth::Event::Http(oauth::HttpResponse {
            id: post.id,
            status: 200,
            body: bytes::copy_of(br#"{"access_token":"access","expires_in":30}"#),
            retry_after: Duration::ZERO,
            evidence: oauth::HttpEvidence::Response,
            now: time(0),
            wall: wall(0),
        }),
    ) else {
        panic!("sign-in record")
    };
    record
}

struct Owner {
    component: accounts::Component,
    env: Env<accounts::Limits>,
    up: Queue<accounts::Event>,
    io: Queue<accounts::Lower>,
    files: Queue<accounts::FileLower>,
}
impl Owner {
    fn new(wanted: bool) -> Self {
        let profile = limits();
        let mut configured = List::with_capacity(1);
        configured
            .push(accounts::Account::SignIn {
                registration: registration(Story::Public),
                endpoint: accounts::Endpoint {
                    address: ([127, 0, 0, 1], 31000).into(),
                    transport: accounts::Transport::Plaintext,
                },
                keeper: accounts::Keeper::Owner { kept: Some(signed_in()) },
                shared: false,
                origins: List::with_capacity(0),
            })
            .ok()
            .expect("account");
        let mut owner = Self {
            component: accounts::Component::new(
                configured,
                &profile,
                CryptoRng::new(&skein_world::crypto_seed(17), 0).expect("seed"),
            )
            .expect("component"),
            env: Env { now: time(0), wall: wall(0), limits: profile },
            up: Queue::uncapped(),
            io: Queue::uncapped(),
            files: Queue::uncapped(),
        };
        owner.ask(accounts::Request::Want { account: 0, wanted });
        owner
    }
    fn holding(wanted: bool) -> Self {
        let mut owner = Self::new(wanted);
        owner.ask(accounts::Request::Grant { account: 0 });
        owner.granted(30);
        owner
    }
    fn close(&mut self) {
        self.ask(accounts::Request::Close);
        assert!(matches!(self.up.pop(), Some(accounts::Event::Closed)));
        assert!(self.io.is_empty() && self.files.is_empty());
    }
    fn ask(&mut self, request: accounts::Request) {
        self.component.down(&self.env, request, &mut self.up, &mut self.io, &mut self.files);
    }
    fn at(&mut self, seconds: u64) {
        self.env.now = time(seconds);
        self.env.wall = wall(seconds);
        self.component.fire(&self.env, &mut self.up, &mut self.io, &mut self.files);
        assert!(self.io.is_empty() && self.files.is_empty(), "access-only sign-ins never refresh");
    }
    fn granted(&mut self, valid: u64) {
        let Some(accounts::Event::Granted { generation, valid: remaining, token, .. }) = self.up.pop() else {
            panic!("granted")
        };
        assert_eq!((generation, remaining), (1, Duration::from_secs(valid)));
        assert!(token.as_ref() == b"access");
    }
}

#[test]
fn an_access_only_sign_in_expires_without_refreshing() {
    for wanted in [false, true] {
        let mut owner = Owner::holding(wanted);
        owner.ask(accounts::Request::Release { account: 0 });
        assert_eq!(owner.component.next_deadline(), Some(time(20)));
        owner.at(20);
        assert!(matches!(owner.up.pop(), Some(accounts::Event::Expiring { account: 0, generation: 1 })));
        owner.at(21);
        assert!(owner.up.is_empty(), "expiring is announced once");
        owner.ask(accounts::Request::Grant { account: 0 });
        owner.granted(9);
        owner.ask(accounts::Request::Release { account: 0 });
        owner.at(31);
        owner.ask(accounts::Request::Grant { account: 0 });
        assert!(matches!(
            owner.up.pop(),
            Some(accounts::Event::Failed { ends: accounts::Ends::Grant, failure: accounts::Failure::Expired, .. })
        ));
        owner.close();
    }
}

#[test]
fn a_held_access_only_sign_in_fails_at_expiry() {
    let mut owner = Owner::holding(false);
    owner.at(20);
    assert!(matches!(owner.up.pop(), Some(accounts::Event::Expiring { generation: 1, .. })));
    assert_eq!(owner.component.next_deadline(), Some(time(30)));
    owner.at(30);
    assert!(matches!(owner.up.pop(), Some(accounts::Event::Failed { failure: accounts::Failure::Expired, .. })));
    assert_eq!(owner.component.next_deadline(), None);
    owner.close();
}
