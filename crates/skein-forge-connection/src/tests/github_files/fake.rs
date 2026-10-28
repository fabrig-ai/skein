//! The shared fake and GitHub face, driven on the component test's thread.
use super::super::fixtures as f;
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as domain, api as a};
use skein_fake_forge_github::{app::App, installations, keys::KEYS, pages, service};
use skein_forge::github;
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Secret, Time, Token, Wall, Writer};

pub(super) struct Fake {
    pub(super) service: service::Service,
    pub(super) domain: domain::Domain,
    pub(super) env: Env<domain::Config>,
    pub(super) first: u64,
    pub(super) head: u64,
    pub(super) base: u64,
    pub(super) number: u64,
    pub(super) token: Secret,
}
impl Fake {
    pub fn new(content: &[u8], divergent: bool) -> Self {
        Self::with_files(content, divergent, 1)
    }
    pub(super) fn with_files(content: &[u8], divergent: bool, files: u32) -> Self {
        let mut config = config();
        config.limits.files = config.limits.files.max(files);
        config.limits.page_size = config.limits.page_size.max(files);
        let mut domain = domain::Domain::new(&config, 5);
        let first = domain::repository(
            &mut domain,
            &config,
            a::Setup {
                workflows: Box::new([]),
                id: 1,
                name: Box::from(b"acme/widgets".as_slice()),
                default: Box::from(b"main".as_slice()),
                tree: Box::from([]),
                labels: Box::from([]),
                checks: a::Checks {
                    cues: Box::new([]),
                    contexts: Box::from([]),
                    latency_min: Duration::ZERO,
                    latency_max: Duration::ZERO,
                    silent: 0,
                    passes: 1000,
                    reruns: 0,
                    cue: None,
                },
                protection: None,
                hooked: false,
            },
        );
        domain::grant(&mut domain, b"acme/widgets", 9, a::Permission::Write);
        let mut service = github_service(files.max(2));
        assert_eq!(
            service.installations_mut().install(
                22,
                b"acme",
                installations::Kind::Organization,
                installations::Reach::All,
                Wall::EPOCH
            ),
            Ok(1)
        );
        let (token, _) =
            service.installations_mut().mint(1, installations::Scope::Full, Wall::EPOCH).expect("fake token");
        let mut tree = List::with_capacity(files);
        for index in 0..files {
            let path = if files == 1 {
                Box::from(b"file".as_slice())
            } else {
                let index = skein_lib::Decimal::of(u64::from(index));
                let mut path = Writer::new(4_usize.checked_add(index.as_bytes().len()).expect("path length"));
                path.put(b"file").expect("prefix");
                path.put(index.as_bytes()).expect("index");
                path.finish()
            };
            tree.push(a::File { path, content: Box::from(content) }).expect("file count");
        }
        let head = domain::commit(&mut domain, &config, first, tree.into_boxed(), b"feature")
            .expect("commit room")
            .expect("parent exists");
        let mut fake = Self {
            service,
            domain,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config },
            first,
            head,
            base: first,
            number: 0,
            token,
        };
        fake.call(a::Op::Git(a::Git::Push { branch: Box::from(b"feature".as_slice()), commit: head, expected: None }));
        let a::Answer::Created(number) = fake.call(a::Op::Write(a::Write::OpenPull {
            title: Box::from(b"feature".as_slice()),
            body: Box::from([]),
            head: Box::from(b"feature".as_slice()),
            base: Box::from(b"main".as_slice()),
            opens_empty: false,
        })) else {
            unreachable!("created pull")
        };
        fake.number = number;
        if divergent {
            fake.base = domain::commit(
                &mut fake.domain,
                &config,
                first,
                Box::new([a::File {
                    path: Box::from(b"file".as_slice()),
                    content: Box::from(b"base only".as_slice()),
                }]),
                b"base",
            )
            .expect("commit room")
            .expect("parent exists");
            fake.call(a::Op::Git(a::Git::Push {
                branch: Box::from(b"main".as_slice()),
                commit: fake.base,
                expected: None,
            }));
        }
        fake
    }
    fn drive(&mut self, event: domain::Event) -> Result<a::Answer, a::Error> {
        let mut out = Queue::uncapped();
        domain::step_admitted(&mut self.domain, &self.env, event, &mut out);
        if out.is_empty() {
            domain::fire(&mut self.domain, &self.env, &mut out);
        }
        let domain::Request::Reply { to, result } = out.pop().expect("zero-latency reply") else {
            unreachable!("no hooks")
        };
        assert_eq!(to.into_token(), Token::new(77));
        assert!(out.is_empty());
        self.domain.reclaim();
        result
    }
    fn call(&mut self, op: a::Op) -> a::Answer {
        self.drive(domain::Event::Call {
            reply_to: ReplyTo::new(Token::new(77)),
            user: 9,
            repository: Box::from(b"acme/widgets".as_slice()),
            op,
        })
        .expect("scenario call")
    }
    pub fn answer(&mut self, wire: &[u8]) -> Box<[u8]> {
        // Every operation here sends GETs with empty request bodies.
        let mut previous = 0;
        let mut head = None;
        for (at, part) in wire.windows(4).enumerate() {
            if part == b"\r\n\r\n" {
                head = wire.get(previous..at);
                previous = at.checked_add(4).expect("head end");
            }
        }
        let request = head.expect("complete request");
        let line_end = skein_lib::bytes::find(request, b"\r\n").expect("request line");
        let line = request.get(..line_end).expect("line boundary");
        let target = line.strip_prefix(b"GET ").expect("GET request").strip_suffix(b" HTTP/1.1").expect("HTTP version");
        let mut rest = request.get(line_end.checked_add(2).expect("line end")..).expect("headers");
        let mut authorization = None;
        let mut version = None;
        let mut agent = false;
        for _ in 0..f::limits().headers {
            let (line, next) = match skein_lib::bytes::find(rest, b"\r\n") {
                Some(end) => (
                    rest.get(..end).expect("line"),
                    rest.get(end.checked_add(2).expect("line end")..).expect("next header"),
                ),
                None => (rest, b"".as_slice()),
            };
            rest = next;
            if let Some(value) = line.strip_prefix(b"Authorization: ") {
                authorization = Some(value);
            }
            if let Some(value) = line.strip_prefix(b"X-GitHub-Api-Version: ") {
                version = Some(value);
            }
            if line.starts_with(b"User-Agent: ") {
                agent = true;
            }
        }
        let mut outcome = self.service.request(
            &service::Request {
                validator: None,
                method: github::request::Method::Get,
                target,
                authorization,
                version,
                agent,
                body: b"",
            },
            Wall::EPOCH,
            ReplyTo::new(Token::new(77)),
        );
        let response = loop {
            outcome = match outcome {
                service::Outcome::Reply(response) => break response,
                service::Outcome::Admit(admission) => {
                    self.service.admit(admission, &mut self.domain, &self.env, ReplyTo::new(Token::new(77)))
                }
                service::Outcome::Call(pending, event) => {
                    let result = self.drive(event);
                    self.service.finish(pending, result, ReplyTo::new(Token::new(77)), &self.domain)
                }
            };
        };
        encode(response)
    }
}
fn encode(response: service::Response) -> Box<[u8]> {
    let mut fields = List::with_capacity(7);
    fields
        .push(skein_http::Header {
            name: Box::from(b"Content-Type".as_slice()),
            value: Box::from(response.content_type.bytes()),
        })
        .expect("content type header");
    if let Some(rate) = response.rate {
        skein_fake_forge_github::rates::headers(rate, response.status == 403, &mut fields);
    }
    if let Some(etag) = response.etag {
        fields.push(skein_http::Header { name: Box::from(b"ETag".as_slice()), value: etag }).expect("etag header");
    }
    let status = skein_lib::Decimal::of(u64::from(response.status));
    let length = skein_lib::Decimal::of(u64::try_from(response.body.len()).expect("body length"));
    let pieces =
        [b"HTTP/1.1 ".as_slice(), status.as_bytes(), b" Answer\r\nContent-Length: ", length.as_bytes(), b"\r\n"];
    let mut size = response.body.len().checked_add(2).expect("body framing");
    for piece in pieces {
        size = size.checked_add(piece.len()).expect("head length");
    }
    if let Some(link) = &response.link {
        size = size.checked_add(link.len()).expect("link length").checked_add(8).expect("link framing");
    }
    for field in &fields {
        size =
            size.checked_add(field.name.len()).unwrap().checked_add(field.value.len()).unwrap().checked_add(4).unwrap();
    }
    let mut out = Writer::new(size);
    for piece in pieces {
        out.put(piece).expect("response room");
    }
    for field in &fields {
        out.put(&field.name).expect("field name");
        out.put(b": ").expect("field separator");
        out.put(&field.value).expect("field value");
        out.put(b"\r\n").expect("field end");
    }
    if let Some(link) = response.link {
        out.put(b"Link: ").expect("link");
        out.put(&link).expect("link");
        out.put(b"\r\n").expect("link");
    }
    out.put(b"\r\n").expect("head end");
    out.put(&response.body).expect("body");
    out.finish()
}

pub(super) fn object(value: u64) -> skein_forge::api::Commit {
    let mut commit = [0; 32];
    for (out, byte) in commit.iter_mut().zip(domain::object_bytes(value)) {
        *out = byte;
    }
    commit
}

fn config() -> domain::Config {
    domain::Config {
        limits: domain::Limits {
            repositories: 2,
            users: 3,
            bypassing_users: 4,
            labels: 1,
            items: 1,
            comments: 1,
            dependencies: 1,
            reviews: 1,
            branches: 4,
            commits: 8,
            files: 4,
            statuses: 1,
            contexts: 1,
            workflows: 0,
            workflow_runs: 0,
            pages: 1,
            name_bytes: 64,
            title_bytes: 64,
            body_bytes: 128,
            content_bytes: 8192,
            page_size: 2,
            calls: 2,
            hooks: 4,
            observations: 64,
        },
        latency_min: Duration::ZERO,
        latency_max: Duration::ZERO,
        settle_min: Duration::from_secs(2),
        settle_max: Duration::from_secs(2),
        late: 0,
        late_min: Duration::ZERO,
        late_max: Duration::ZERO,
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::ZERO,
        land_max: Duration::ZERO,
        rate_limit: 0,
        rate_window: Duration::from_secs(10),
        ci: 3,
        setup_faults: 0,
        lost: 0,
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: domain::Skew::None,
        status_updates: false,
        edit_updates: false,
    }
}

fn github_service(page: u32) -> service::Service {
    let mut repositories = List::with_capacity(1);
    repositories
        .push(service::Row {
            id: 1,
            owner: Box::from(b"acme".as_slice()),
            owner_id: 22,
            organization: true,
            name: Box::from(b"widgets".as_slice()),
            private: true,
        })
        .expect("repository");
    let mut accounts = List::with_capacity(1);
    accounts
        .push(service::Account { id: 22, login: Box::from(b"acme".as_slice()), organization: true })
        .expect("account");
    let limits = service::Limits {
        deliveries: skein_fake_forge_github::delivery::Limits { entries: 64, payload_bytes: 32768 },
        http: skein_http::server::Limits {
            head: 4096,
            headers: 16,
            body: u64::from(f::BODY_ROOM),
            read: 1024,
            response: 16384,
            send: 1024,
        },
        github: crate::limits::github_documents(&f::limits()),
        installations: installations::Limits { installations: 1, tokens: 2, listed: 1 },
        pages: pages::Limits { states: 1, manifest_bytes: 2048 },
        page,
        body: f::BODY_ROOM,
        response: 8192,
        blob_records: 16,
        git_records: 16,
    };
    service::Service::new(
        service::Config {
            deliveries: skein_fake_forge_github::delivery::Config::standard(),
            rate_quota: 5000,
            rate_window: Duration::from_secs(3600),
            token_life: installations::TOKEN_LIFE,
            base: Box::from(b"http://127.0.0.1:3000".as_slice()),
            blob: Box::from(b"http://logs.example.test".as_slice()),
            third_origin: None,
            blob_redirect: None,
            app: App {
                id: 7,
                slug: Box::from(b"test-app".as_slice()),
                owner: 1,
                permissions: Box::from([]),
                events: Box::from([]),
                key: KEYS.first().expect("public test key"),
                webhook: Secret::new(Box::from(b"test-hook".as_slice())),
            },
            app_facts: service::AppFacts::Complete,
            shown: service::Shown::Ruleset,
            ruleset_id: 17,
            pull_requests_only: false,
            required_signatures: false,
            tokens: List::with_capacity(0),
            faults: skein_fake_forge_github::Faults { reset: 0, drop: 0, server_error: 0 },
            bot: 9,
            hook: Box::from(b"https://hook.test/".as_slice()),
            repositories,
            accounts,
            person: Box::from(b"acme".as_slice()),
        },
        &limits,
        5,
    )
    .expect("fake config")
}
