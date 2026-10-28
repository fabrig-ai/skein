//! One seeded loop drives the real client and issuer. The world owns grants.
use skein_heap::Span;
use skein_lib::{
    CryptoRng, Duration, List, Queue, Secret, Time, Wall, Writer,
    base64::{self, Alphabet},
    bytes,
};
use skein_oauth as oauth;
use skein_oauth_issuer as issuer;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const ISSUER: &[u8] = b"http://127.0.0.1:8123";
const AUTHORIZE: &[u8] = b"http://127.0.0.1:8123/authorize";
const TOKEN: &[u8] = b"http://127.0.0.1:8123/token";
const USERINFO: &[u8] = b"http://127.0.0.1:8123/userinfo";
const KEYS: &[u8] = b"http://127.0.0.1:8123/keys";
const DISCOVERY: &[u8] = b"http://127.0.0.1:8123/.well-known/openid-configuration";
const REDIRECT: &[u8] = b"https://app.example.com/callback";
const CODE_LIFE: Duration = Duration::from_secs(60);
const ACCESS_LIFE: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Hash)]
pub enum Story {
    SignIn,
    Expired,
    AnotherKey,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub trace: u64,
    pub state: u64,
    pub failure: Option<oauth::Failure>,
    pub subject: Option<Box<[u8]>>,
}
fn boxed(input: &[u8]) -> Box<[u8]> {
    bytes::copy_of(input)
}
fn secret(input: &[u8]) -> Secret {
    Secret::new(boxed(input))
}
fn at(seconds: u64) -> Time {
    Time::from_nanos(seconds.checked_mul(1_000_000_000).expect("time"))
}
fn wall(seconds: u64) -> Wall {
    Wall::from_nanos(seconds.checked_mul(1_000_000_000).expect("wall"))
}
// RFC 7515 Appendix A.2 and RFC 9500 section 2.1 testRSA2048 are public
// test keys, never secrets. This is this crate's only fixture key seam.
fn test_key(second: bool) -> Secret {
    let name =
        if second { "rfc9500-2.1-testRSA2048-public-test-key.pk8.der" } else { "rfc7515-a2-public-test-key.pk8.der" };
    Secret::new(
        std::fs::read(std::path::Path::new("tests/fixtures").join(name))
            .expect("published PKCS#8 test key")
            .into_boxed_slice(),
    )
}
fn issuer_limits() -> issuer::Limits {
    issuer::Limits {
        clients: 1,
        uri_bytes: 64,
        client_bytes: 16,
        request_bytes: 1024,
        state_bytes: 43,
        token_bytes: 43,
        claim_bytes: 64,
    }
}
fn make_issuer(second: bool) -> issuer::Issuer {
    let mut clients = List::with_capacity(1);
    clients
        .push(issuer::Client { id: boxed(b"client"), secret: secret(b"secret"), redirect_uri: boxed(REDIRECT) })
        .expect("client");
    issuer::Issuer::new(
        issuer::Settings {
            issuer: boxed(ISSUER),
            authorization_endpoint: boxed(AUTHORIZE),
            token_endpoint: boxed(TOKEN),
            userinfo_endpoint: boxed(USERINFO),
            jwks_uri: boxed(KEYS),
            clients,
            key: test_key(second),
        },
        issuer_limits(),
    )
    .expect("issuer")
}
fn documents() -> oauth::Limits {
    oauth::Limits {
        document_bytes: 4096,
        string_bytes: 2048,
        token_bytes: 64,
        client_bytes: 16,
        detail_bytes: 64,
        record_bytes: 4096,
        depth: 8,
        tokens: 128,
        issued_client_bytes: 16,
        granted_bytes: 64,
        subject_bytes: 64,
        id_token_bytes: 2048,
    }
}
fn client_limits() -> oauth::ClientLimits {
    oauth::ClientLimits {
        document: documents(),
        uri_bytes: 64,
        scope_bytes: 64,
        state_bytes: 43,
        code_bytes: 43,
        url_bytes: 1024,
        request_bytes: 1024,
        parameter_bytes: 0,
        resource_bytes: 0,
        nonce_bytes: 43,
        metadata_bytes: 2048,
        key_set_bytes: 2048,
        keys: 1,
        modulus_bits: 2048,
        skew: Duration::from_secs(60),
        sign_in_time: Duration::from_secs(600),
        request_time: Duration::from_secs(400),
        backoff_base: Duration::from_secs(1),
        backoff_ceiling: Duration::from_secs(4),
        max_attempts: 1,
    }
}
fn registration() -> oauth::Registration {
    oauth::Registration {
        authorization_url: boxed(AUTHORIZE),
        token_endpoint: boxed(TOKEN),
        client_id: boxed(b"client"),
        redirect_uri: boxed(REDIRECT),
        scope: boxed(b"openid profile email"),
        wire: oauth::WireFormat::Form,
        client_secret: Some(boxed(b"secret")),
        pkce_for_confidential: true,
        metadata_claim: None,
        terms: Some(oauth::Terms {
            parameters: Box::new([]),
            openid: Some(oauth::OpenId { issuer: boxed(ISSUER), key_set: None }),
            issued_client_id: false,
            varying_port: false,
            resource: None,
            required_scopes: None,
            revocation_endpoint: None,
            sign_in_again: Box::new([]),
        }),
    }
}
fn person() -> issuer::Person {
    issuer::Person {
        subject: boxed(b"person-42"),
        username: Some(boxed(b"someone")),
        email: Some(issuer::Email { address: boxed(b"someone@example.com"), verified: true }),
        name: Some(boxed(b"Some One")),
    }
}
fn draw(rng: &mut CryptoRng) -> Secret {
    let mut raw = [0_u8; 32];
    rng.fill(&mut raw);
    let mut out = Writer::new(43);
    base64::encode(Alphabet::Url, &raw, &mut out).expect("base64url draw");
    Secret::new(out.finish())
}
fn json_string(input: &[u8], name: &[u8]) -> Box<[u8]> {
    let json = oauth::Json::from_bytes(input, &documents()).expect("peer JSON");
    let mut wanted = false;
    for token in json.as_tokens() {
        match token {
            skein_json::Token::Key(key) => wanted = key.as_ref() == name,
            skein_json::Token::String(value) if wanted => return value.clone(),
            skein_json::Token::ObjectStart
            | skein_json::Token::ObjectEnd
            | skein_json::Token::ArrayStart
            | skein_json::Token::ArrayEnd
            | skein_json::Token::String(_)
            | skein_json::Token::Number(_)
            | skein_json::Token::True
            | skein_json::Token::False
            | skein_json::Token::Null => {}
        }
    }
    panic!("string member missing")
}
fn query_value(query: &[u8], name: &[u8]) -> Box<[u8]> {
    let mut found = None;
    for field in query.split(|byte| *byte == b'&') {
        let at = field.iter().position(|byte| *byte == b'=').expect("form equals");
        if &field[..at] != name {
            continue;
        }
        assert!(found.is_none(), "one redirect parameter");
        let mut decoded = Vec::new();
        let mut bytes = field[at + 1..].iter().copied();
        while let Some(byte) = bytes.next() {
            decoded.push(if byte == b'%' {
                let high = char::from(bytes.next().expect("high hex")).to_digit(16).expect("hex");
                let low = char::from(bytes.next().expect("low hex")).to_digit(16).expect("hex");
                u8::try_from(high * 16 + low).expect("decoded byte")
            } else if byte == b'+' {
                b' '
            } else {
                byte
            });
        }
        found = Some(decoded.into_boxed_slice());
    }
    found.expect("redirect parameter")
}
fn decoded_claims(jwt: &[u8]) -> Box<[u8]> {
    let encoded = jwt.split(|byte| *byte == b'.').nth(1).expect("payload");
    base64::decode(Alphabet::Url, encoded, 4096).expect("base64url claims")
}

// Each measured call includes retained heap and all its output and temporary
// allocations. Inputs and queue storage belong to the caller; returned bytes
// are subtracted from retained heap after the peak has been checked.
fn measured<T>(live: &mut i64, bound: u64, output_bytes: impl FnOnce(&T) -> usize, call: impl FnOnce() -> T) -> T {
    let span = Span::start();
    let output = call();
    let grown = span.end();
    let peak = live.checked_add(grown.peak).expect("heap sum");
    assert!(peak >= 0 && u64::try_from(peak).expect("positive heap") <= bound, "heap {peak} exceeds bound {bound}");
    let returned = i64::try_from(output_bytes(&output)).expect("output length");
    *live = live.checked_add(grown.net).expect("net heap").checked_sub(returned).expect("handed bytes");
    output
}
#[derive(Hash)]
struct Code {
    value: Secret,
    authorization: issuer::Authorization,
    person: issuer::Person,
    expires: Time,
}
#[derive(Hash)]
struct Access {
    value: Secret,
    person: issuer::Person,
    expires: Time,
}
struct World {
    issuer: issuer::Issuer,
    issuer_live: i64,
    client: oauth::Client,
    client_live: i64,
    rng: CryptoRng,
    code: Option<Code>,
    access: Option<Access>,
    now: Time,
    wall: Wall,
    trace: DefaultHasher,
    terminal: u32,
    pending: bool,
    wrong_keys: Option<Box<[u8]>>,
}
impl World {
    fn new(seed: u64, story: Story) -> World {
        let mut issuer_live = 0;
        let issuer = measured(
            &mut issuer_live,
            issuer::worst_case(&issuer_limits()).expect("issuer bound"),
            |_| 0,
            || make_issuer(false),
        );
        let mut client_live = 0;
        let client = measured(
            &mut client_live,
            oauth::client_worst_case(&client_limits()).expect("client bound"),
            |_| 0,
            || oauth::Client::new(client_limits()).expect("client"),
        );
        let wrong_keys = if matches!(story, Story::AnotherKey) {
            let second = make_issuer(true);
            let other = second.key_set();
            let source_kid = json_string(&other, b"kid");
            let signer_kid = json_string(&issuer.key_set(), b"kid");
            // Keep the first key's selector while substituting the second
            // published key, so this story reaches RSA signature verification.
            Some(
                String::from_utf8(other.into_vec())
                    .expect("ASCII JWK")
                    .replace(
                        std::str::from_utf8(&source_kid).expect("kid"),
                        std::str::from_utf8(&signer_kid).expect("kid"),
                    )
                    .into_bytes()
                    .into_boxed_slice(),
            )
        } else {
            None
        };
        World {
            issuer,
            issuer_live,
            client,
            client_live,
            rng: CryptoRng::new(&secret(&[7; 32]), seed).expect("seed"),
            code: None,
            access: None,
            now: at(1),
            wall: wall(100),
            trace: DefaultHasher::new(),
            terminal: 0,
            pending: false,
            wrong_keys,
        }
    }
    fn client_step(&mut self, event: oauth::Event) -> Option<oauth::Request> {
        let input_bytes = event_bytes(&event);
        // Http bodies were handed to the client before its call. Other event
        // allocations are supplied at their configured maxima in this bound.
        self.client_live =
            self.client_live.checked_add(i64::try_from(input_bytes).expect("input")).expect("input heap");
        let bound = oauth::client_worst_case(&client_limits()).expect("bound");
        measured::<Option<oauth::Request>>(
            &mut self.client_live,
            bound,
            |requests| request_bytes(requests.as_ref()),
            || {
                // The temporary output queue grows and releases inside this call.
                let mut out = Queue::uncapped();
                self.client.step(event, &mut out);
                out.pop()
            },
        )
    }
    fn answer(&mut self, request: &oauth::HttpRequest) -> oauth::HttpResponse {
        assert!(!self.pending, "one HTTP request at once");
        self.pending = true;
        request.endpoint.hash(&mut self.trace);
        request.body.hash(&mut self.trace);
        let bound = issuer::worst_case(&issuer_limits()).expect("bound");
        let (status, body) = match request.method {
            oauth::Method::Get => {
                assert!(request.body.is_empty(), "GET has no credentials");
                let body = if request.endpoint.as_ref() == DISCOVERY {
                    measured::<Box<[u8]>>(&mut self.issuer_live, bound, |body| body.len(), || self.issuer.discovery())
                } else if request.endpoint.as_ref() == KEYS {
                    self.wrong_keys.clone().unwrap_or_else(|| {
                        measured::<Box<[u8]>>(&mut self.issuer_live, bound, |body| body.len(), || self.issuer.key_set())
                    })
                } else {
                    panic!("unknown GET route")
                };
                (200, body)
            }
            oauth::Method::Post => {
                assert_eq!(request.endpoint.as_ref(), TOKEN, "fixed token route");
                self.post(request)
            }
        };
        status.hash(&mut self.trace);
        body.hash(&mut self.trace);
        self.pending = false;
        oauth::HttpResponse {
            id: request.id,
            status,
            body,
            retry_after: Duration::ZERO,
            evidence: oauth::HttpEvidence::Response,
            now: self.now,
            wall: self.wall,
        }
    }
    fn post(&mut self, request: &oauth::HttpRequest) -> (u16, Box<[u8]>) {
        let bound = issuer::worst_case(&issuer_limits()).expect("bound");
        let span = Span::start();
        let parsed = self.issuer.token_request(None, Some(request.content_type), &request.body);
        let grown = span.end();
        assert!(
            u64::try_from(self.issuer_live.checked_add(grown.peak).expect("heap")).expect("heap") <= bound,
            "token request memory"
        );
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                let answer = error.answer();
                return (answer.status, answer.body);
            }
        };
        // Spend on first presentation, before checking redirect or PKCE.
        let code =
            if self.code.as_ref().is_some_and(|code| code.value == parsed.code) { self.code.take() } else { None };
        let Some(code) = code else {
            let answer = issuer::TokenError::InvalidGrant.answer();
            return (answer.status, answer.body);
        };
        let redeemed =
            measured(&mut self.issuer_live, bound, |_| 0, || self.issuer.redeem(&parsed, &code.authorization));
        if self.now >= code.expires || redeemed.is_err() {
            let answer = issuer::TokenError::InvalidGrant.answer();
            return (answer.status, answer.body);
        }
        let value = draw(&mut self.rng);
        let body = measured::<Box<[u8]>>(
            &mut self.issuer_live,
            bound,
            |body| body.len(),
            || self.issuer.tokens(&code.authorization, &code.person, &value, ACCESS_LIFE, self.wall).expect("tokens"),
        );
        self.access = Some(Access { value, person: code.person, expires: self.now.saturating_add(ACCESS_LIFE) });
        (200, body)
    }
    fn authorize(&mut self, url: &[u8]) -> oauth::Event {
        let query =
            url.strip_prefix(AUTHORIZE).expect("registered authorization route").strip_prefix(b"?").expect("query");
        url.hash(&mut self.trace);
        let bound = issuer::worst_case(&issuer_limits()).expect("bound");
        let span = Span::start();
        let authorization = self.issuer.authorize(query).expect("valid authorization");
        let grown = span.end();
        assert!(
            u64::try_from(self.issuer_live.checked_add(grown.peak).expect("heap")).expect("heap") <= bound,
            "authorization memory"
        );
        let value = draw(&mut self.rng);
        let location = measured::<Box<[u8]>>(
            &mut self.issuer_live,
            bound,
            |body| body.len(),
            || self.issuer.redirect(&authorization, &value).expect("redirect"),
        );
        assert!(location.starts_with(REDIRECT), "browser reaches registered redirect");
        location.hash(&mut self.trace);
        let at = location.iter().position(|byte| *byte == b'?').expect("redirect query");
        let (uri, query) = location.split_at(at);
        let query = &query[1..];
        assert_eq!(uri, REDIRECT, "exact callback URL");
        let state = query_value(query, b"state");
        let code = query_value(query, b"code");
        assert_eq!(state.as_ref(), authorization.state.as_deref().expect("state"));
        assert_eq!(code.as_ref(), value.expose(), "wire code");
        let event = oauth::Event::Redirected {
            uri: boxed(uri),
            state,
            code: Some(code),
            error: None,
            client_id: None,
            now: self.now,
        };
        self.code = Some(Code { value, authorization, person: person(), expires: self.now.saturating_add(CODE_LIFE) });
        event
    }
    fn check_record(&mut self, record: &oauth::SavedToken) {
        assert!(record.refresh_token.is_none() && record.refresh_state().is_none(), "access-only sign-in");
        assert_eq!(record.subject.as_deref(), Some(person().subject.as_ref()), "record subject");
        let claims = decoded_claims(record.id_token.as_deref().expect("ID token"));
        assert_eq!(json_string(&claims, b"preferred_username"), person().username.expect("username"));
        assert_eq!(json_string(&claims, b"email"), person().email.expect("email").address);
        assert_eq!(json_string(&claims, b"name"), person().name.expect("name"));
        let (status, body) = self.userinfo(USERINFO, &record.access_token);
        assert_eq!(status, 200);
        assert_eq!(json_string(&body, b"sub").as_ref(), record.subject.as_deref().expect("subject"));
        assert_eq!(json_string(&claims, b"sub"), json_string(&body, b"sub"), "ID token and userinfo agree");
        record.hash(&mut self.trace);
        body.hash(&mut self.trace);
    }
    fn userinfo(&mut self, endpoint: &[u8], token: &[u8]) -> (u16, Box<[u8]>) {
        assert_eq!(endpoint, USERINFO, "userinfo route");
        let header = [b"Bearer ".as_slice(), token].concat();
        let bound = issuer::worst_case(&issuer_limits()).expect("bound");
        let supplied = measured::<Option<Secret>>(
            &mut self.issuer_live,
            bound,
            |value| value.as_ref().map_or(0, Secret::len),
            || self.issuer.bearer(Some(&header)),
        );
        let Some(access) =
            self.access.as_ref().filter(|access| Some(&access.value) == supplied.as_ref() && self.now < access.expires)
        else {
            let answer = self.issuer.userinfo_refused();
            return (answer.status, answer.body);
        };
        let bound = issuer::worst_case(&issuer_limits()).expect("bound");
        (
            200,
            measured::<Box<[u8]>>(
                &mut self.issuer_live,
                bound,
                |body| body.len(),
                || self.issuer.userinfo(&access.person).expect("userinfo"),
            ),
        )
    }
}
fn event_bytes(event: &oauth::Event) -> usize {
    match event {
        oauth::Event::SignIn { registration, state, verifier, nonce, .. } => {
            let mut count =
                state.len() + verifier.as_ref().map_or(0, |v| v.len()) + nonce.as_ref().map_or(0, |v| v.len());
            count += registration.authorization_url.len()
                + registration.token_endpoint.len()
                + registration.client_id.len()
                + registration.redirect_uri.len()
                + registration.scope.len()
                + registration.client_secret.as_ref().map_or(0, |v| v.len());
            if let Some(terms) = &registration.terms
                && let Some(openid) = &terms.openid
            {
                count += openid.issuer.len();
            }
            count
        }
        oauth::Event::Redirected { uri, state, code, .. } => {
            uri.len() + state.len() + code.as_ref().map_or(0, |v| v.len())
        }
        oauth::Event::Http(answer) => answer.body.len(),
        oauth::Event::Tick { .. } | oauth::Event::Cancel | oauth::Event::Reset => 0,
        oauth::Event::Refresh { .. } | oauth::Event::Revoke { .. } => unreachable!("sign-in world"),
    }
}
fn request_bytes(request: Option<&oauth::Request>) -> usize {
    match request {
        Some(oauth::Request::Http(request)) => {
            request.endpoint.len().checked_add(request.body.len()).expect("request bytes")
        }
        Some(oauth::Request::Visit { url }) => url.len(),
        Some(oauth::Request::Tokens { record, missing }) => record
            .access_token
            .len()
            .checked_add(record.id_token.as_ref().map_or(0, |value| value.len()))
            .expect("record bytes")
            .checked_add(record.subject.as_ref().map_or(0, |value| value.len()))
            .expect("record bytes")
            .checked_add(record.granted_scopes.as_ref().map_or(0, |value| value.len()))
            .expect("record bytes")
            .checked_add(missing.len())
            .expect("missing scopes"),
        Some(oauth::Request::Revoked { .. } | oauth::Request::Failed { .. }) | None => 0,
    }
}
pub fn run(seed: u64, story: Story) -> Outcome {
    let mut world = World::new(seed, story);
    let state = draw(&mut world.rng);
    let verifier = draw(&mut world.rng);
    let nonce = draw(&mut world.rng);
    let mut event = oauth::Event::SignIn {
        registration: registration(),
        key: 7,
        generation: 0,
        state: boxed(state.expose()),
        verifier: Some(boxed(verifier.expose())),
        nonce: Some(boxed(nonce.expose())),
        port: None,
        issued_client_id: None,
        id_token: None,
        subject: None,
        now: world.now,
    };
    let mut subject = None;
    let mut failure = None;
    for _ in 0..8 {
        let request = world.client_step(event);
        match request {
            Some(oauth::Request::Http(request)) => {
                let token = request.method == oauth::Method::Post;
                let mut answer = world.answer(&request);
                if token && matches!(story, Story::Expired) {
                    // Token delivered after exp + skew, within transport deadline.
                    world.now = at(362);
                    world.wall = wall(461);
                    answer.now = world.now;
                    answer.wall = world.wall;
                }
                if token {
                    let (status, body) = world.post(&request);
                    assert_eq!(status, 400, "code is spent once");
                    assert_eq!(body.as_ref(), b"{\"error\":\"invalid_grant\"}");
                }
                event = oauth::Event::Http(answer);
            }
            Some(oauth::Request::Visit { url }) => {
                event = world.authorize(&url);
            }
            Some(oauth::Request::Tokens { record, missing }) => {
                world.terminal += 1;
                assert!(missing.is_empty(), "all requested scopes");
                world.check_record(&record);
                subject = record.subject;
                break;
            }
            Some(oauth::Request::Failed { failure: why }) => {
                world.terminal += 1;
                why.hash(&mut world.trace);
                failure = Some(why);
                break;
            }
            Some(oauth::Request::Revoked { .. }) | None => panic!("world did not reach a sign-in terminal"),
        }
    }
    assert_eq!(world.terminal, 1, "exactly one terminal at seed {seed}");
    assert!(world.client.is_done() && !world.pending && world.code.is_none(), "world settled");
    assert!(world.client_step(oauth::Event::Tick { now: at(1000) }).is_none(), "terminal remains last");
    match story {
        Story::SignIn => {
            assert_eq!(failure, None);
            assert_eq!(subject.as_deref(), Some(person().subject.as_ref()));
        }
        Story::Expired => {
            assert_eq!(failure, Some(oauth::Failure::Unverified { check: oauth::Check::Expiry }));
            assert!(subject.is_none());
        }
        Story::AnotherKey => {
            assert_eq!(failure, Some(oauth::Failure::Unverified { check: oauth::Check::Signature }));
            assert!(subject.is_none());
        }
    }
    assert_eq!(world.client_live, 0, "terminal client released its held heap");
    let token = boxed(world.access.as_ref().expect("issued access").value.expose());
    let expires = world.access.as_ref().expect("issued access").expires;
    world.now = world.now.max(expires);
    assert_eq!(world.userinfo(USERINFO, &token), (401, Box::from(b"".as_slice())), "access expires at five minutes");
    assert_eq!(world.userinfo(USERINFO, b"unknown"), (401, Box::from(b"".as_slice())), "unknown access refused");
    world.access = None;
    let mut digest = DefaultHasher::new();
    world.rng.hash(&mut digest);
    world.code.hash(&mut digest);
    world.access.hash(&mut digest);
    world.now.hash(&mut digest);
    world.wall.hash(&mut digest);
    world.terminal.hash(&mut digest);
    world.client.is_done().hash(&mut digest);
    Outcome { trace: world.trace.finish(), state: digest.finish(), failure, subject }
}

#[test]
fn code_ledger_refuses_unknown_expired_and_failed_first_presentations() {
    const QUERY: &[u8] = b"http://127.0.0.1:8123/authorize?client_id=client&redirect_uri=https%3A%2F%2Fapp.example.com%2Fcallback&response_type=code&scope=openid&code_challenge_method=S256&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    for expired in [false, true] {
        let mut world = World::new(47, Story::SignIn);
        let event = world.authorize(&[QUERY, b"&state=state"].concat());
        let oauth::Event::Redirected { code: Some(code), .. } = event else { panic!("code") };
        let mut request = oauth::HttpRequest {
            id: 1, method: oauth::Method::Post, endpoint: boxed(TOKEN),
            content_type: b"application/x-www-form-urlencoded",
            body: format!("grant_type=authorization_code&client_id=client&client_secret=secret&code=unknown&redirect_uri=https%3A%2F%2Fapp.example.com%2Fcallback&code_verifier={VERIFIER}").into_bytes().into_boxed_slice(),
            deadline: at(400),
        };
        let (status, body) = world.post(&request);
        assert_eq!((status, body.as_ref()), (400, b"{\"error\":\"invalid_grant\"}".as_slice()));
        assert!(world.code.is_some(), "unknown code does not spend a different code");
        let value = std::str::from_utf8(&code).expect("drawn code");
        let body =
            String::from_utf8(request.body.into_vec()).expect("form").replace("code=unknown", &format!("code={value}"));
        if expired {
            world.now = at(61);
            request.body = body.into_bytes().into_boxed_slice();
        } else {
            request.body = body.replace(VERIFIER, &"x".repeat(43)).into_bytes().into_boxed_slice();
        }
        let (status, body) = world.post(&request);
        assert_eq!((status, body.as_ref()), (400, b"{\"error\":\"invalid_grant\"}".as_slice()));
        assert!(world.code.is_none(), "expired or failed presentation spends its code");
        let body = String::from_utf8(request.body.into_vec()).expect("form").replace(&"x".repeat(43), VERIFIER);
        request.body = body.into_bytes().into_boxed_slice();
        assert_eq!(world.post(&request).0, 400, "a spent code stays spent");
        assert!(world.access.is_none(), "no access issued for a refused grant");
    }
}
