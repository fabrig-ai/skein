use skein_heap::{Counting, Meter};
use skein_io::file;
use skein_lib::Token;
use skein_lib::{Duration, Env, List, Queue, Time, Wall, bytes};
use skein_oauth::{ClientLimits, SavedToken};
use skein_oauth_accounts::{
    Account, Component, Endpoint, FileLower, Keeper, Limits, Request, Transport, worst_case,
};

#[global_allocator]
static HEAP: Counting = Counting;

#[test]
fn every_account_holds_a_maximal_record_within_the_heap_bound() {
    let limits = shared_memory_limits();
    let bound = worst_case(&limits).expect("heap bound");
    let meter = Meter::new();
    meter.start();
    let configured = shared_memory_accounts(&limits);
    let crypto = skein_lib::CryptoRng::new(&skein_world::crypto_seed(17), 0).expect("32-byte world seed");
    let mut component = Component::new(configured, &limits, crypto).expect("component");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut out = Queue::uncapped();
    let mut shared_reads = Queue::with_capacity(limits.accounts);
    for account in 0..limits.accounts {
        component.down(
            &env,
            Request::Grant { account },
            &mut out,
            &mut Queue::uncapped(),
            &mut Queue::with_capacity(1),
        );
        assert!(out.pop() == Some(skein_oauth_accounts::Event::Lock { account }));
        shared_reads.push(Request::Locked {
            account,
            answer: skein_oauth_accounts::Locked::Held { record: Some(shared_memory_record(account)) },
        });
    }
    assert_eq!(shared_reads.len(), limits.accounts, "every pending lock owns its bounded read");
    let measured = meter.end();
    meter.check(measured, bound, &"OAuth shared accounts and their reads");
}

#[test]
fn every_refresh_slot_holds_its_maximal_records_and_tls_connection() {
    use skein_oauth_accounts::{Endpoint, Keeper, Transport};
    let mut limits = skein_oauth_accounts_world::world::limits();
    limits.accounts = 4;
    limits.exchanges = 4;
    limits.io.sockets = 6;
    let trust = skein_tls_world::pki::client(&[]);
    let bound = worst_case(&limits).expect("component heap bound");
    let meter = Meter::new();
    meter.start();
    let mut configured = List::with_capacity(limits.accounts);
    for account in 0..limits.accounts {
        let registration = skein_oauth::Registration {
            authorization_url: bytes::copy_of(b"https://localhost/authorize"),
            token_endpoint: bytes::copy_of(b"https://localhost/token"),
            client_id: bytes::copy_of(&[b'c'; 64]),
            redirect_uri: bytes::copy_of(b"http://localhost:31234/callback"),
            scope: bytes::copy_of(&[b's'; 64]),
            wire: skein_oauth::WireFormat::Form,
            client_secret: None,
            pkce_for_confidential: false,
            metadata_claim: None,
            terms: None,
        };
        assert!(
            configured
                .push(Account::SignIn {
                    origins: List::with_capacity(0),
                    shared: false,
                    registration,
                    endpoint: Endpoint {
                        address: "127.0.0.1:31000".parse().expect("loopback"),
                        transport: Transport::Tls { server_name: skein_tls_world::pki::name(), trust: trust.clone() },
                    },
                    keeper: Keeper::Owner {
                        kept: Some(SavedToken {
                            key: account,
                            generation: 1,
                            access_token: bytes::copy_of(&[b'a'; 256]),
                            refresh_token: Some(bytes::copy_of(&[b'r'; 256])),
                            metadata: Some(bytes::copy_of(&[b'm'; 256])),
                            expires_at: Wall::from_nanos(5_000_000_000),
                            issued_client_id: None,
                            granted_scopes: None,
                            subject: None,
                            id_token: None,
                            earliest_refresh_at: None,
                        })
                    },
                })
                .is_ok()
        );
    }
    let crypto = skein_lib::CryptoRng::new(&skein_world::crypto_seed(17), 0).expect("32-byte world seed");
    let mut component = Component::new(configured, &limits, crypto).expect("configured slots");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut above = Queue::uncapped();
    let mut io = Queue::uncapped();
    for account in 0..limits.accounts {
        component.down(&env, Request::Grant { account }, &mut above, &mut io, &mut Queue::with_capacity(1));
        assert!(above.is_empty(), "grant waits for a kept refresh");
    }
    let mut connects = 0;
    for _ in 0..64 {
        if !component.has_work() {
            break;
        }
        component.fire(&env, &mut above, &mut io, &mut Queue::with_capacity(1));
        assert!(above.is_empty());
        while let Some(request) = io.pop() {
            assert!(matches!(request, skein_io::Request::Connect { .. }));
            connects += 1;
        }
    }
    assert_eq!(connects, limits.exchanges, "one TLS connection in every slot");
    let measured = meter.end();
    meter.check(measured, bound, &"all OAuth TLS exchange slots");
}

#[test]
fn every_listener_slot_keeps_a_bounded_partial_head_within_the_heap_bound() {
    let mut limits = skein_oauth_accounts_world::world::limits();
    limits.accounts = 4;
    limits.exchanges = 4;
    limits.listeners = 4;
    limits.io.sockets = 12;
    let keys = skein_oauth_accounts_world::sign_in::key_set();
    limits.client.keys = 1;
    limits.client.key_set_bytes = u32::try_from(keys.len()).expect("key set length");
    let response =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", keys.len()).into_bytes();
    let mut response = response;
    response.extend_from_slice(&keys);
    let bound = worst_case(&limits).expect("all listener bounds");
    let meter = Meter::new();
    meter.start();
    let configured = listener_accounts(&limits);
    let crypto = skein_lib::CryptoRng::new(&skein_world::crypto_seed(17), 0).expect("32-byte world seed");
    let mut component = Component::new(configured, &limits, crypto).expect("component");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    drive_partial_heads(&mut component, &env, response);
    let measured = meter.end();
    meter.check(measured, bound, &"every OAuth listener and partial head");
}

const LISTENER_TURNS: usize = 2048;

fn drive_partial_heads(component: &mut Component, env: &Env<Limits>, response: Vec<u8>) {
    let mut above = Queue::with_capacity(64);
    let mut below = Queue::with_capacity(64);
    for account in 0..env.limits.accounts {
        component.down(
            env,
            Request::SignIn { account, subject: None },
            &mut above,
            &mut below,
            &mut Queue::with_capacity(1),
        );
    }
    for account in 0..env.limits.accounts {
        let mut location = b"https://application.example/".to_vec();
        location.resize(skein_oauth_accounts::RETURN_BYTES, b'x');
        component.down(
            env,
            Request::Return { account, location: location.into_boxed_slice() },
            &mut above,
            &mut below,
            &mut Queue::with_capacity(1),
        );
    }
    assert!(above.is_empty(), "returns have no event");
    let mut sockets = ListenerSockets::new(response);
    let mut visits = 0;
    for _ in 0..LISTENER_TURNS {
        if component.has_work() {

            component.fire(env, &mut above, &mut below, &mut Queue::with_capacity(1));
        }
        while let Some(event) = above.pop() {
            assert!(matches!(event, skein_oauth_accounts::Event::Visit { .. }));
            visits += 1;
        }
        while let Some(request) = below.pop() {
            sockets.request(component, env, request, &mut above, &mut below);
        }
        sockets.answer(component, env, &mut above, &mut below);
    }
    assert_eq!(sockets.callbacks.len(), usize::try_from(env.limits.accounts).expect("account count"));
    assert_eq!(visits, env.limits.accounts);
    assert!(sockets.web.is_empty());
    assert!(component.next_deadline().is_some());
}

struct ListenerSockets {
    callbacks: Vec<(Token, Token)>,
    wires: Vec<(Token, skein_world::stream::Wire)>,
    web: Vec<(Token, Token, skein_world::stream::Wire)>,
    response: Vec<u8>,
}

impl ListenerSockets {
    fn new(response: Vec<u8>) -> Self {
        Self { callbacks: Vec::with_capacity(4), wires: Vec::with_capacity(4), web: Vec::with_capacity(4), response }
    }

    fn request(
        &mut self,
        component: &mut Component,
        env: &Env<Limits>,
        request: skein_io::Request,
        above: &mut Queue<skein_oauth_accounts::Event>,
        below: &mut Queue<skein_io::Request>,
    ) {
        use skein_io::{Event as IoEvent, Request as IoRequest};
        use skein_world::stream::Wire;
        match request {
            IoRequest::Connect { owner, .. } => {
                let socket = Token::new(100 + u64::try_from(self.web.len()).expect("self.web index"));
                component.up(env, IoEvent::Connecting { owner, socket }, above, below, &mut Queue::with_capacity(1));
                component.up(env, IoEvent::Connected { owner }, above, below, &mut Queue::with_capacity(1));
                let mut wire = Wire::new(socket.raw());
                wire.write(&self.response);
                self.web.push((owner, socket, wire));
            }
            IoRequest::Close { entity } => {
                let at = self.web.iter().position(|(_, socket, _)| *socket == entity).expect("issuer socket");
                let (owner, _, _) = self.web.remove(at);
                component.up(env, IoEvent::Closed { owner }, above, below, &mut Queue::with_capacity(1));
            }
            IoRequest::Listen { owner, addr } => {
                let index = u64::try_from(self.wires.len()).expect("four listeners");
                let listener = Token::new(20 + index);
                let socket = Token::new(40 + index);
                component.up(
                    env,
                    IoEvent::Listening { owner, listener, addr },
                    above,
                    below,
                    &mut Queue::with_capacity(1),
                );
                component.up(
                    env,
                    IoEvent::Accepted { owner, socket, peer: addr },
                    above,
                    below,
                    &mut Queue::with_capacity(1),
                );
                let mut wire = Wire::new(index);
                let mut partial = b"GET /callback?state=".to_vec();
                partial.resize(usize::try_from(env.limits.server.head - 64).expect("head size"), b'x');
                wire.write(&partial);
                self.wires.push((socket, wire));
            }
            IoRequest::Bind { socket, owner } => self.callbacks.push((socket, owner)),
            IoRequest::Stream { stream, down } => {
                if let Some((_, wire)) = self.wires.iter_mut().find(|(socket, _)| *socket == stream) {
                    wire.take(down);
                } else {
                    self.web.iter_mut().find(|(_, socket, _)| *socket == stream).expect("issuer wire").2.take(down);
                }
            }
            other @ (IoRequest::Reject { .. }
            | IoRequest::Output { .. }
            | IoRequest::Spawn { .. }
            | IoRequest::Signal { .. }
            | IoRequest::Window { .. }
            | IoRequest::Usage { .. }
            | IoRequest::Abort { .. }) => panic!("partial head remains live: {other:?}"),
        }
    }

    fn answer(
        &mut self,
        component: &mut Component,
        env: &Env<Limits>,
        above: &mut Queue<skein_oauth_accounts::Event>,
        below: &mut Queue<skein_io::Request>,
    ) {
        use skein_io::Event as IoEvent;
            for (owner, _, wire) in &mut self.web {
            if let Some(up) = wire.answer() {
                component.up(env, IoEvent::Stream { owner: *owner, up }, above, below, &mut Queue::with_capacity(1));
            }
        }
        for (socket, wire) in &mut self.wires {
            if let Some(up) = wire.answer() {
                let owner = self.callbacks.iter().find(|(candidate, _)| candidate == socket).expect("bound callback").1;

                component.up(env, IoEvent::Stream { owner, up }, above, below, &mut Queue::with_capacity(1));
            }
        }
    }
}

#[test]
fn every_private_keeper_retains_its_maximal_paths_and_pending_load() {
    let mut limits = skein_oauth_accounts_world::world::limits();
    limits.accounts = 4;
    limits.exchanges = 4;
    limits.listeners = 0;
    limits.io.sockets = 4;
    let bound = worst_case(&limits).expect("private account bound");
    let meter = Meter::new();
    meter.start();
    let mut configured = List::with_capacity(4);
    for _ in 0..4 {
        assert!(
            configured
                .push(Account::SignIn {
                    origins: List::with_capacity(0),
                    shared: false,
                    registration: registration(
                        &[b'c'; 64],
                        bytes::copy_of(b"https://service.example/callback"),
                        Some(bytes::copy_of(&[b's'; 64])),
                        &[b's'; 64],
                    ),
                    endpoint: Endpoint { address: ([127, 0, 0, 1], 31000).into(), transport: Transport::Plaintext },
                    keeper: Keeper::Private {
                        root: Token::new(99),
                        directory: bytes::copy_of(&[b'd'; 4095]),
                        file: bytes::copy_of(&[b'f'; 4095])
                    },
                })
                .is_ok()
        );
    }
    let crypto = skein_lib::CryptoRng::new(&skein_world::crypto_seed(17), 0).expect("32-byte world seed");
    let mut component = Component::new(configured, &limits, crypto).expect("maximal private keepers");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut above = Queue::with_capacity(32);
    let mut io = Queue::with_capacity(32);
    let mut files = Queue::with_capacity(32);
    for account in 0..4 {
        component.down(&env, Request::SignIn { account, subject: None }, &mut above, &mut io, &mut files);
        component.down(&env, Request::Grant { account }, &mut above, &mut io, &mut files);
    }
    for _ in 0..4 {
        component.fire(&env, &mut above, &mut io, &mut files);
    }
    assert!(above.is_empty() && io.is_empty(), "all exchanges wait for private loading");
    assert_eq!(files.len(), 4);

    let mut requests = Vec::with_capacity(4);
    while let Some(lower) = files.pop() {
        match lower {
            FileLower::Request { request: file::Request::OpenPrivate { owner, .. }, .. } => requests.push(owner),
            FileLower::Request { .. } | FileLower::Cancel { .. } | FileLower::Remove { .. } => {
                panic!("expected private opening")
            }
        }
    }
    for (account, owner) in requests.iter().enumerate() {
        component.filed(
            &env,
            file::Event::Opened {
                owner: *owner,
                file: Token::new(u64::try_from(account).expect("four roots") + 100),
                len: 0,
            },
            &mut above,
            &mut io,
            &mut files,
        );
    }
    for _ in 0..4 {
        component.fire(&env, &mut above, &mut io, &mut files);
    }
    assert_eq!(files.len(), 4, "one pending load per keeper at the account limit");
    let mut loads = Vec::with_capacity(4);
    while let Some(lower) = files.pop() {
        match lower {
            FileLower::Request { request: file::Request::Load { owner, .. }, .. } => loads.push(owner),
            FileLower::Request { .. } | FileLower::Cancel { .. } | FileLower::Remove { .. } => {
                panic!("expected private loading")
            }
        }
    }
    for (account, owner) in loads.iter().enumerate() {
        let record = maximal_private_record(u32::try_from(account).expect("four accounts"));
        component.filed(
            &env,
            file::Event::Loaded {
                owner: *owner,
                bytes: skein_oauth::encode_record(&record, &limits.client.document).expect("maximal record"),
            },
            &mut above,
            &mut io,
            &mut files,
        );
    }
    for _ in 0..4 {
        component.fire(&env, &mut above, &mut io, &mut files);
    }
    assert_eq!(above.len(), 4, "every sign-in slot starts only after its maximal record loaded");
    let measured = meter.end();
    meter.check(measured, bound, &"all private paths, pending roots, records and sign-in slots");
}

fn registration(
    client_id: &[u8],
    redirect_uri: Box<[u8]>,
    client_secret: Option<Box<[u8]>>,
    scope: &[u8],
) -> skein_oauth::Registration {
    skein_oauth::Registration {
        authorization_url: bytes::copy_of(b"http://127.0.0.1:31000/authorize"),
        token_endpoint: bytes::copy_of(b"http://127.0.0.1:31000/token"),
        client_id: bytes::copy_of(client_id),
        redirect_uri,
        scope: bytes::copy_of(scope),
        wire: skein_oauth::WireFormat::Form,
        client_secret,
        pkce_for_confidential: false,
        metadata_claim: None,
        terms: None,
    }
}

fn maximal_private_record(account: u32) -> SavedToken {
    SavedToken {
        key: account,
        generation: 0,
        access_token: bytes::copy_of(&[b'a'; 256]),
        refresh_token: Some(bytes::copy_of(&[b'r'; 256])),
        metadata: Some(bytes::copy_of(&[b'm'; 256])),
        expires_at: Wall::from_nanos(30_000_000_000),
        issued_client_id: None,
        granted_scopes: None,
        subject: None,
        id_token: None,
        earliest_refresh_at: None,
    }
}

fn handed_in_record(account: u32) -> SavedToken {
    SavedToken {
        key: account,
        generation: 1,
        access_token: bytes::copy_of(&[b'x'; 64]),
        refresh_token: None,
        metadata: Some(bytes::copy_of(&[b'm'; 64])),
        expires_at: Wall::from_nanos(30_000_000_000),
        issued_client_id: None,
        granted_scopes: None,
        subject: None,
        id_token: None,
        earliest_refresh_at: None,
    }
}

#[test]
fn an_added_and_removed_place_fits_the_heap_bound() {
    drop(skein_oauth_accounts_world::accounts::run(
        131,
        skein_oauth_accounts_world::accounts::Story::Keeping,
        skein_world::Memory::Checked,
    ));
}

#[test]
fn two_varying_sign_ins_fit_the_heap_bound() {
    drop(skein_oauth_accounts_world::sign_in::run(
        73,
        skein_oauth_accounts_world::sign_in::Story::TwoPorts,
        false,
        false,
        skein_world::Memory::Checked,
    ));
}

fn shared_memory_record(account: u32) -> SavedToken {
    let mut record = handed_in_record(account);
    record.refresh_token = Some(bytes::copy_of(&[b'r'; 64]));
    record.granted_scopes = Some(bytes::copy_of(&[b's'; 64]));
    record.id_token = Some(bytes::copy_of(&[b'i'; 64]));
    record.expires_at = Wall::from_nanos(Duration::from_secs(5).as_nanos());
    record
}

fn shared_memory_limits() -> Limits {
    let document = skein_oauth::Limits {
        document_bytes: 512,
        string_bytes: 64,
        token_bytes: 64,
        client_bytes: 64,
        detail_bytes: 64,
        record_bytes: 512,
        depth: 8,
        tokens: 64,
        issued_client_bytes: 0,
        granted_bytes: 64,
        subject_bytes: 0,
        id_token_bytes: 64,
    };
    Limits {
        accounts: 4,
        file_stall: Duration::from_secs(1),
        lock_wait: Duration::from_secs(1),
        exchanges: 4,
        listeners: 0,
        server: skein_http::server::Limits {
            head: 2048,
            headers: 16,
            body: 1024,
            read: 256,
            response: 2048,
            send: 256,
        },
        http: skein_http::client::Limits { request: 2048, head: 2048, headers: 16, read: 256, send: 256 },
        tls: skein_tls::client::Limits { read: 2048, send: 2048, records: 131_072 },
        io: skein_io::Limits {
            sockets: 4,
            refusals: 1,
            intake: 32_768,
            receive: 1024,
            output: 32_768,
            sends: 4,
            accepts: 1,
            backlog: 2,
            close_timeout: Duration::from_secs(1),
            names: 0,
            retry: Duration::from_millis(1),
        },
        refresh_lead: Duration::from_secs(10),
        client: ClientLimits {
            document,
            uri_bytes: 256,
            scope_bytes: 64,
            state_bytes: 64,
            code_bytes: 64,
            url_bytes: 512,
            request_bytes: 512,
            parameter_bytes: 0,
            resource_bytes: 0,
            nonce_bytes: 0,
            metadata_bytes: 0,
            key_set_bytes: 0,
            keys: 0,
            modulus_bits: 2048,
            skew: Duration::from_secs(0),
            sign_in_time: Duration::from_secs(120),
            request_time: Duration::from_secs(10),
            backoff_base: Duration::from_secs(2),
            backoff_ceiling: Duration::from_secs(8),
            max_attempts: 3,
        },
    }
}

fn shared_memory_accounts(limits: &Limits) -> List<Account> {
    let mut configured = List::with_capacity(limits.accounts);
    for account in 0..limits.accounts {
        assert!(
            configured
                .push(Account::SignIn {
                    origins: List::with_capacity(0),
                    shared: true,
                    registration: registration(
                        b"client",
                        bytes::copy_of(b"https://service.example/callback"),
                        Some(bytes::copy_of(b"secret")),
                        b"read"
                    ),
                    endpoint: Endpoint { address: ([127, 0, 0, 1], 31000).into(), transport: Transport::Plaintext },
                    keeper: Keeper::Owner { kept: Some(shared_memory_record(account)) },
                })
                .is_ok()
        );
    }
    configured
}

fn listener_accounts(limits: &Limits) -> List<Account> {
    let mut configured = List::with_capacity(4);
    for account in 0..4 {
        let uri = format!("http://localhost:{}/callback", 31234 + account).into_bytes().into_boxed_slice();
        assert!(
            configured
                .push(Account::SignIn {
                    origins: maximal_origins(limits),
                    shared: false,
                    registration: {
                        let mut registration = skein_oauth_accounts_world::sign_in::registration(
                            skein_oauth_accounts_world::sign_in::Story::OpenIdAgain,
                        );
                        registration.redirect_uri = uri;
                        registration.terms.as_mut().expect("OpenID terms").openid.as_mut().expect("OpenID").key_set =
                            Some(bytes::copy_of(b"http://127.0.0.1:31000/keys"));
                        registration
                    },
                    endpoint: Endpoint {
                        address: "127.0.0.1:31000".parse().expect("loopback"),
                        transport: Transport::Plaintext
                    },
                    keeper: Keeper::Owner { kept: None }
                })
                .is_ok()
        );
    }
    configured
}
fn maximal_origins(limits: &Limits) -> List<skein_oauth_accounts::Origin> {
    let mut origins = List::with_capacity(3);
    for port in 31001..31004 {
        let base = format!("http://.example:{port}");
        let length = usize::try_from(limits.client.uri_bytes).expect("URI bound");
        let uri = format!("http://{}.example:{port}", "o".repeat(length - base.len()));
        origins
            .push(skein_oauth_accounts::Origin {
                uri: uri.into_bytes().into_boxed_slice(),
                endpoint: Endpoint { address: ([127, 0, 0, 1], port).into(), transport: Transport::Plaintext },
            })
            .expect("origins");
    }
    origins
}
