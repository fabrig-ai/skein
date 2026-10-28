use super::{app, wall};
use crate::{
    installations::{self, Kind, Reach},
    service::{self, Account, Config, Limits, Outcome, Request, Response, Row, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::github::{
    self,
    decode::{self, Step},
    document::{Document, Kind as DocumentKind, Row as DocumentRow},
    request::Method,
};
use skein_json::{tokenizer, writer};
use skein_lib::{
    Duration, Env, Intake, List, Queue, ReplyTo, Secret, Time, Token, Wall, bytes,
    stream::{Down, Up},
};

pub const VIEWER: &[u8] = br#"{"query":"query { viewer { login databaseId } }"}"#;
pub const INSTALLATIONS: &[u8] = b"/app/installations?per_page=100&page=1";
pub const REACHABLE: &[u8] = b"/installation/repositories?per_page=100&page=1";
pub const BOT: u64 = 9;
pub fn boxed(text: &[u8]) -> Box<[u8]> {
    bytes::copy_of(text)
}
pub fn listed(ids: &[u32]) -> Reach {
    let mut rows = List::with_capacity(u32::try_from(ids.len()).expect("fixture count"));
    for id in ids {
        rows.push(*id).expect("fixture reach");
    }
    Reach::Listed(rows)
}
pub fn config() -> Config {
    let mut repositories = List::with_capacity(3);
    // Deliberately not in id order: the service owns the listing's order.
    for (id, name, organization, private) in
        [(4, b"tools".as_slice(), false, false), (2, b"widgets", true, true), (6, b"absent", true, false)]
    {
        repositories
            .push(Row { id, owner: boxed(b"acme"), owner_id: 22, organization, name: boxed(name), private })
            .expect("fixture rows");
    }
    let mut accounts = List::with_capacity(2);
    for (login, id, organization) in [(b"acme".as_slice(), 22, true), (b"alice".as_slice(), 31, false)] {
        accounts.push(Account { login: boxed(login), id, organization }).expect("fixture accounts");
    }
    Config {
        rate_quota: 5000,
        rate_window: Duration::from_secs(3600),
        token_life: installations::TOKEN_LIFE,
        deliveries: crate::delivery::Config::standard(),
        base: boxed(b"http://127.0.0.1:3000"),
        blob: boxed(b"http://logs.example.test"),
        third_origin: None,
        blob_redirect: None,
        app: app(),
        app_facts: service::AppFacts::Complete,
        shown: service::Shown::Ruleset,
        ruleset_id: 17,
        pull_requests_only: false,
        required_signatures: false,
        tokens: List::with_capacity(0),
        faults: crate::Faults { reset: 0, drop: 0, server_error: 0 },
        bot: BOT,
        hook: boxed(b"https://hook.test/start"),
        repositories,
        accounts,
        person: boxed(b"alice"),
    }
}
pub fn limits() -> Limits {
    Limits {
        deliveries: crate::delivery::Limits { entries: 64, payload_bytes: 32768 },
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
            inner: 8,
            target_bytes: 1024,
        },
        installations: installations::Limits { installations: 3, tokens: 4, listed: 2 },
        pages: crate::pages::Limits { states: 3, manifest_bytes: 2048 },
        page: 2,
        body: 4096,
        response: 8192,
        blob_records: 16,
        git_records: 16,
    }
}
pub fn authorization(key: &[u8], issued: u64, expires: u64) -> Secret {
    authorization_for(7, key, issued, expires)
}
pub fn authorization_for(id: u64, key: &[u8], issued: u64, expires: u64) -> Secret {
    let input = github::jwt::signing_input(id, wall(issued), wall(expires)).expect("fixture JWT window");
    let signature = skein_tls::sign_rs256(key, &input).expect("test key signs");
    let token = github::jwt::token(&input, &signature).expect("fixture JWT");
    bearer(token.expose())
}
pub fn bearer(token: &[u8]) -> Secret {
    Secret::new(service::joined(b"Bearer ", token).expect("fixture header"))
}

pub struct Harness {
    pub service: Service,
    pub domain: fake::Domain,
    pub env: Env<fake::Config>,
    pub limits: Limits,
    pub now: Wall,
    pub calls: u32,
    pub hooks: u32,
    pub first: u64,
    pub jwt: Secret,
    pub drain_timers: bool,
}
impl Harness {
    pub fn new() -> Self {
        Self::with(limits(), config())
    }
    pub fn with(limits: Limits, config: Config) -> Self {
        Self::with_domain(limits, config, domain_config())
    }
    pub fn with_domain(limits: Limits, config: Config, fake_config: fake::Config) -> Self {
        Self::with_protection(limits, config, fake_config, None)
    }
    pub fn with_protection(
        limits: Limits,
        config: Config,
        fake_config: fake::Config,
        protection: Option<a::Protection>,
    ) -> Self {
        Self::with_checks(
            limits,
            config,
            fake_config,
            protection,
            a::Checks {
                cues: Box::new([]),
                contexts: Box::from([]),
                latency_min: Duration::ZERO,
                latency_max: Duration::ZERO,
                silent: 0,
                passes: 1000,
                reruns: 0,
                cue: None,
            },
        )
    }
    pub fn with_checks(
        limits: Limits,
        config: Config,
        fake_config: fake::Config,
        protection: Option<a::Protection>,
        checks: a::Checks,
    ) -> Self {
        let mut domain = fake::Domain::new(&fake_config, 5);
        let mut first = 0;
        for (id, name) in [(2, b"acme/widgets".as_slice()), (4, b"acme/tools")] {
            let commit = fake::repository(
                &mut domain,
                &fake_config,
                a::Setup {
                    id,
                    name: boxed(name),
                    default: boxed(b"trunk"),
                    tree: Box::from([]),
                    labels: Box::from([]),
                    checks: checks.clone(),
                    protection: if id == 2 { protection.clone() } else { None },
                    hooked: true,
                },
            );
            if id == 2 {
                first = commit;
            }
            fake::grant(&mut domain, name, BOT, a::Permission::Read);
            fake::grant(&mut domain, name, 1, a::Permission::Write);
            fake::settings(
                &mut domain,
                name,
                a::Settings { default: boxed(b"trunk"), merge: false, rebase: true, squash: true },
            );
        }
        let mut service = Service::new(config, &limits, 5).expect("valid fixture config");
        seed_installations(&mut service);
        let jwt = authorization(app().key, 1000, 1600);
        Self {
            drain_timers: true,
            service,
            domain,
            env: Env { now: Time::ZERO, wall: wall(1200), limits: fake_config },
            limits,
            now: wall(1200),
            calls: 0,
            hooks: 0,
            first,
            jwt,
        }
    }
    pub fn ask(&mut self, method: Method, target: &[u8], authorization: Option<&[u8]>, body: &[u8]) -> Response {
        self.ask_validated(method, target, authorization, body, None)
    }
    pub fn ask_validated(
        &mut self,
        method: Method,
        target: &[u8],
        authorization: Option<&[u8]>,
        body: &[u8],
        validator: Option<&[u8]>,
    ) -> Response {
        let outcome = self.service.request(
            &Request {
                validator,
                method,
                target,
                authorization,
                version: Some(github::pin::API_VERSION.as_bytes()),
                agent: true,
                body,
            },
            self.now,
            ReplyTo::new(Token::new(77)),
        );
        let mut outcome = outcome;
        // A merge refusal needs at most five domain calls (settings, merge,
        // pull, protection and author); leave room for response reads as well.
        for _call in 0..8_u32 {
            outcome = match outcome {
                Outcome::Reply(response) => return response,
                Outcome::Admit(admission) => {
                    self.service.admit(admission, &mut self.domain, &self.env, ReplyTo::new(Token::new(77)))
                }
                Outcome::Call(pending, event) => {
                    self.calls = self.calls.checked_add(1).expect("fixture calls");
                    let result = self.drive(event);
                    self.service.finish(pending, result, ReplyTo::new(Token::new(77)), &self.domain)
                }
            };
        }
        panic!("request settles within fixture call bound")
    }
    pub fn page(&mut self, method: Method, target: &[u8], body: &[u8]) -> crate::pages::Answer {
        self.service.page(&crate::pages::Page { method, target, body }, self.now)
    }
    pub fn app(&mut self, method: Method, target: &[u8], body: &[u8]) -> Response {
        let jwt = self.jwt.clone();
        self.ask(method, target, Some(jwt.expose()), body)
    }
    pub fn mint(&mut self, id: u64, contents: bool) -> github::document::Minted {
        let encoded = github::request::encode(
            &github::request::Request::Mint { installation: id, scope: github::request::Scope::Full },
            &self.limits.github,
        )
        .expect("fixture mint target");
        let response = self.app(
            Method::Post,
            &encoded.target,
            if contents { br#"{"permissions":{"contents":"write"}}"# } else { b"" },
        );
        assert_eq!(response.status, 201);
        let (_, Document::Minted(minted)) = self.decode(DocumentKind::Minted, &response) else {
            panic!("mint document");
        };
        minted
    }
    pub fn decode(&self, kind: DocumentKind, response: &Response) -> (List<DocumentRow>, Document) {
        decode_body(kind, &response.body, &self.limits.github)
    }
    pub fn branch(&mut self, name: &[u8]) {
        let event = fake::Event::Call {
            reply_to: ReplyTo::new(Token::new(77)),
            user: 1,
            repository: boxed(b"acme/widgets"),
            op: a::Op::Write(a::Write::CreateBranch { branch: boxed(name), commit: self.first }),
        };
        self.drive(event).expect("fixture branch");
        // Setup's changes are outside each read story; reads must cause zero hooks.
        self.hooks = 0;
    }
    pub fn drive(&mut self, event: fake::Event) -> Result<a::Answer, a::Error> {
        let mut out = Queue::with_capacity(fake::MAX_OUT);
        fake::step_admitted(&mut self.domain, &self.env, event, &mut out);
        let mut reply = None;
        for _tick in 0..16_u32 {
            while let Some(request) = out.pop() {
                match request {
                    fake::Request::Reply { to, result } => {
                        assert_eq!(to.into_token(), Token::new(77), "reply right passed through");
                        assert!(reply.replace(result).is_none(), "one reply per call");
                    }
                    fake::Request::Hook { .. } => {
                        self.hooks = self.hooks.checked_add(1).expect("fixture hooks");
                    }
                }
            }
            self.domain.reclaim();
            if !self.drain_timers
                && let Some(reply) = reply.take()
            {
                return reply;
            }
            let Some(deadline) = self.domain.next_deadline() else {
                return reply.expect("terminal reply");
            };
            self.env.now = deadline;
            self.env.wall = Wall::from_nanos(fake::time(&self.env.limits, deadline).as_nanos());
            fake::fire(&mut self.domain, &self.env, &mut out);
        }
        panic!("domain settles within fixture tick bound")
    }
}
pub fn decode_body(kind: DocumentKind, body: &[u8], limits: &github::limits::Limits) -> (List<DocumentRow>, Document) {
    decode_with(decode::decoder(kind, limits), body, limits)
}
pub fn decode_with(
    mut decoder: decode::Decoder,
    body: &[u8],
    limits: &github::limits::Limits,
) -> (List<DocumentRow>, Document) {
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake = Intake::with_capacity(
        u32::try_from(body.len()).expect("fixture body").max(tokenizer::largest_demand(&limits.json)),
    );
    intake.append(body).expect("fixture body fits");
    let mut rows = List::with_capacity(limits.inner);
    tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
    for _tick in 0..body.len().saturating_mul(8).saturating_add(64) {
        if let Some(event) = above.pop() {
            let (demand, step) = decoder.step(event);
            match step {
                Step::Demand => {}
                Step::Row(row) => rows.push(row).expect("fixture rows fit"),
                Step::Done(document) => return (rows, document),
                Step::Malformed(error) => panic!("fake response decodes: {error:?}"),
            }
            tokenizer::down(&mut tokenizer, &env, demand, &mut above, &mut below);
        } else {
            let Down::Demand { read, room: 0 } = below.pop().expect("tokenizer demand") else {
                panic!("JSON demands input only");
            };
            let delivery = match intake.meet(read) {
                Some(bytes) => Up::Bytes(bytes),
                None => Up::End,
            };
            tokenizer::up(&mut tokenizer, &env, delivery, &mut above, &mut below);
        }
    }
    panic!("document settles within input step bound")
}
pub fn refusal(h: &Harness, response: &Response, status: u16, message: &[u8]) {
    assert_eq!(response.status, status);
    let (_, Document::Error(error)) = h.decode(DocumentKind::Error, response) else {
        panic!("refusal document");
    };
    assert_eq!(error.message.as_ref(), message);
}
pub const REDIRECT: &[u8] = b"https://setup.test/convert";
pub const SETUP: &[u8] = b"https://setup.test/install";
pub const NAME: &[u8] = b"Example App";
pub const PERSONAL: &[u8] = b"/settings/apps/new?state=";
pub const INSTALL: &[u8] = b"/apps/example-app/installations/new?state=";
// One spare installation for alice; the dialect's full manifest has fourteen events.
pub fn page_limits() -> Limits {
    let mut limits = limits();
    limits.installations.installations = 4;
    limits.github.inner = u32::try_from(github::setup::EVENTS.len()).expect("fixture events");
    limits
}
pub fn pages() -> Harness {
    Harness::with(page_limits(), config())
}
pub fn parts(parts: &[&[u8]]) -> Box<[u8]> {
    let mut length = 0_usize;
    for part in parts {
        length = length.checked_add(part.len()).expect("fixture length");
    }
    let mut out = skein_lib::Writer::new(length);
    for part in parts {
        out.put(part).expect("fixture parts");
    }
    out.finish()
}
pub fn manifest(name: &[u8], redirect: &[u8], setup: &[u8]) -> Box<[u8]> {
    github::setup::manifest(
        &github::setup::Values {
            name,
            url: b"https://setup.test/",
            hook_url: b"https://hook.test/app",
            redirect_url: redirect,
            setup_url: setup,
        },
        &limits().github.writer,
    )
    .expect("fixture manifest")
}
pub fn form(manifest: &[u8]) -> Box<[u8]> {
    let mut encoded =
        List::with_capacity(u32::try_from(manifest.len().checked_mul(3).expect("fixture form")).expect("fixture form"));
    for byte in manifest {
        if byte.is_ascii_alphanumeric() || b"._~-".contains(byte) {
            encoded.push(*byte).expect("fixture form");
        } else if *byte == b' ' {
            encoded.push(b'+').expect("fixture form");
        } else {
            encoded.push(b'%').expect("fixture form");
            for nibble in [byte.wrapping_shr(4), byte & 15] {
                encoded.push(*b"0123456789ABCDEF".get(usize::from(nibble)).expect("hex")).expect("fixture form");
            }
        }
    }
    parts(&[b"manifest=", &encoded.into_boxed()])
}
pub fn create(h: &mut Harness, state: &[u8], name: &[u8]) -> crate::pages::Answer {
    h.page(Method::Post, &parts(&[PERSONAL, state]), &form(&manifest(name, REDIRECT, SETUP)))
}
pub fn link(answer: &crate::pages::Answer) -> Box<[u8]> {
    let marker = b"href=\"";
    let at = bytes::find(&answer.body, marker).expect("create href").checked_add(marker.len()).expect("href");
    let tail = answer.body.get(at..).expect("href");
    let end = bytes::find(tail, b"\"").expect("href end");
    let encoded = tail.get(..end).expect("href end");
    let mut out = List::with_capacity(u32::try_from(encoded.len()).expect("href bound"));
    let mut at = 0_usize;
    for _byte in 0..encoded.len() {
        let Some(tail) = encoded.get(at..) else {
            break;
        };
        if tail.is_empty() {
            break;
        }
        let mut consumed = 1;
        let mut byte = *tail.first().expect("href byte");
        for (entity, value) in
            [(b"&amp;".as_slice(), b'&'), (b"&lt;", b'<'), (b"&gt;", b'>'), (b"&quot;", b'"'), (b"&#39;", b'\'')]
        {
            if tail.starts_with(entity) {
                consumed = entity.len();
                byte = value;
                break;
            }
        }
        out.push(byte).expect("href fits");
        at = at.checked_add(consumed).expect("href cursor");
    }
    out.into_boxed()
}
pub fn code(answer: &crate::pages::Answer) -> Secret {
    let link = link(answer);
    let at = bytes::find(&link, b"?code=").expect("conversion query").checked_add(6).expect("code start");
    let code = link.get(at..at.checked_add(20).expect("code end")).expect("code width");
    Secret::new(boxed(code))
}
pub fn conversion_target(code: &Secret) -> Box<[u8]> {
    parts(&[b"/app-manifests/", code.expose(), b"/conversions"])
}
pub fn convert(h: &mut Harness, code: &Secret) -> Response {
    h.ask(Method::Post, &conversion_target(code), None, b"")
}
pub fn conversion(h: &Harness, response: &Response) -> github::document::Conversion {
    // The PEM is longer than the manifest's scalar bound; conversion readers admit it whole.
    let mut limits = h.limits.github;
    limits.string_bytes = limits.json.string;
    let (_, Document::Conversion(row)) = decode_body(DocumentKind::Conversion, &response.body, &limits) else {
        panic!("conversion document");
    };
    row
}
pub fn converted_with_installations() -> Harness {
    let mut h = pages();
    let answer = create(&mut h, b"s1", NAME);
    let response = convert(&mut h, &code(&answer));
    assert_eq!(response.status, 201);
    h.jwt = authorization_for(8, crate::keys::KEYS.first().expect("first key"), 1000, 1600);
    // These installation tests start after this App acquired the fixture's
    // three installations, independently of the preceding App's records.
    h.service.installations = installations::Installations::new(&h.limits.installations, 5);
    seed_installations(&mut h.service);
    h
}
pub fn page_refusal(h: &Harness, answer: crate::pages::Answer, status: u16, message: &[u8]) {
    assert!(answer.location.is_none(), "refusals never redirect");
    assert_eq!(answer.media, crate::pages::Media::None);
    refusal(
        h,
        &Response {
            rate: None,
            etag: None,
            content_type: service::ContentType::Json,
            status: answer.status,
            body: answer.body,
            link: None,
            location: None,
        },
        status,
        message,
    );
}

pub fn domain_config() -> fake::Config {
    fake::Config {
        limits: fake::Limits {
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
            files: 1,
            statuses: 1,
            contexts: 1,
            pages: 1,
            name_bytes: 64,
            title_bytes: 64,
            body_bytes: 128,
            content_bytes: 128,
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
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: fake::Skew::None,
        status_updates: false,
        edit_updates: false,
    }
}

fn seed_installations(service: &mut Service) {
    let store = service.installations_mut();
    assert_eq!(store.install(22, b"acme", Kind::Organization, listed(&[2, 4]), wall(1000)), Ok(1));
    assert_eq!(store.install(23, b"example", Kind::User, Reach::All, wall(1001)), Ok(2));
    assert_eq!(store.install(24, b"other", Kind::User, Reach::All, wall(1002)), Ok(3));
    assert!(store.suspend(3, true), "third installation exists");
}
