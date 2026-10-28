//! Full bounded scenario shared by the service and hosted-peer memory binaries.
use skein_fake_forge_domain::{self as domain, api as a};
use skein_fake_forge_github::{
    self as fake,
    app::App,
    service::{Account, AppFacts, Row, Shown},
};
use skein_forge::github::{
    document::{Access, Permission},
    request::Method,
};
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Secret, Time, Token, Wall};

pub const SEED: u64 = 23;
pub const BOT: u64 = 9;
pub const PERSON: u64 = 31;
pub const NOW: Wall = Wall::from_nanos(1_200_000_000_000);
pub const MANIFEST: &[u8] = br#"{"name":"example-app","url":"http://127.0.0.1/","redirect_url":"http://127.0.0.1/callback","setup_url":"http://127.0.0.1/setup","public":true,"default_permissions":{"contents":"write","issues":"write","pull_requests":"write","checks":"read"},"default_events":["issues","pull_request","push","check_run"]}"#;

pub fn limits() -> fake::Limits {
    fake::Limits {
        deliveries: fake::delivery::Limits { entries: 64, payload_bytes: 32768 },
        http: skein_http::server::Limits { head: 4096, headers: 16, body: 4096, read: 128, response: 16384, send: 128 },
        github: skein_forge::github::limits::Limits {
            json: skein_json::tokenizer::Limits { depth: 16, string: 4096, number: 32, chunk: 128, length: 16384 },
            writer: skein_json::writer::Limits { depth: 16, length: 16384 },
            string_bytes: 128,
            body_bytes: 4096,
            inner: 4,
            target_bytes: 1024,
        },
        installations: fake::installations::Limits { installations: 2, tokens: 4, listed: 2 },
        pages: fake::pages::Limits { states: 2, manifest_bytes: 2048 },
        page: 2,
        body: 4096,
        response: 16384,
        blob_records: 2,
        git_records: 16,
    }
}
pub fn config() -> fake::Config {
    let mut repositories = List::with_capacity(4);
    let mut accounts = List::with_capacity(4);
    let mut tokens = List::with_capacity(4);
    for id in 1..=4_u32 {
        let (owner, owner_id) = owner(id);
        repositories
            .push(Row { id, owner: owner.into(), owner_id, organization: true, name: name(id), private: true })
            .expect("bounded fixture");
        let user = if id % 2 == 0 { 32 } else { PERSON };
        tokens
            .push((
                Secret::new(vec![b'a' + u8::try_from(id).expect("bounded fixture"); 128].into_boxed_slice()),
                user,
                None,
            ))
            .expect("bounded fixture");
    }
    for (login, id, organization) in
        [(b"acme".as_slice(), 22, true), (b"example", 23, true), (b"alice", PERSON, false), (b"bob", 32, false)]
    {
        accounts.push(Account { login: login.into(), id, organization }).expect("bounded fixture");
    }
    fake::Config {
        rate_quota: 5000,
        rate_window: Duration::from_secs(3600),
        token_life: skein_fake_forge_github::installations::TOKEN_LIFE,
        deliveries: fake::delivery::Config::standard(),
        base: Box::from(b"http://127.0.0.1:3000".as_slice()),
        blob: Box::from(b"http://127.0.0.1:3001".as_slice()),
        third_origin: None,
        blob_redirect: None,
        app: App {
            id: 7,
            slug: Box::from(b"example-app".as_slice()),
            owner: PERSON,
            key: fake::keys::KEYS[0],
            webhook: Secret::new(vec![b's'; 128].into_boxed_slice()),
            permissions: Box::new([
                Permission { name: Box::from(b"contents".as_slice()), access: Access::Write },
                Permission { name: Box::from(b"issues".as_slice()), access: Access::Write },
                Permission { name: Box::from(b"pull_requests".as_slice()), access: Access::Write },
                Permission { name: Box::from(b"checks".as_slice()), access: Access::Read },
            ]),
            events: Box::new([
                Box::from(b"issues".as_slice()),
                Box::from(b"pull_request".as_slice()),
                Box::from(b"push".as_slice()),
                Box::from(b"check_run".as_slice()),
            ]),
        },
        app_facts: AppFacts::Complete,
        shown: Shown::Both,
        ruleset_id: 17,
        pull_requests_only: false,
        required_signatures: true,
        bot: BOT,
        hook: Box::from(b"http://127.0.0.1/hooks".as_slice()),
        repositories,
        accounts,
        person: Box::from(b"alice".as_slice()),
        tokens,
        faults: fake::Faults { reset: 0, drop: 0, server_error: 0 },
    }
}
pub fn name(id: u32) -> Box<[u8]> {
    match id {
        1 => Box::from(b"widgets".as_slice()),
        2 => Box::from(b"tools".as_slice()),
        3 => Box::from(b"third".as_slice()),
        4 => Box::from(b"fourth".as_slice()),
        _ => panic!("four repositories"),
    }
}
pub fn owner(id: u32) -> (&'static [u8], u64) {
    if id <= 2 { (b"acme", 22) } else { (b"example", 23) }
}
pub fn path(id: u32) -> Box<[u8]> {
    let mut path = owner(id).0.to_vec();
    path.push(b'/');
    path.extend_from_slice(&name(id));
    path.into_boxed_slice()
}
pub fn domain_config() -> domain::Config {
    domain::Config {
        limits: domain::Limits {
            repositories: 4,
            users: 5,
            bypassing_users: 1,
            labels: 1,
            items: 1,
            comments: 1,
            dependencies: 1,
            reviews: 1,
            branches: 3,
            commits: 24,
            files: 1,
            statuses: 4,
            contexts: 1,
            workflows: 0,
            workflow_runs: 0,
            pages: 1,
            name_bytes: 128,
            title_bytes: 32,
            body_bytes: 64,
            content_bytes: 64,
            page_size: 2,
            calls: 2,
            hooks: 8,
            observations: 32,
        },
        latency_min: Duration::ZERO,
        latency_max: Duration::ZERO,
        late: 0,
        late_min: Duration::ZERO,
        late_max: Duration::ZERO,
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::ZERO,
        land_max: Duration::ZERO,
        rate_limit: 0,
        rate_window: Duration::from_secs(1),
        settle_min: Duration::ZERO,
        settle_max: Duration::ZERO,
        ci: 9,
        setup_faults: 0,
        lost: 0,
        hook_min: Duration::from_secs(3600),
        hook_max: Duration::from_secs(3600),
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: domain::Skew::None,
        status_updates: false,
        edit_updates: false,
    }
}
pub fn domain() -> (domain::Domain, domain::Config) {
    let config = domain_config();
    let mut domain = domain::Domain::new(&config, SEED);
    for id in 1..=4_u32 {
        let path = path(id);
        let first = domain::repository(
            &mut domain,
            &config,
            a::Setup {
                workflows: Box::new([]),
                id,
                name: path.clone(),
                default: Box::from(b"main".as_slice()),
                tree: Box::new([]),
                labels: Box::new([]),
                checks: a::Checks {
                    cues: Box::new([]),
                    contexts: Box::new([Box::from(b"ci".as_slice())]),
                    latency_min: Duration::from_secs(3600),
                    latency_max: Duration::from_secs(3600),
                    silent: 1000,
                    passes: 0,
                    reruns: 0,
                    cue: None,
                },
                protection: Some(a::Protection {
                    branch: Box::from(b"main".as_slice()),
                    contexts: Box::new([Box::from(b"ci".as_slice())]),
                    approvals: 0,
                    dismiss_stale: true,
                    code_owners: false,
                    linear: true,
                    queue: true,
                    bypassing_users: Box::new([]),
                }),
                hooked: true,
            },
        );
        domain::grant(&mut domain, &path, PERSON, a::Permission::Admin);
        domain::grant(&mut domain, &path, BOT, a::Permission::Write);
        let head = domain::commit(
            &mut domain,
            &config,
            first,
            Box::new([a::File { path: Box::from(b"file".as_slice()), content: Box::from(b"feature".as_slice()) }]),
            b"feature",
        )
        .expect("bounded fixture")
        .expect("bounded fixture");
        let mut env = Env { now: Time::ZERO, wall: NOW, limits: config };
        let mut out = Queue::uncapped();
        for op in [
            a::Op::Git(a::Git::Push { branch: Box::from(b"feature".as_slice()), commit: head, expected: None }),
            a::Op::Write(a::Write::OpenPull {
                title: Box::from(b"feature".as_slice()),
                body: Box::from(b"body".as_slice()),
                head: Box::from(b"feature".as_slice()),
                base: Box::from(b"main".as_slice()),
                opens_empty: false,
            }),
            a::Op::Write(a::Write::Enqueue { number: 1, head }),
        ] {
            domain::step(
                &mut domain,
                &env,
                domain::Event::Call {
                    reply_to: ReplyTo::new(Token::new(1)),
                    user: PERSON,
                    repository: path.clone(),
                    op,
                },
                &mut out,
            );
            if out.is_empty() {
                env.now = domain.next_deadline().expect("immediate call timer");
                assert_eq!(env.now, Time::ZERO, "fixture call precedes CI and hooks");
                domain::fire(&mut domain, &env, &mut out);
            }
            let Some(domain::Request::Reply { result, .. }) = out.pop() else { panic!("queued scenario reply") };
            result.expect("queued scenario call");
            domain.reclaim();
        }
        let a::Answer::Pull(pull) =
            domain.inspect(&config, &path, &a::Read::Pull { number: 1 }).expect("bounded fixture")
        else {
            panic!("queued pull")
        };
        assert!(pull.queued.is_some(), "every repository's queue holds its only pull");
    }
    (domain, config)
}
pub fn bearer(value: &[u8]) -> Secret {
    let mut header = b"Bearer ".to_vec();
    header.extend_from_slice(value);
    Secret::new(header.into_boxed_slice())
}
pub fn jwt(app: u64, key: &[u8], now: Wall) -> Secret {
    let input = skein_forge::github::jwt::signing_input(app, now, Wall::from_nanos(now.as_nanos() + 600_000_000_000))
        .expect("bounded fixture");
    let signature = skein_tls::sign_rs256(key, &input).expect("bounded fixture");
    bearer(skein_forge::github::jwt::token(&input, &signature).expect("bounded fixture").expose())
}
pub fn form() -> Box<[u8]> {
    let mut form = b"manifest=".to_vec();
    for byte in MANIFEST {
        form.extend_from_slice(format!("%{byte:02X}").as_bytes());
    }
    form.into_boxed_slice()
}
pub fn ask(
    service: &mut fake::Service,
    method: Method,
    target: &[u8],
    authorization: Option<&[u8]>,
    body: &[u8],
) -> fake::service::Response {
    let fake::service::Outcome::Reply(response) = service.request(
        &fake::service::Request {
            validator: None,
            method,
            target,
            authorization,
            version: Some(skein_forge::github::pin::API_VERSION.as_bytes()),
            agent: true,
            body,
        },
        NOW,
        ReplyTo::new(Token::new(1)),
    ) else {
        panic!("layer store route")
    };
    response
}
