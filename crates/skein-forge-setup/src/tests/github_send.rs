use super::{fixtures, github_fixtures, host};
use crate::{
    call::Auth,
    host::{
        Forge, Kind as ForgeKind, Limits, Transport,
        connection::{Connection, Refused, kind_of},
    },
    reconcile::Reply,
};
use skein_forge::github::{
    self as dialect,
    document::{Document, Kind, Row},
    request::{Request, Scope},
};
use skein_lib::{
    Duration, Env, Queue, Secret, Time, Token, Wall, base64, bytes,
    stream::{Down, Held, Up},
};

const AGENT: &[u8] = b"kit/0.0.0";
const TOKEN: &[u8] = b"installation-secret-token";
const MINTED: &[u8] = b"new-installation-secret";
// Whole seconds make the skew and lifetime visible in the claims (5.3).
const WALL: Wall = Wall::from_nanos(1_000_000_000_000);
const LATER: Wall = Wall::from_nanos(1_001_000_000_000);

fn forge() -> Forge {
    Forge {
        kind: ForgeKind::GitHub(github_fixtures::given()),
        address: skein_io::Addr::Ip(([127, 0, 0, 1], 3001).into()),
        transport: Transport::Plaintext { name: None },
    }
}
fn app() -> Auth {
    Auth::App { id: 7, key: fixtures::secret(skein_fake_forge_github::keys::KEYS[0]) }
}
fn key() -> Secret {
    fixtures::secret(b"\x30\x03\x02\x01\x00")
}
fn bearer() -> Auth {
    Auth::Bearer(fixtures::secret(TOKEN))
}
fn reachable() -> Request {
    Request::Reachable { page: 1, per_page: 4 }
}
fn conversion() -> Request {
    Request::Conversion { code: b"code".as_slice().into() }
}
fn sent(request: &Request, auth: &Auth, wall: Wall, limits: &Limits) -> Result<Connection, Refused> {
    Connection::github(Token::new(1), &forge(), AGENT, &request.clone().into(), auth, wall, limits)
}
fn connection(request: &Request, auth: &Auth) -> Connection {
    sent(request, auth, WALL, &host::limits()).expect("request fits")
}
fn header(conn: &Connection, name: &[u8]) -> Option<Box<[u8]>> {
    for h in &conn.call.as_ref().expect("unsent call").headers {
        if h.name.eq_ignore_ascii_case(name) {
            return Some(h.value.clone());
        }
    }
    None
}
fn authorization(conn: &Connection) -> Box<[u8]> {
    header(conn, b"Authorization").expect("authorized request")
}
pub(crate) fn answering(mut conn: Connection, head: &[u8], body: &[u8]) -> Connection {
    let env = Env { now: Time::ZERO, wall: WALL, limits: host::limits() };
    let mut bytes = skein_lib::Writer::new(head.len().checked_add(body.len()).unwrap());
    bytes.put(head).unwrap();
    bytes.put(body).unwrap();
    let mut transcript = Held::new(bytes.finish());
    let mut io = Queue::with_capacity(4);
    conn.socket = Some(Token::new(2));
    conn.connect(&env);
    for _ in 0_u32..10000 {
        let progress = conn.pump(&env, &mut io);
        match io.pop() {
            Some(skein_io::Request::Stream { down: Down::Demand { read, room }, .. }) => {
                if room > 0 {
                    conn.stream(&env, Up::Room);
                } else if let Some(event) = transcript.answer(read) {
                    conn.stream(&env, event);
                }
            }
            Some(skein_io::Request::Stream { down: Down::Send(_) | Down::Finish, .. }) => (),
            Some(skein_io::Request::Close { .. }) => return conn,
            Some(request) => panic!("unexpected io: {request:?}"),
            None => {
                if !progress {
                    return conn;
                }
            }
        }
    }
    panic!("connection did not idle");
}
fn answer(request: &Request, auth: &Auth, body: &[u8]) -> Reply {
    answering(connection(request, auth), b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n", body).result.expect("answer")
}
fn claims(conn: &Connection) -> Box<[u8]> {
    let auth = authorization(conn);
    let first = bytes::find(&auth, b".").expect("header dot").checked_add(1).unwrap();
    let second = bytes::find_from(&auth, b".", first).expect("signature dot");
    base64::decode(base64::Alphabet::Url, &auth[first..second], 128).expect("claims base64url")
}
#[test]
fn an_app_request_carries_a_jwt_the_key_signed() {
    // The first fixed key's DER: 4-byte sequence header, 3-byte version,
    // 4-byte integer header, sign padding, 256-byte modulus, then a 3-byte exponent.
    const MODULUS: core::ops::Range<usize> = 12..268;
    const EXPONENT: core::ops::Range<usize> = 270..273;
    let conn = connection(&Request::App, &app());
    let auth = authorization(&conn);
    let token = auth.strip_prefix(b"Bearer ").expect("bearer");
    let first = bytes::find(token, b".").expect("header dot");
    let dot = bytes::find_from(token, b".", first + 1).expect("signature dot");
    let input = &token[..dot];
    let signature = base64::decode(base64::Alphabet::Url, &token[dot + 1..], 256).expect("signature base64url");
    assert_eq!(claims(&conn).as_ref(), br#"{"iat":940,"exp":1540,"iss":"7"}"#);
    let signed = skein_tls::sign_rs256(skein_fake_forge_github::keys::KEYS[0], input).expect("test key signs");
    assert_eq!(signature.as_ref(), signed);
    let der = skein_fake_forge_github::keys::KEYS[0];
    assert!(skein_tls::verify_rs256(&der[MODULUS], &der[EXPONENT], input, &signature));
}
#[test]
fn a_second_connection_signs_a_fresh_jwt() {
    let auth = app();
    let first = sent(&Request::App, &auth, WALL, &host::limits()).unwrap();
    let second = sent(&Request::App, &auth, LATER, &host::limits()).unwrap();
    assert!(authorization(&first) != authorization(&second));
    assert_eq!(claims(&first).as_ref(), br#"{"iat":940,"exp":1540,"iss":"7"}"#);
    assert_eq!(claims(&second).as_ref(), br#"{"iat":941,"exp":1541,"iss":"7"}"#);
}
#[test]
fn an_installation_request_carries_its_bearer_token() {
    assert!(authorization(&connection(&reachable(), &bearer())).as_ref() == b"Bearer installation-secret-token");
}
#[test]
fn a_request_without_auth_carries_no_authorization_header() {
    for request in [conversion(), Request::BotUser { slug: b"kit".as_slice().into() }] {
        assert_eq!(header(&connection(&request, &Auth::None), b"Authorization"), None);
    }
}
#[test]
fn every_github_request_carries_host_agent_accept_and_the_api_version() {
    for (request, auth) in [(Request::App, app()), (reachable(), bearer()), (conversion(), Auth::None)] {
        let conn = connection(&request, &auth);
        for (name, value) in [
            (b"Host".as_slice(), b"127.0.0.1:3001".as_slice()),
            (b"User-Agent", AGENT),
            (b"Accept", b"application/vnd.github+json"),
            (b"X-GitHub-Api-Version", dialect::pin::API_VERSION.as_bytes()),
        ] {
            assert_eq!(header(&conn, name).as_deref(), Some(value));
        }
    }
}
#[test]
fn a_request_with_a_body_carries_content_type_and_its_length() {
    let conn = connection(&Request::SetHook { url: b"https://kit.test/".as_slice().into() }, &app());
    let body = br#"{"url":"https://kit.test/"}"#;
    assert_eq!(header(&conn, b"Content-Type").as_deref(), Some(b"application/json".as_slice()));
    assert_eq!(conn.upload.as_deref(), Some(body.as_slice()));
    assert_eq!(conn.call.as_ref().unwrap().body, skein_http::client::Body::Length(27));
}
fn setup_requests() -> [(Request, Option<Kind>, skein_http::Method, &'static [u8]); 11] {
    use skein_http::Method::{Delete, Get, Patch, Post};
    [
        (
            Request::Installations { page: 2 },
            Some(Kind::InstallationRows),
            Get,
            b"/app/installations?per_page=100&page=2",
        ),
        (Request::Installation { id: 8 }, Some(Kind::Installation), Get, b"/app/installations/8"),
        (
            Request::Mint { installation: 8, scope: Scope::Full },
            Some(Kind::Minted),
            Post,
            b"/app/installations/8/access_tokens",
        ),
        (reachable(), Some(Kind::ReachableRows), Get, b"/installation/repositories?per_page=4&page=1"),
        (Request::Viewer, Some(Kind::Viewer), Post, b"/graphql"),
        (Request::App, Some(Kind::App), Get, b"/app"),
        (Request::HookConfig, Some(Kind::HookConfig), Get, b"/app/hook/config"),
        (
            Request::SetHook { url: b"https://kit.test/".as_slice().into() },
            Some(Kind::HookConfig),
            Patch,
            b"/app/hook/config",
        ),
        (Request::Uninstall { id: 8 }, None, Delete, b"/app/installations/8"),
        (conversion(), Some(Kind::Conversion), Post, b"/app-manifests/code/conversions"),
        (Request::BotUser { slug: b"kit".as_slice().into() }, Some(Kind::BotUser), Get, b"/users/kit%5Bbot%5D"),
    ]
}
#[test]
fn the_method_and_target_are_the_dialects() {
    for (request, kind, method, target) in setup_requests() {
        let auth = match dialect::request::encode(&request, &crate::host::github(&host::limits())).unwrap().auth {
            dialect::request::Auth::App => app(),
            dialect::request::Auth::Installation => bearer(),
            dialect::request::Auth::None => Auth::None,
        };
        let conn = connection(&request, &auth);
        let call = conn.call.as_ref().unwrap();
        assert_eq!(call.method, method, "{request:?}");
        assert_eq!(call.target.as_ref(), target, "{request:?}");
        assert_eq!(kind_of(&request), Ok(kind), "{request:?}");
    }
}
#[test]
fn an_auth_the_request_does_not_take_is_unexpected_and_sends_nothing() {
    let app_auth = app();
    let token = Auth::Token(fixtures::secret(TOKEN));
    for (request, auth) in [
        (Request::App, token.clone()),
        (reachable(), token.clone()),
        (conversion(), token),
        (reachable(), app_auth.clone()),
        (conversion(), app_auth),
        (Request::App, bearer()),
        (conversion(), bearer()),
        (Request::App, Auth::None),
        (reachable(), Auth::None),
    ] {
        assert_eq!(sent(&request, &auth, WALL, &host::limits()).err(), Some(Refused::Unexpected));
    }
}

#[test]
fn a_key_that_cannot_sign_is_unsigned_and_sends_nothing() {
    assert_eq!(
        sent(&Request::App, &Auth::App { id: 7, key: key() }, WALL, &host::limits()).err(),
        Some(Refused::Unsigned)
    );
}
#[test]
fn a_request_the_setup_never_sends_is_unexpected() {
    for request in other_requests() {
        assert_eq!(kind_of(&request), Err(Refused::Unexpected), "{request:?}");
        assert_eq!(sent(&request, &app(), WALL, &host::limits()).err(), Some(Refused::Unexpected), "{request:?}");
    }
}
#[test]
fn a_target_past_target_bytes_is_unexpected() {
    let target_len = u32::try_from(b"/app-manifests/code/conversions".len()).unwrap();
    let mut l = host::limits();
    for (limit, expected) in [(target_len, None), (target_len - 1, Some(Refused::Unexpected))] {
        l.target_bytes = limit;
        assert_eq!(sent(&conversion(), &Auth::None, WALL, &l).err(), expected);
    }
}
#[test]
fn a_non_2xx_answer_is_its_status() {
    for (head, status) in [
        (b"HTTP/1.1 404 Not Found\r\nContent-Length: 99\r\n\r\n".as_slice(), 404),
        (b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 99\r\n\r\n", 401),
    ] {
        assert_eq!(answering(connection(&reachable(), &bearer()), head, b"unread").result, Some(Reply::Status(status)));
    }
}
#[test]
fn an_uninstall_answered_2xx_is_empty() {
    assert_eq!(
        answering(connection(&Request::Uninstall { id: 8 }, &app()), b"HTTP/1.1 204 No Content\r\n\r\n", b"").result,
        Some(Reply::Empty)
    );
}
#[test]
fn a_document_answer_is_a_github_document() {
    for (request, body, document) in [
        (
            Request::Mint { installation: 8, scope: Scope::Full },
            br#"{"token":"new-installation-secret","expires_at":"1970-01-01T00:00:01Z"}"#.as_slice(),
            Document::Minted(dialect::document::Minted {
                permissions: skein_forge::github::document::Levels::NONE,
                token: fixtures::secret(MINTED),
                expires: Wall::from_nanos(Duration::from_secs(1).as_nanos()),
            }),
        ),
        (
            Request::App,
            br#"{"id":7,"slug":"kit"}"#,
            Document::App(dialect::document::App {
                id: 7,
                slug: b"kit".as_slice().into(),
                permissions: Box::new([]),
                events: Box::new([]),
            }),
        ),
    ] {
        assert_eq!(answer(&request, &app(), body), Reply::GithubDocument(document));
    }
}
#[test]
fn a_listing_answer_is_github_rows() {
    for (body, expected) in [
        (br#"{"repositories":[{"id":8,"owner":{"id":2,"login":"ada"},"name":"one","private":true},{"id":9,"owner":{"id":3,"login":"bea"},"name":"two","private":false}]}"#.as_slice(),
            Box::<[Row]>::from([
                Row::Reachable(dialect::document::Reachable { id: 8, owner: 2, login: b"ada".as_slice().into(), name: b"one".as_slice().into(), private: true }),
                Row::Reachable(dialect::document::Reachable { id: 9, owner: 3, login: b"bea".as_slice().into(), name: b"two".as_slice().into(), private: false }),
            ])),
        (br#"{"repositories":[]}"#, Box::new([])),
    ] {
        assert_eq!(answer(&reachable(), &bearer(), body), Reply::GithubRows(expected));
    }
}
#[test]
fn a_malformed_or_cut_answer_is_down() {
    for body in [b"{".as_slice(), b"not json", br#"{"repositories":["#, br#"{"repositories":{}}"#] {
        assert_eq!(answer(&reachable(), &bearer(), body), Reply::Down);
    }
}
#[test]
fn a_forgejo_connection_is_unchanged() {
    use skein_forge::forgejo::{
        document::Row as FRow,
        request::{Page, Request as FRequest},
    };
    let forge = Forge {
        kind: ForgeKind::Forgejo(fixtures::given()),
        address: skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()),
        ..forge()
    };
    let conn = Connection::new(
        Token::new(1),
        &forge,
        AGENT,
        &FRequest::RepoSearch { page: Page { page: Some(1), limit: Some(4) } }.into(),
        &fixtures::secret(TOKEN),
        &host::limits(),
    )
    .unwrap();
    let call = conn.call.as_ref().unwrap();
    assert_eq!(call.method, skein_http::Method::Get);
    assert_eq!(call.target.as_ref(), b"/api/v1/repos/search?sort=id&order=asc&page=1&limit=4");
    assert_eq!(header(&conn, b"Host").as_deref(), Some(b"127.0.0.1:3000".as_slice()));
    assert!(header(&conn, b"Authorization").as_deref() == Some(b"token installation-secret-token".as_slice()));
    assert_eq!(header(&conn, b"User-Agent").as_deref(), Some(AGENT));
    assert_eq!(call.headers.len(), 3);
    assert_eq!(call.body, skein_http::client::Body::None);
    assert_eq!(conn.upload, None);
    assert_eq!(
        answering(conn, b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n", br#"{"ok":true,"data":[]}"#).result,
        Some(Reply::Rows(Box::<[FRow]>::from([])))
    );
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the test inspects Debug for secret bytes")]
fn no_secret_is_in_a_connections_debug() {
    let conn = connection(&Request::App, &app());
    let jwt = authorization(&conn);
    let debug = format!("{conn:?}");
    hidden(&debug, &jwt);
    hidden(&debug, skein_fake_forge_github::keys::KEYS[0]);
    let conn = answering(conn, b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n", br#"{"id":7,"slug":"kit"}"#);
    hidden(&format!("{conn:?}"), &jwt);
    let minted = answering(
        connection(&Request::Mint { installation: 8, scope: Scope::Full }, &app()),
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
        br#"{"token":"new-installation-secret","expires_at":"1970-01-01T00:00:01Z"}"#,
    );
    hidden(&format!("{minted:?} {:?}", minted.result), MINTED);
}
#[expect(clippy::disallowed_macros, reason = "the test checks the decimal byte Debug rendering for redaction")]
fn hidden(debug: &str, secret: &[u8]) {
    assert!(bytes::find(debug.as_bytes(), secret).is_none());
    let mut hex = skein_lib::Writer::new(secret.len().checked_mul(2).unwrap());
    let digits = b"0123456789abcdef";
    for byte in secret {
        hex.put(&[*digits.get(usize::from(byte >> 4_u32)).unwrap(), *digits.get(usize::from(byte & 15)).unwrap()])
            .unwrap();
    }
    assert!(bytes::find(debug.as_bytes(), &hex.finish()).is_none());
    let list = format!("{secret:?}");
    assert!(!debug.contains(&list));
}

fn other_requests() -> [Request; 36] {
    let owner: Box<[u8]> = b"ada".as_slice().into();
    let name: Box<[u8]> = b"repo".as_slice().into();
    let branch: Box<[u8]> = b"main".as_slice().into();
    let commit = [1; 32];
    [
        Request::Repository { id: 8 },
        Request::Locate { owner: owner.clone(), name: name.clone() },
        Request::Issues { owner: owner.clone(), name: name.clone(), since: Wall::EPOCH, page: 1, per_page: 4 },
        Request::Issue { owner: owner.clone(), name: name.clone(), number: 1 },
        Request::Comments { owner: owner.clone(), name: name.clone(), number: 1, page: 1, per_page: 4 },
        Request::Pull { owner: owner.clone(), name: name.clone(), number: 1 },
        Request::PullsFor {
            owner: owner.clone(),
            name: name.clone(),
            head: branch.clone(),
            base: branch.clone(),
            open: true,
        },
        Request::Reviews { owner: owner.clone(), name: name.clone(), number: 1, page: 1, per_page: 4 },
        Request::ReviewComments {
            owner: owner.clone(),
            name: name.clone(),
            number: 1,
            review: 2,
            page: 1,
            per_page: 4,
        },
        Request::CombinedStatus { owner: owner.clone(), name: name.clone(), commit, page: 1, per_page: 4 },
        Request::CheckRuns { owner: owner.clone(), name: name.clone(), commit, page: 1, per_page: 4 },
        Request::Job { owner: owner.clone(), name: name.clone(), id: 1 },
        Request::JobLog { owner: owner.clone(), name: name.clone(), id: 1 },
        Request::Branch { owner: owner.clone(), name: name.clone(), branch: branch.clone() },
        Request::Branches { owner: owner.clone(), name: name.clone(), page: 1, per_page: 4 },
        Request::PullFiles { owner: owner.clone(), name: name.clone(), number: 1, page: 1, per_page: 4 },
        Request::Compare { owner: owner.clone(), name: name.clone(), before: commit, after: commit, per_page: 4 },
        Request::Raw { owner: owner.clone(), name: name.clone(), path: branch.clone(), commit },
        Request::RulesForBranch { owner: owner.clone(), name: name.clone(), branch: branch.clone(), page: 1 },
        Request::Ruleset { owner: owner.clone(), name: name.clone(), id: 1 },
        Request::ClassicProtection { owner: owner.clone(), name: name.clone(), branch: branch.clone() },
        Request::Collaborators { owner: owner.clone(), name: name.clone(), page: 1, per_page: 4 },
        Request::Permission { owner: owner.clone(), name: name.clone(), login: owner.clone() },
        Request::User { id: 2 },
        Request::PullGraphql { owner: owner.clone(), name: name.clone(), number: 1 },
        Request::DeleteBranch { owner: owner.clone(), name: name.clone(), branch: branch.clone() },
        Request::Reopen { owner: owner.clone(), name: name.clone(), number: 1 },
        Request::Close { owner: owner.clone(), name: name.clone(), number: 1 },
        Request::Edit { owner: owner.clone(), name: name.clone(), number: 1, title: None, body: None },
        Request::Status {
            owner: owner.clone(),
            name: name.clone(),
            commit,
            context: branch.clone(),
            description: Box::new([]),
            link: Box::new([]),
            check: skein_forge::api::Check::Passed,
        },
        Request::CreateBranch { owner: owner.clone(), name: name.clone(), branch: branch.clone(), commit },
        Request::Update { owner: owner.clone(), name: name.clone(), number: 1, head: commit },
        Request::Review {
            owner: owner.clone(),
            name: name.clone(),
            number: 1,
            verdict: skein_forge::api::Verdict::Approve,
            body: Box::new([]),
        },
        Request::Post { owner: owner.clone(), name: name.clone(), number: 1, body: Box::new([]) },
        Request::CreateIssue { owner: owner.clone(), name: name.clone(), title: branch.clone(), body: Box::new([]) },
        Request::OpenPull {
            owner,
            name,
            title: branch.clone(),
            body: Box::new([]),
            head: branch.clone(),
            base: branch,
        },
    ]
}

#[test]
fn a_github_request_head_must_fit_the_hosts_output() {
    fn settings() -> crate::host::Settings {
        let mut forges = skein_lib::List::with_capacity(1);
        forges.push(forge()).unwrap();
        crate::host::Settings { forges, user_agent: AGENT.into() }
    }
    fn crypto() -> skein_lib::CryptoRng {
        skein_lib::CryptoRng::new(&fixtures::secret(&[42; 32]), 0).unwrap()
    }
    const HOST_BYTES: u32 = 14; // 127.0.0.1:3001, from forge().
    const AGENT_BYTES: u32 = 9; // kit/0.0.0, from AGENT.
    const FRAMING: u32 = 320; // GitHub's request-head overhead in host::limits.
    let mut l = host::limits();
    let bound = l.target_bytes + HOST_BYTES + u32::try_from(dialect::jwt::JWT_BYTES).unwrap() + AGENT_BYTES + FRAMING;
    for (output, expected) in [(bound, None), (bound - 1, Some(crate::host::Unusable::Request { forge: 4 }))] {
        l.io.output = output;
        l.tls.send = 1;
        assert_eq!(crate::host::Host::new(&l, settings(), crypto()).err(), expected);
    }
}
