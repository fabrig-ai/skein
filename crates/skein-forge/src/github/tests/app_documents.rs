use super::fixtures::{DOCUMENT_BYTES, bytes, decode, limits, wall};
use crate::github::{
    decode::{self, Malformed, Step},
    document::{self, Account, Document, Kind, Row},
    request::{self, Accept, Auth, Method, Refusal, Request, Scope},
    server,
};
use skein_json::{Token, tokenizer::Event};
use skein_lib::{Secret, Writer};

const INSTALLATION: &[u8] = br#"{"id":18446744073709551615,"account":{"login":"o","id":9007199254740993,"type":"Organization","ignored":[{"a":false}]},"created_at":"2026-10-20T08:00:00Z","suspended_at":null,"permissions":{"contents":"write"}}"#;
const MINTED: &[u8] =
    br#"{"token":"fixture-mint-sentinel","expires_at":"2026-10-20T08:00:00Z","permissions":{"contents":"write"}}"#;
const CONVERSION: &[u8] = br#"{"id":18446744073709551615,"slug":"probe","owner":{"login":"o","id":9007199254740993,"type":"Organization"},"pem":"fixture-pem-sentinel","webhook_secret":"fixture-hook-sentinel","client_id":"ignored","client_secret":{"ignored":[true,null]}}"#;

struct RequestCase {
    request: Request,
    target: &'static [u8],
    method: Method,
    auth: Auth,
    body: Option<&'static [u8]>,
}
fn requests() -> [RequestCase; 14] {
    [
        RequestCase {
            request: Request::Installations { page: 2 },
            target: b"/app/installations?per_page=100&page=2",
            method: Method::Get,
            auth: Auth::App,
            body: None,
        },
        RequestCase {
            request: Request::Installation { id: u64::MAX },
            target: b"/app/installations/18446744073709551615",
            method: Method::Get,
            auth: Auth::App,
            body: None,
        },
        RequestCase {
            request: Request::Mint { installation: 3, scope: Scope::Full },
            target: b"/app/installations/3/access_tokens",
            method: Method::Post,
            auth: Auth::App,
            body: None,
        },
        RequestCase {
            request: Request::Mint { installation: 3, scope: Scope::Contents },
            target: b"/app/installations/3/access_tokens",
            method: Method::Post,
            auth: Auth::App,
            body: Some(br#"{"permissions":{"contents":"write"}}"#),
        },
        RequestCase {
            request: Request::Reachable { page: 2, per_page: 20 },
            target: b"/installation/repositories?per_page=20&page=2",
            method: Method::Get,
            auth: Auth::Installation,
            body: None,
        },
        RequestCase {
            request: Request::Viewer,
            target: b"/graphql",
            method: Method::Post,
            auth: Auth::Installation,
            body: Some(br#"{"query":"query { viewer { login databaseId } }"}"#),
        },
        RequestCase { request: Request::App, target: b"/app", method: Method::Get, auth: Auth::App, body: None },
        RequestCase {
            request: Request::HookConfig,
            target: b"/app/hook/config",
            method: Method::Get,
            auth: Auth::App,
            body: None,
        },
        RequestCase {
            request: Request::SetHook { url: Box::from(*b"https://example.invalid/hook?q=\"x\"\n") },
            target: b"/app/hook/config",
            method: Method::Patch,
            auth: Auth::App,
            body: Some(br#"{"url":"https://example.invalid/hook?q=\"x\"\n"}"#),
        },
        RequestCase {
            request: Request::Uninstall { id: 3 },
            target: b"/app/installations/3",
            method: Method::Delete,
            auth: Auth::App,
            body: None,
        },
        RequestCase {
            request: Request::Conversion { code: Box::from(*b"a/b ?%") },
            target: b"/app-manifests/a%2Fb%20%3F%25/conversions",
            method: Method::Post,
            auth: Auth::None,
            body: None,
        },
        RequestCase {
            request: Request::BotUser { slug: Box::from(*b"probe-app") },
            target: b"/users/probe-app%5Bbot%5D",
            method: Method::Get,
            auth: Auth::None,
            body: None,
        },
        RequestCase {
            request: Request::BotUser { slug: Box::from("a/b ?é".as_bytes()) },
            target: b"/users/a%2Fb%20%3F%C3%A9%5Bbot%5D",
            method: Method::Get,
            auth: Auth::None,
            body: None,
        },
        RequestCase {
            request: Request::SetHook { url: Box::from([]) },
            target: b"/app/hook/config",
            method: Method::Patch,
            auth: Auth::App,
            body: Some(br#"{"url":""}"#),
        },
    ]
}

#[test]
fn github_app_request_targets() {
    let good = limits();
    assert_eq!(document::VIEWER, b"query { viewer { login databaseId } }");
    for case in requests() {
        let encoded = request::encode(&case.request, &good).unwrap();
        assert_eq!(encoded.target.as_ref(), case.target);
        assert_eq!(encoded.method, case.method);
        assert_eq!(encoded.auth, case.auth);
        assert_eq!(encoded.accept, Accept::Json);
        assert_eq!(encoded.body.as_deref(), case.body);
        let mut bound = good;
        bound.target_bytes = u32::try_from(case.target.len()).unwrap();
        assert_eq!(request::encode(&case.request, &bound), Ok(encoded.clone()));
        bound.target_bytes = bound.target_bytes.saturating_sub(1);
        assert_eq!(request::encode(&case.request, &bound), Err(Refusal::TooLong));
        if let Some(body) = case.body {
            let mut bound = good;
            bound.body_bytes = u32::try_from(body.len()).unwrap();
            assert_eq!(request::encode(&case.request, &bound), Ok(encoded));
            bound.body_bytes = bound.body_bytes.saturating_sub(1);
            assert_eq!(request::encode(&case.request, &bound), Err(Refusal::TooLong));
        }
    }
    for request in [
        Request::BotUser { slug: Box::from([0xff]) },
        Request::Conversion { code: Box::from([0xff]) },
        Request::SetHook { url: Box::from([0xff]) },
    ] {
        assert_eq!(request::encode(&request, &good), Err(Refusal::Text));
    }
    let long = Request::SetHook { url: Box::from([b'x'; 129]) };
    assert_eq!(request::encode(&long, &good), Err(Refusal::TooLong));
}

fn installation() -> document::Installation {
    document::Installation {
        id: u64::MAX,
        login: Box::from(*b"o"),
        account: 9_007_199_254_740_993,
        kind: Account::Organization,
        created: wall(),
        suspended: false,
    }
}
fn reachable() -> document::Reachable {
    document::Reachable {
        id: u64::MAX,
        owner: 9_007_199_254_740_993,
        login: Box::from(*b"o"),
        name: Box::from(*b"n"),
        private: true,
    }
}
fn minted() -> document::Minted {
    document::Minted {
        permissions: document::Levels { contents: document::Level::Write, pull_requests: document::Level::None },
        token: Secret::new(Box::from(*b"fixture-mint-sentinel")),
        expires: wall(),
    }
}
fn conversion() -> document::Conversion {
    document::Conversion {
        id: u64::MAX,
        slug: Box::from(*b"probe"),
        owner: Box::from(*b"o"),
        owner_id: 9_007_199_254_740_993,
        pem: Secret::new(Box::from(*b"fixture-pem-sentinel")),
        webhook_secret: Secret::new(Box::from(*b"fixture-hook-sentinel")),
    }
}
fn viewer() -> document::Viewer {
    document::Viewer { id: 9_007_199_254_740_993, login: Box::from(*b"probe[bot]") }
}
fn graphql() -> document::GraphqlRefusal {
    document::GraphqlRefusal { kind: Box::from(*b"FORBIDDEN"), message: Box::from(*b"refused") }
}
struct DocumentCase {
    kind: Kind,
    body: &'static [u8],
    expected: Document,
    rows: Box<[Row]>,
}
fn documents() -> [DocumentCase; 13] {
    [
        DocumentCase { kind: Kind::Installation, body: INSTALLATION, expected: Document::Installation(installation()), rows: Box::from([]) },
        DocumentCase { kind: Kind::InstallationRows, body: br#"[{"id":18446744073709551615,"account":{"login":"o","id":9007199254740993,"type":"Organization"},"created_at":"2026-10-20T08:00:00Z"},{"id":2,"account":{"login":"u","id":3,"type":"User"},"created_at":"2026-10-20T08:00:00Z","suspended_at":"2026-10-20T09:00:00Z"},{"id":4,"account":{"login":"v","id":5,"type":"User"},"created_at":"2026-10-20T08:00:00Z","suspended_at":null}]"#, expected: Document::Rows, rows: Box::from([Row::Installation(installation()), Row::Installation(document::Installation { id: 2, login: Box::from(*b"u"), account: 3, kind: Account::User, created: wall(), suspended: true }), Row::Installation(document::Installation { id: 4, login: Box::from(*b"v"), account: 5, kind: Account::User, created: wall(), suspended: false })]) },
        DocumentCase { kind: Kind::Minted, body: MINTED, expected: Document::Minted(minted()), rows: Box::from([]) },
        DocumentCase { kind: Kind::ReachableRows, body: br#"{"total_count":10,"repository_selection":"selected","repositories":[{"id":18446744073709551615,"owner":{"id":9007199254740993,"login":"o","type":"Organization"},"name":"n","private":true,"ignored":[{"a":[false]}]},{"id":2,"owner":{"id":3,"login":"u"},"name":"public","private":false}]}"#, expected: Document::Rows, rows: Box::from([Row::Reachable(reachable()), Row::Reachable(document::Reachable { id: 2, owner: 3, login: Box::from(*b"u"), name: Box::from(*b"public"), private: false })]) },
        DocumentCase { kind: Kind::Viewer, body: br#"{"data":{"viewer":{"login":"probe[bot]","databaseId":9007199254740993,"id":"ignored"}},"ignored":[null]}"#, expected: Document::Viewer(viewer()), rows: Box::from([]) },
        DocumentCase { kind: Kind::Viewer, body: br#"{"data":null,"errors":[{"type":"FORBIDDEN","message":"refused","path":["viewer"]},{"ignored":[{"nested":false}]}]}"#, expected: Document::Graphql(graphql()), rows: Box::from([]) },
        DocumentCase { kind: Kind::App, body: br#"{"id":18446744073709551615,"slug":"probe","permissions":{"contents":"write"},"events":["push"]}"#, expected: Document::App(document::App { id: u64::MAX, slug: Box::from(*b"probe"), permissions: Box::from([document::Permission { name: Box::from(*b"contents"), access: document::Access::Write }]), events: Box::from([Box::<[u8]>::from(*b"push")]) }), rows: Box::from([]) },
        DocumentCase { kind: Kind::HookConfig, body: br#"{"url":"https://example.invalid/hook","content_type":"json","insecure_ssl":"0"}"#, expected: Document::HookConfig(document::Hook { url: Box::from(*b"https://example.invalid/hook") }), rows: Box::from([]) },
        DocumentCase { kind: Kind::Conversion, body: CONVERSION, expected: Document::Conversion(conversion()), rows: Box::from([]) },
        DocumentCase { kind: Kind::BotUser, body: br#"{"id":18446744073709551615,"login":"probe[bot]"}"#, expected: Document::BotUser(u64::MAX), rows: Box::from([]) },
        DocumentCase { kind: Kind::InstallationRows, body: b"[]", expected: Document::Rows, rows: Box::from([]) },
        DocumentCase { kind: Kind::ReachableRows, body: br#"{"total_count":0,"repositories":[]}"#, expected: Document::Rows, rows: Box::from([]) },
        DocumentCase { kind: Kind::Viewer, body: br#"{"errors":[{"type":"FORBIDDEN","message":"refused"}],"data":{"viewer":{"databaseId":1,"login":"ignored"}}}"#, expected: Document::Graphql(graphql()), rows: Box::from([]) },
    ]
}

#[test]
fn github_app_documents_decode() {
    let good = limits();
    for case in documents() {
        for chunk in [1, 7, u32::try_from(case.body.len()).unwrap()] {
            let (rows, result) = decode(case.kind, case.body, chunk, &good);
            assert_eq!(result, Ok(case.expected.clone()));
            assert_eq!(rows.as_slice(), case.rows.as_ref());
        }
    }
    for (kind, body, expected) in [
        (Kind::App, br#"{"slug":"probe"}"#.as_slice(), Malformed::Missing { field: b"id" }),
        (Kind::App, br#"{"id":null,"slug":"probe"}"#, Malformed::Missing { field: b"id" }),
        (Kind::BotUser, br#"{"id":18446744073709551616}"#, Malformed::Range { field: b"id" }),
        (
            Kind::Viewer,
            br#"{"data":{"viewer":{"databaseId":18446744073709551616,"login":"bot"}}}"#,
            Malformed::Range { field: b"databaseId" },
        ),
        (Kind::Minted, br#"{"token":"x","expires_at":"no-time"}"#, Malformed::Time { field: b"expires_at" }),
        (
            Kind::Minted,
            br#"{"token":null,"expires_at":"2026-10-20T08:00:00Z"}"#,
            Malformed::Missing { field: b"token" },
        ),
        (
            Kind::Conversion,
            br#"{"id":1,"slug":"p","owner":{"id":2,"login":"o"},"pem":"x"}"#,
            Malformed::Missing { field: b"webhook_secret" },
        ),
        (
            Kind::Installation,
            br#"{"id":1,"account":{"id":2,"login":"o","type":"Bot"},"created_at":"2026-10-20T08:00:00Z"}"#,
            Malformed::Value { field: b"type" },
        ),
        (
            Kind::Installation,
            br#"{"id":1,"account":{"id":2,"login":"o","type":false},"created_at":"2026-10-20T08:00:00Z"}"#,
            Malformed::Shape { field: b"type" },
        ),
        (
            Kind::Installation,
            br#"{"id":1,"account":{"id":2,"login":"o","type":"User"}}"#,
            Malformed::Missing { field: b"created_at" },
        ),
        (
            Kind::ReachableRows,
            br#"{"repositories":[{"id":1,"owner":{"id":2,"login":"o"},"name":"n"}]}"#,
            Malformed::Missing { field: b"private" },
        ),
        (Kind::ReachableRows, br#"{"repositories":null}"#, Malformed::Missing { field: b"repositories" }),
        (Kind::HookConfig, br#"{"url":true}"#, Malformed::Shape { field: b"url" }),
        (Kind::HookConfig, br#"{"url":"a","url":"b"}"#, Malformed::Shape { field: b"url" }),
        (Kind::Viewer, b"{}", Malformed::Missing { field: b"viewer" }),
        (Kind::Viewer, br#"{"data":{"viewer":null}}"#, Malformed::Missing { field: b"viewer" }),
        (Kind::Viewer, br#"{"errors":[]}"#, Malformed::Missing { field: b"type" }),
        (Kind::Viewer, br#"{"errors":[{"type":"FORBIDDEN"}]}"#, Malformed::Missing { field: b"message" }),
    ] {
        for chunk in [1, 7, u32::try_from(body.len()).unwrap()] {
            assert_eq!(decode(kind, body, chunk, &good).1, Err(expected));
        }
    }
    secret_bounds();
}

fn secret_bounds() {
    let good = limits();
    for count in [128, 129] {
        let value: Box<[u8]> = Box::from(&[b'x'; 129][..count]);
        let minted = document::Minted {
            permissions: document::Levels::NONE,
            token: Secret::new(value.clone()),
            expires: wall(),
        };
        let mut conversion = conversion();
        conversion.pem = Secret::new(value.clone());
        conversion.webhook_secret = Secret::new(value);
        for (kind, field) in
            [(Kind::Minted, b"token".as_slice()), (Kind::Conversion, b"pem"), (Kind::Conversion, b"webhook_secret")]
        {
            let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
            let expected = if kind == Kind::Minted {
                server::minted(&minted, &mut out).unwrap();
                Document::Minted(minted.clone())
            } else {
                let mut row = conversion.clone();
                if field == b"pem" {
                    row.webhook_secret = Secret::new(Box::from([]));
                } else {
                    row.pem = Secret::new(Box::from([]));
                }
                server::conversion(&row, &mut out).unwrap();
                Document::Conversion(row)
            };
            let body = bytes(out);
            for chunk in [1, 7, u32::try_from(body.len()).unwrap()] {
                let result = decode(kind, &body, chunk, &good).1;
                if count == 128 {
                    assert_eq!(result, Ok(expected.clone()));
                } else {
                    assert_eq!(result, Err(Malformed::Bound { field }));
                }
            }
        }
    }
}

#[test]
fn github_app_secrets_hidden() {
    for case in documents() {
        if case.kind != Kind::Minted && case.kind != Kind::Conversion {
            continue;
        }
        let document = decode(case.kind, case.body, 7, &limits()).1.unwrap();
        let debug = alloc::fmt::format(format_args!("{document:?}"));
        assert_redacted(&debug);
        assert_redacted(&alloc::fmt::format(format_args!("{:?}", Step::Done(document.clone()))));
        match document {
            Document::Minted(row) => assert_redacted(&alloc::fmt::format(format_args!("{row:?}"))),
            Document::Conversion(row) => assert_redacted(&alloc::fmt::format(format_args!("{row:?}"))),
            Document::Delivery(_)
            | Document::Installation(_)
            | Document::Viewer(_)
            | Document::Merge(_)
            | Document::Enqueued(_)
            | Document::PullGraphql(_)
            | Document::Graphql(_)
            | Document::App(_)
            | Document::HookConfig(_)
            | Document::User(_)
            | Document::Compare(_)
            | Document::Permission(_)
            | Document::Id(_)
            | Document::Number(_)
            | Document::Done
            | Document::Ruleset(_)
            | Document::ClassicProtection(_)
            | Document::BotUser(_)
            | Document::Repository(_)
            | Document::Branch(_)
            | Document::ActionsPermissions(_)
            | Document::Workflows { .. }
            | Document::Rows
            | Document::Pull(_)
            | Document::Job(_)
            | Document::Combined(_)
            | Document::Issue(_)
            | Document::Error(_) => panic!("secret document fixture"),
        }
    }
    for (kind, fields) in [
        (Kind::Minted, [(b"token".as_slice(), b"fixture-mint-sentinel".as_slice()), (b"ignored", b"ignored")]),
        (
            Kind::Conversion,
            [(b"pem".as_slice(), b"fixture-pem-sentinel".as_slice()), (b"webhook_secret", b"fixture-hook-sentinel")],
        ),
    ] {
        let mut decoder = decode::decoder(kind, &limits());
        assert_eq!(decoder.step(Event::Token(Token::ObjectStart)).1, Step::Demand);
        for (key, text) in fields {
            assert_eq!(decoder.step(Event::Token(Token::Key(Box::from(key)))).1, Step::Demand);
            let step = decoder.step(Event::Token(Token::String(Box::from(text)))).1;
            assert_eq!(step, Step::Demand);
            assert_redacted(&alloc::fmt::format(format_args!("{decoder:?}")));
            assert_redacted(&alloc::fmt::format(format_args!("{step:?}")));
        }
    }
}
fn assert_redacted(debug: &str) {
    for text in ["fixture-mint-sentinel", "fixture-pem-sentinel", "fixture-hook-sentinel"] {
        assert!(!debug.contains(text), "secret text is redacted");
        let bytes = alloc::fmt::format(format_args!("{:?}", text.as_bytes()));
        assert!(!debug.contains(&bytes), "secret bytes are redacted");
    }
}

#[test]
fn github_app_server_round_trip() {
    let good = limits();
    for case in requests() {
        let encoded = request::encode(&case.request, &good).unwrap();
        let body = encoded.body.as_deref().unwrap_or(b"");
        let routed = if case.request == Request::Viewer {
            assert_eq!(server::route(encoded.method, &encoded.target, &good), None);
            server::graphql(body).unwrap()
        } else {
            let mut routed = server::route(encoded.method, &encoded.target, &good).unwrap();
            match &mut routed {
                Request::Mint { scope, .. } => *scope = server::mint_scope(body).unwrap(),
                Request::SetHook { url } => *url = server::hook_url(body, &good).unwrap(),
                Request::LogPage { .. }
                | Request::LogDelivery { .. }
                | Request::Installations { .. }
                | Request::Installation { .. }
                | Request::Reachable { .. }
                | Request::PullGraphql { .. }
                | Request::Viewer
                | Request::App
                | Request::HookConfig
                | Request::Uninstall { .. }
                | Request::Conversion { .. }
                | Request::BotUser { .. }
                | Request::Repository { .. }
                | Request::ActionsPermissions { .. }
                | Request::Workflows { .. }
                | Request::Locate { .. }
                | Request::Issues { .. }
                | Request::Issue { .. }
                | Request::Comments { .. }
                | Request::Pull { .. }
                | Request::PullsFor { .. }
                | Request::Reviews { .. }
                | Request::ReviewComments { .. }
                | Request::CombinedStatus { .. }
                | Request::CheckRuns { .. }
                | Request::Job { .. }
                | Request::JobLog { .. }
                | Request::Followed { .. }
                | Request::Branch { .. }
                | Request::Branches { .. }
                | Request::PullFiles { .. }
                | Request::Compare { .. }
                | Request::Raw { .. }
                | Request::RulesForBranch { .. }
                | Request::Ruleset { .. }
                | Request::ClassicProtection { .. }
                | Request::Collaborators { .. }
                | Request::Permission { .. }
                | Request::Reopen { .. }
                | Request::Edit { .. }
                | Request::Status { .. }
                | Request::CreateBranch { .. }
                | Request::Reviewers { .. }
                | Request::Update { .. }
                | Request::Review { .. }
                | Request::Post { .. }
                | Request::CreateIssue { .. }
                | Request::OpenPull { .. }
                | Request::Close { .. }
                | Request::DeleteBranch { .. }
                | Request::Merge { .. }
                | Request::Enqueue { .. }
                | Request::User { .. } => {}
            }
            routed
        };
        assert_eq!(routed, case.request);
        for method in [Method::Get, Method::Post, Method::Put, Method::Patch, Method::Delete] {
            if method != encoded.method
                && let Some(routed) = server::route(method, &encoded.target, &good)
            {
                assert_ne!(routed, case.request, "a different method cannot select the same request");
            }
        }
    }
    for case in documents() {
        let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
        write_answer(&case, &mut out).unwrap();
        let body = bytes(out);
        let (rows, document) = decode(case.kind, &body, 7, &good);
        assert_eq!(document, Ok(case.expected.clone()));
        assert_eq!(rows.as_slice(), case.rows.as_ref());
        let mut small = Writer::new(body.len().saturating_sub(1));
        assert_eq!(write_answer(&case, &mut small), Err(skein_lib::Overflow));
        assert_eq!(small.written(), 0);
    }
    server_refusals();
}
fn write_answer(case: &DocumentCase, out: &mut Writer) -> Result<(), skein_lib::Overflow> {
    match &case.expected {
        Document::Installation(row) => server::installation(row, out),
        Document::Minted(row) => server::minted(row, out),
        Document::Viewer(row) => server::viewer(row, out),
        Document::PullGraphql(row) => server::pull_graphql(row, out),
        Document::Graphql(row) => server::graphql_refusal(&row.kind, &row.message, out),
        Document::App(row) => server::app(row, out),
        Document::HookConfig(row) => server::hook(row, out),
        Document::Conversion(row) => server::conversion(row, out),
        Document::User(row) => server::user(row, out),
        Document::Compare(_) => panic!("App fixture"),
        Document::Permission(value) => server::permission(*value, out),
        Document::Id(id) => server::id(*id, out),
        Document::Number(number) => server::number(*number, out),
        Document::Done => server::done(out),
        Document::BotUser(id) => server::bot_user(*id, b"example-app", out),
        Document::Rows => {
            let mut installations = skein_lib::List::with_capacity(3);
            let mut reachable = skein_lib::List::with_capacity(2);
            for row in &case.rows {
                match row {
                    Row::Installation(row) => installations.push(row.clone()).unwrap(),
                    Row::Reachable(row) => reachable.push(row.clone()).unwrap(),
                    Row::Log(_)
                    | Row::Pull(_)
                    | Row::Context(_)
                    | Row::Run(_)
                    | Row::Issue(_)
                    | Row::Comment(_)
                    | Row::Commit(_)
                    | Row::File(_)
                    | Row::Review(_)
                    | Row::Remark(_)
                    | Row::Collaborator(_)
                    | Row::Rule(_)
                    | Row::Branch(_)
                    | Row::SkippedBranch => {
                        panic!("App row fixture")
                    }
                }
            }
            if case.kind == Kind::InstallationRows {
                server::installations(installations.as_slice(), out)
            } else {
                server::reachable(reachable.as_slice(), 10, out)
            }
        }
        Document::Delivery(_)
        | Document::Merge(_)
        | Document::Enqueued(_)
        | Document::Pull(_)
        | Document::Job(_)
        | Document::Combined(_)
        | Document::Repository(_)
        | Document::Branch(_)
        | Document::Issue(_)
        | Document::Ruleset(_)
        | Document::ClassicProtection(_)
        | Document::ActionsPermissions(_)
        | Document::Workflows { .. }
        | Document::Error(_) => {
            panic!("App document fixture")
        }
    }
}
fn server_refusals() {
    let good = limits();
    for body in
        [b"{}".as_slice(), br#"{"permissions":{"contents":"read"}}"#, b" ", br#"{"permissions":{"contents":"write"}} "#]
    {
        assert_eq!(server::mint_scope(body), None);
    }
    for body in [
        b"".as_slice(),
        b"{}",
        br#"{"query":"other"}"#,
        br#"{"query":"query { viewer { login databaseId } }","variables":{}}"#,
    ] {
        assert_eq!(server::graphql(body), None);
    }
    for body in [
        b"{}".as_slice(),
        br#"{"url":null}"#,
        br#"{"url":1}"#,
        br#"{"url":"a","url":"b"}"#,
        br#"{"url":"unterminated}"#,
        br#"{"url":"a"}{}"#,
    ] {
        assert_eq!(server::hook_url(body, &good), None);
    }
    let body = br#" {"ignored":[false],"url":"a\"b"} "#;
    assert_eq!(server::hook_url(body, &good), Some(Box::from(*b"a\"b")));
    let mut bound = good;
    bound.body_bytes = u32::try_from(body.len()).unwrap();
    assert_eq!(server::hook_url(body, &bound), Some(Box::from(*b"a\"b")));
    bound.body_bytes = bound.body_bytes.saturating_sub(1);
    assert_eq!(server::hook_url(body, &bound), None);
    let mut bound = good;
    bound.string_bytes = 2;
    assert_eq!(server::hook_url(body, &bound), None);
    for target in [
        b"/app/installations?per_page=20&page=1".as_slice(),
        b"/app/installations?per_page=100&page=1&extra=1",
        b"/installation/repositories?per_page=100&page=1#fragment",
        b"/app/installations/18446744073709551616",
        b"/users/%FF%5Bbot%5D",
        b"/users/a/b%5Bbot%5D",
    ] {
        assert_eq!(server::route(Method::Get, target, &good), None);
    }
}

#[test]
fn mint_permissions_keep_each_level_and_default_to_none() {
    use document::{Level, Levels};
    for (suffix, permissions) in [
        (b"".as_slice(), Levels::NONE),
        (br#", "permissions":{}"#, Levels::NONE),
        (
            br#", "permissions":{"contents":"read","pull_requests":"write"}"#,
            Levels { contents: Level::Read, pull_requests: Level::Write },
        ),
        (
            br#", "permissions":{"contents":"write","pull_requests":"read"}"#,
            Levels { contents: Level::Write, pull_requests: Level::Read },
        ),
        (br#", "permissions":{"contents":"admin","pull_requests":"future"}"#, Levels::NONE),
        (br#", "permissions":{"contents":"none","pull_requests":""}"#, Levels::NONE),
    ] {
        let mut body = Writer::new(256);
        body.put(br#"{"token":"fixture-mint-sentinel","expires_at":"2026-10-20T08:00:00Z""#).unwrap();
        body.put(suffix).unwrap();
        body.put(b"}").unwrap();
        let body = bytes(body);
        let mut expected = minted();
        expected.permissions = permissions;
        assert_eq!(decode(Kind::Minted, &body, 1, &limits()).1, Ok(Document::Minted(expected)));
    }
}
