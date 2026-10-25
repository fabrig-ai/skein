//! The fake GitHub layer, framed as HTTP for the machine world's peer.
use skein_fake_forge_domain::{self as fake, api as a};
use skein_fake_forge_github::{
    app::App,
    installations::{self, Installations, Kind, Reach},
    keys::KEYS,
    manifest::Manifest,
    pages,
    service::{Account, AppFacts, Config, Limits, Outcome, Request, Row, Service},
};
use skein_forge::github::{self, request::Method};
use skein_json::{tokenizer, writer};
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Secret, Time, Token, Wall};

/// A wall past the fake's installation dates, advanced by the machine's clock.
pub const WALL_BASE: Wall = Wall::from_nanos(1_000_000_000_000_000);

/// The blob host is resolved to a loopback destination by this in-memory world.
pub const BLOB_ADDRESS: core::net::SocketAddr =
    core::net::SocketAddr::new(core::net::IpAddr::V4(core::net::Ipv4Addr::LOCALHOST), 3001);
pub const BLOB_ORIGIN: &[u8] = b"http://logs.example.test:3001";

struct LogScenario<'a> {
    third_origin: Option<&'a [u8]>,
    blob_redirect: Option<&'a [u8]>,
}

/// The GitHub service's state behind one machine's scripted peer.
pub struct Fake {
    service: Service,
    domain: fake::Domain,
    config: fake::Config,
    statuses: Vec<u16>,
}
impl Fake {
    /// Uses the GitHub fake's fixture App, accounts, repositories and installations.
    /// The App holds the grants setup asks for: its bot's permission is its
    /// installation's (forge-github.md, 4.1, 8.2).
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::scenario(seed, AppFacts::Complete, true, b"https://hook.test/start")
    }

    /// The fixture App without grants or events: setup's checks of a lacking App
    /// (forge-setup.md, 5.6). Its bot's repository permission is none.
    #[must_use]
    pub fn bare(seed: u64) -> Self {
        Self::scenario(seed, AppFacts::Complete, false, b"https://hook.test/start")
    }

    /// Scenario-only App metadata and hook, using the dialect's public configuration.
    #[must_use]
    pub fn scenario(seed: u64, facts: AppFacts, complete: bool, hook: &[u8]) -> Self {
        Self::configured(seed, facts, complete, hook, None)
    }

    /// A failing CI job and the fake's signed-log scenarios.
    #[must_use]
    pub fn job_logs(seed: u64, third_origin: Option<&[u8]>, blob_redirect: Option<&[u8]>) -> Self {
        Self::configured(
            seed,
            AppFacts::Complete,
            true,
            b"https://hook.test/start",
            Some(LogScenario { third_origin, blob_redirect }),
        )
    }

    fn configured(seed: u64, facts: AppFacts, complete: bool, hook: &[u8], logs: Option<LogScenario<'_>>) -> Self {
        let has_logs = logs.is_some();
        let LogScenario { third_origin, blob_redirect } =
            logs.unwrap_or(LogScenario { third_origin: None, blob_redirect: None });
        let permissions = if complete { skein_fake_forge_github::app::wanted() } else { Box::from([]) };
        let events = if complete {
            github::setup::EVENTS.iter().map(|name| Box::from(*name)).collect::<Vec<_>>().into_boxed_slice()
        } else {
            Box::from([])
        };
        let config = Config {
            deliveries: skein_fake_forge_github::delivery::Config::standard(),
            rate_quota: 5000,
            rate_window: Duration::from_secs(3600),
            token_life: skein_fake_forge_github::installations::TOKEN_LIFE,
            base: Box::from(b"http://127.0.0.1:3000".as_slice()),
            blob: Box::from(BLOB_ORIGIN),
            third_origin: third_origin.map(Box::from),
            blob_redirect: blob_redirect.map(Box::from),
            app: App { permissions, events, ..fixture_app() },
            app_facts: facts,
            shown: skein_fake_forge_github::service::Shown::Ruleset,
            ruleset_id: 17,
            pull_requests_only: false,
            required_signatures: false,
            tokens: List::with_capacity(0),
            faults: skein_fake_forge_github::Faults { reset: 0, drop: 0, server_error: 0 },
            bot: 9,
            hook: Box::from(hook),
            repositories: fixture_repositories(),
            accounts: fixture_accounts(),
            person: Box::from(b"alice".as_slice()),
        };
        let installation_slots = 3;
        let limits = Limits {
            deliveries: skein_fake_forge_github::delivery::Limits { entries: 64, payload_bytes: 32768 },
            http: skein_http::server::Limits {
                head: 4096,
                headers: 16,
                body: 4096,
                read: 1024,
                response: 16384,
                send: 1024,
            },
            github: github::limits::Limits {
                json: tokenizer::Limits { depth: 10, string: 4096, number: 32, chunk: 128, length: 8192 },
                writer: writer::Limits { depth: 10, length: 8192 },
                string_bytes: 256,
                body_bytes: 4096,
                // Browser manifests carry every subscribed event and wanted permission.
                inner: u32::try_from(github::setup::EVENTS.len().max(github::setup::PERMISSIONS.len()))
                    .expect("manifest lists fit"),
                target_bytes: 1024,
            },
            // Each installation has Full and Contents tokens, with old and renewed generations overlapping.
            installations: installations::Limits {
                installations: installation_slots,
                tokens: installation_slots * 2 * 2,
                listed: 2,
            },
            pages: pages::Limits { states: 3, manifest_bytes: 2048 },
            page: 2,
            body: 4096,
            response: 8192,
            blob_records: 16,
            git_records: 16,
        };
        let mut service = Service::new(config, &limits, seed).expect("valid fixture config");
        let mut listed = List::with_capacity(2);
        for id in [2, 4] {
            listed.push(id).expect("fixture reach");
        }
        let store = service.installations_mut();
        assert_eq!(
            store
                .install(22, b"acme", Kind::Organization, Reach::Listed(listed), Wall::from_nanos(1_000_000_000_000))
                .expect("fixture installation"),
            1
        );
        assert_eq!(
            store
                .install(23, b"example", Kind::User, Reach::All, Wall::from_nanos(1_001_000_000_000))
                .expect("fixture installation"),
            2
        );
        assert_eq!(
            store
                .install(24, b"other", Kind::User, Reach::All, Wall::from_nanos(1_002_000_000_000))
                .expect("fixture installation"),
            3
        );
        assert!(store.suspend(3, true), "third installation exists");
        let (domain, config) = fixture_domain(seed, has_logs);
        Self { service, domain, config, statuses: Vec::new() }
    }

    pub(crate) fn into_service(self) -> Service {
        self.service
    }

    /// Scenario access to the fake's installation store.
    pub fn installations_mut(&mut self) -> &mut Installations {
        self.service.installations_mut()
    }

    /// Statuses of the fake's HTTP responses, in request order.
    #[must_use]
    pub fn statuses(&self) -> &[u16] {
        &self.statuses
    }

    /// Scenario releases use the fake's configured visibility delay and failed endpoint.
    pub fn release(&mut self, payload: github::delivery::Delivery, wall: Wall) -> u64 {
        let (id, post) = self.service.release(payload, Some(1), wall).expect("fixture delivery fits");
        assert!(post.is_none(), "no hook endpoint");
        id
    }

    /// A redelivery retains its guid and gets a newer log id.
    pub fn redeliver(&mut self, id: u64, wall: Wall) -> u64 {
        let (id, post) = self.service.redeliver(id, wall).expect("retained delivery");
        assert!(post.is_none(), "no hook endpoint");
        id
    }

    /// Later App requests are refused by the fake.
    pub fn revoke_key(&mut self) {
        self.service.revoke_key();
    }

    /// Scenario inspection without issuing an API request.
    #[must_use]
    pub fn installation_ids(&mut self) -> Vec<u64> {
        self.service.installations_mut().iter().map(|row| row.id).collect()
    }

    /// Answers one complete HTTP/1.1 request, keeping the connection open.
    pub fn serve(&mut self, request: &[u8], wall: Wall) -> Vec<u8> {
        let request = parse(request);
        let outcome = self.service.request(&request, wall, ReplyTo::new(Token::new(77)));
        self.finish(outcome, wall)
    }

    /// Blob requests go through the fake's signature check and header-presence record.
    pub fn blob(&mut self, bytes: &[u8], wall: Wall) -> Vec<u8> {
        let request = parse(bytes);
        let range = bytes
            .split(|b| *b == b'\n')
            .any(|line| line.split(|b| *b == b':').next().is_some_and(|name| name.eq_ignore_ascii_case(b"Range")));
        let outcome = self.service.blob(
            request.method,
            request.target,
            request.authorization.is_some(),
            range,
            wall,
            ReplyTo::new(Token::new(77)),
        );
        self.finish(outcome, wall)
    }

    #[must_use]
    pub fn blob_records(&self) -> &Queue<skein_fake_forge_github::logs::Record> {
        self.service.blob_records()
    }

    fn finish(&mut self, mut outcome: Outcome, wall: Wall) -> Vec<u8> {
        let response = loop {
            outcome = match outcome {
                Outcome::Reply(response) => break response,
                Outcome::Admit(admission) => self.service.admit(
                    admission,
                    &mut self.domain,
                    &Env {
                        now: Time::from_nanos(wall.as_nanos().saturating_sub(WALL_BASE.as_nanos())),
                        wall,
                        limits: self.config,
                    },
                    ReplyTo::new(Token::new(77)),
                ),
                Outcome::Call(pending, event) => {
                    let result = domain_reply(&mut self.domain, &self.config, wall, event);
                    self.service.finish(pending, result, ReplyTo::new(Token::new(77)), &self.domain)
                }
            };
        };
        self.statuses.push(response.status);
        let media = match response.content_type {
            skein_fake_forge_github::service::ContentType::Json => "application/json; charset=utf-8",
            skein_fake_forge_github::service::ContentType::Plaintext => "text/plain; charset=utf-8",
            skein_fake_forge_github::service::ContentType::Octets => "application/octet-stream",
            skein_fake_forge_github::service::ContentType::Html => "text/html; charset=utf-8",
        };
        let mut bytes = response_head(response.status, wall, Some(media));
        if let Some(location) = response.location {
            bytes.extend_from_slice(b"Location: ");
            bytes.extend_from_slice(&location);
            bytes.extend_from_slice(b"\r\n");
        }
        if let Some(rate) = response.rate {
            let mut headers = List::with_capacity(6);
            skein_fake_forge_github::rates::headers(rate, response.status == 403, &mut headers);
            for header in &headers {
                bytes.extend_from_slice(&header.name);
                bytes.extend_from_slice(b": ");
                bytes.extend_from_slice(&header.value);
                bytes.extend_from_slice(b"\r\n");
            }
        }
        if let Some(etag) = response.etag {
            bytes.extend_from_slice(b"ETag: ");
            bytes.extend_from_slice(&etag);
            bytes.extend_from_slice(b"\r\n");
        }
        if let Some(link) = response.link {
            bytes.extend_from_slice(b"Link: ");
            bytes.extend_from_slice(&link);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(format!("Content-Length: {}\r\n\r\n", response.body.len()).as_bytes());
        bytes.extend_from_slice(&response.body);
        bytes
    }

    /// Answers one complete browser request without the API's headers or credentials.
    pub fn page(&mut self, request: &[u8], wall: Wall) -> Vec<u8> {
        let request = parse(request);
        assert!(request.authorization.is_none(), "browser carries no API credential");
        let answer = self
            .service
            .page(&pages::Page { method: request.method, target: request.target, body: request.body }, wall);
        let media = match answer.media {
            pages::Media::Html => Some("text/html; charset=utf-8"),
            pages::Media::None => None,
        };
        let mut bytes = response_head(answer.status, wall, media);
        if let Some(location) = answer.location {
            bytes.extend_from_slice(b"Location: ");
            bytes.extend_from_slice(&location);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(format!("Content-Length: {}\r\n\r\n", answer.body.len()).as_bytes());
        bytes.extend_from_slice(&answer.body);
        bytes
    }

    /// The fake's posted manifest, readable after conversion until reclamation.
    #[must_use]
    pub fn manifest(&self, state: &[u8]) -> Option<&Manifest> {
        self.service.manifest(state)
    }
}

fn fixture_accounts() -> List<Account> {
    let mut accounts = List::with_capacity(2);
    for (login, id, organization) in [(b"acme".as_slice(), 22, true), (b"alice".as_slice(), 31, false)] {
        accounts.push(Account { login: Box::from(login), id, organization }).expect("fixture accounts");
    }
    accounts
}

fn fixture_repositories() -> List<Row> {
    let mut repositories = List::with_capacity(3);
    for (id, name, organization, private) in
        [(4, b"tools".as_slice(), false, false), (2, b"widgets", true, true), (6, b"absent", true, false)]
    {
        repositories
            .push(Row {
                ci_setup: skein_fake_forge_github::service::CiSetup::DEFAULT,
                id,
                owner: Box::from(b"acme".as_slice()),
                owner_id: 22,
                organization,
                name: Box::from(name),
                private,
            })
            .expect("fixture rows");
    }
    repositories
}

fn parse(request: &[u8]) -> Request<'_> {
    let end = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").expect("complete request head");
    let mut lines = request[..end].split(|byte| *byte == b'\n').map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let mut line = lines.next().expect("request line").split(|byte| *byte == b' ');
    let method = match line.next().expect("request method") {
        b"GET" => Some(Method::Get),
        b"POST" => Some(Method::Post),
        b"PUT" => Some(Method::Put),
        b"PATCH" => Some(Method::Patch),
        b"DELETE" => Some(Method::Delete),
        _ => None,
    }
    .expect("supported request method");
    let target = line.next().expect("request target");
    assert_eq!(line.next(), Some(b"HTTP/1.1".as_slice()), "HTTP version");
    assert!(line.next().is_none(), "complete request line");
    let mut authorization = None;
    let mut version = None;
    let mut agent = false;
    let mut validator = None;
    for line in lines {
        let colon = line.iter().position(|byte| *byte == b':').expect("header colon");
        let name = &line[..colon];
        let value = line[colon + 1..].trim_ascii();
        if name.eq_ignore_ascii_case(b"Authorization") {
            authorization = Some(value);
        } else if name.eq_ignore_ascii_case(b"X-GitHub-Api-Version") {
            version = Some(value);
        } else if name.eq_ignore_ascii_case(b"If-None-Match") {
            validator = Some(value);
        } else if name.eq_ignore_ascii_case(b"User-Agent") {
            agent = true;
        }
    }
    Request { validator, method, target, authorization, version, agent, body: &request[end + 4..] }
}

fn response_head(status: u16, wall: Wall, media: Option<&str>) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut bytes = format!("HTTP/1.1 {status} {reason}\r\nDate: {}\r\n", imf(wall)).into_bytes();
    if let Some(media) = media {
        bytes.extend_from_slice(format!("Content-Type: {media}\r\n").as_bytes());
    }
    bytes
}

pub(crate) fn fixture_app() -> App {
    App {
        id: 7,
        slug: Box::from(b"test-app".as_slice()),
        owner: 1,
        permissions: Box::from([]),
        events: Box::from([]),
        key: app_key(),
        webhook: Secret::new(Box::from(b"test-webhook".as_slice())),
    }
}

/// The fake App's public test key.
#[must_use]
pub fn app_key() -> &'static [u8] {
    KEYS.first().expect("first test key")
}

/// Signs an App JWT and prefixes its whole Authorization value.
#[must_use]
pub fn bearer(key: &[u8], app: u64, issued: Wall, expires: Wall) -> Vec<u8> {
    let input = github::jwt::signing_input(app, issued, expires).expect("fixture JWT window");
    let signature = skein_tls::sign_rs256(key, &input).expect("test key signs");
    let token = github::jwt::token(&input, &signature).expect("fixture JWT");
    let mut header = b"Bearer ".to_vec();
    header.extend_from_slice(token.expose());
    header
}

// Hinnant's civil_from_days, with March starting each Gregorian cycle.
fn imf(wall: Wall) -> String {
    let seconds = wall.as_secs();
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let shifted = days + 719_468;
    let cycle = shifted / 146_097;
    let of_cycle = shifted % 146_097;
    let year_of_cycle = (of_cycle - of_cycle / 1_460 + of_cycle / 36_524 - of_cycle / 146_096) / 365;
    let day_of_year = of_cycle - (year_of_cycle * 365 + year_of_cycle / 4 - year_of_cycle / 100);
    let month = (day_of_year * 5 + 2) / 153;
    let day = day_of_year - (month * 153 + 2) / 5 + 1;
    let year = cycle * 400 + year_of_cycle + u64::from(month >= 10);
    let weekday =
        ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][usize::try_from((days + 4) % 7).expect("weekday index")];
    let month = ["Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec", "Jan", "Feb"]
        [usize::try_from(month).expect("month index")];
    format!("{weekday}, {day:02} {month} {year:04} {:02}:{:02}:{:02} GMT", time / 3600, time / 60 % 60, time % 60)
}

fn fixture_domain(seed: u64, logs: bool) -> (fake::Domain, fake::Config) {
    let mut config = crate::fixtures::fake_config();
    config.limits.repositories = 3;
    config.latency_min = Duration::ZERO;
    config.latency_max = Duration::ZERO;
    config.skew = fake::Skew::None;
    let mut domain = fake::Domain::new(&config, seed);
    for (id, name) in [(2, b"acme/widgets".as_slice()), (4, b"acme/tools"), (6, b"acme/absent")] {
        let first = fake::repository(
            &mut domain,
            &config,
            a::Setup {
                id,
                name: Box::from(name),
                default: Box::from(if id == 4 { b"queue".as_slice() } else { b"main".as_slice() }),
                tree: Box::from([]),
                labels: Box::from([]),
                checks: if logs && id == 2 {
                    crate::joint::checks(0)
                } else {
                    a::Checks {
                        contexts: Box::from([]),
                        latency_min: Duration::ZERO,
                        latency_max: Duration::ZERO,
                        silent: 0,
                        passes: 1000,
                        reruns: 0,
                        cue: None,
                    }
                },
                protection: (id == 4).then(|| a::Protection {
                    linear: false,
                    code_owners: false,
                    bypassing_users: Box::from([]),
                    queue: true,
                    branch: Box::from(b"queue".as_slice()),
                    contexts: Box::from([]),
                    approvals: 0,
                    dismiss_stale: false,
                }),
                hooked: false,
            },
        );
        fake::grant(&mut domain, name, 9, a::Permission::Write);
        if id == 4 {
            merge_fixture(&mut domain, &config, first);
        }
        fake::settings(
            &mut domain,
            name,
            a::Settings { default: Box::from(b"main".as_slice()), merge: true, squash: true, rebase: false },
        );
        if id == 2 {
            for branch in [b"alpha".as_slice(), b"feature/one"] {
                domain_reply(
                    &mut domain,
                    &config,
                    Wall::EPOCH,
                    fake::Event::Call {
                        reply_to: ReplyTo::new(Token::new(77)),
                        user: 9,
                        repository: Box::from(name),
                        op: a::Op::Write(a::Write::CreateBranch { branch: Box::from(branch), commit: first }),
                    },
                )
                .expect("fixture branch");
            }
        }
    }
    (domain, config)
}

// More timers than a fixture's queue and CI can have due at once.
const FIRES: usize = 64;

// Fixture calls have zero latency and one reply, handled by the world's same thread.
fn domain_reply(
    domain: &mut fake::Domain,
    config: &fake::Config,
    wall: Wall,
    event: fake::Event,
) -> Result<a::Answer, a::Error> {
    let env = Env { now: Time::from_nanos(wall.as_nanos()), wall, limits: *config };
    let mut out = Queue::with_capacity(fake::MAX_OUT);
    fake::step_admitted(domain, &env, event, &mut out);
    // The queue's and CI's timers due by now fire before the call's own.
    for _ in 0..FIRES {
        if !out.is_empty() {
            break;
        }
        fake::fire(domain, &env, &mut out);
    }
    let result = match out.pop().expect("zero-latency domain reply") {
        fake::Request::Reply { to, result } => {
            assert_eq!(to.into_token(), Token::new(77));
            result
        }
        fake::Request::Hook { .. } => panic!("fixture calls produce one reply"),
    };
    assert!(out.is_empty());
    domain.reclaim();
    result
}

// Tools has ordinary and queued pulls; widgets keeps the read fixtures' branches.
fn merge_fixture(domain: &mut fake::Domain, config: &fake::Config, first: u64) {
    let repository = b"acme/tools";
    fake::grant(domain, repository, 31, a::Permission::Read);
    let head = fake::commit(
        domain,
        config,
        first,
        Box::from([a::File { path: Box::from(b"change".as_slice()), content: Box::from(b"feature".as_slice()) }]),
        b"feature",
    )
    .expect("fixture commit")
    .expect("changed tree");
    for op in [
        a::Op::Write(a::Write::CreateBranch { branch: Box::from(b"main".as_slice()), commit: first }),
        a::Op::Git(a::Git::Push { branch: Box::from(b"feature".as_slice()), commit: head, expected: None }),
    ] {
        fixture_call(domain, config, repository, op);
    }
    for base in [b"main".as_slice(), b"queue"] {
        fixture_call(
            domain,
            config,
            repository,
            a::Op::Write(a::Write::OpenPull {
                title: Box::from(b"feature".as_slice()),
                body: Box::from(b"pull body".as_slice()),
                head: Box::from(b"feature".as_slice()),
                base: Box::from(base),
                opens_empty: false,
            }),
        );
    }
}
fn fixture_call(domain: &mut fake::Domain, config: &fake::Config, repository: &[u8], op: a::Op) {
    domain_reply(
        domain,
        config,
        Wall::EPOCH,
        fake::Event::Call { reply_to: ReplyTo::new(Token::new(77)), user: 9, repository: Box::from(repository), op },
    )
    .expect("fixture call");
}
