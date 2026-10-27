mod app;
mod app_documents;
mod app_facts;
mod checks;
mod delivery;
mod items;
mod job;
mod kinds;
mod log;
mod protection;
mod pulls;
mod pulls_decode;
mod rate_validator;
mod reads;
mod server_log;
mod server_protection;
use super::{
    credential,
    decode::Malformed,
    document::{Document, Kind, Row},
    failure::{self, Asked, Drop, Verdict},
    header::{self, Head, RateFields, Resource},
    id,
    limits::{self, Problem},
    request::{self, Accept, Auth, Method, Refusal, Request},
    server,
};
use crate::{api, credential::Credential};
use skein_lib::{Duration, Secret, Wall, Writer};
mod fixtures;
use fixtures::{
    BRANCH, BRANCHES, DOCUMENT_BYTES, INNER, OTHER_SHA, REFUSAL, REPOSITORY, SHA, STRING_BYTES, bytes, decode, limits,
    requests, wall,
};

#[test]
fn github_limits_check() {
    let good = limits();
    let mut cases = skein_lib::List::with_capacity(16);
    for case in [
        ("valid", good, Ok(())),
        ("string", limits::Limits { string_bytes: 0, ..good }, Err(Problem::Zero)),
        ("body", limits::Limits { body_bytes: 0, ..good }, Err(Problem::Zero)),
        ("inner", limits::Limits { inner: 0, ..good }, Err(Problem::Zero)),
        ("target", limits::Limits { target_bytes: 0, ..good }, Err(Problem::Zero)),
        (
            "chunk",
            limits::Limits { json: skein_json::tokenizer::Limits { chunk: 0, ..good.json }, ..good },
            Err(Problem::Zero),
        ),
        (
            "number",
            limits::Limits { json: skein_json::tokenizer::Limits { number: 0, ..good.json }, ..good },
            Err(Problem::Zero),
        ),
        (
            "length",
            limits::Limits { json: skein_json::tokenizer::Limits { length: 0, ..good.json }, ..good },
            Err(Problem::Zero),
        ),
        (
            "writer length",
            limits::Limits { writer: skein_json::writer::Limits { length: 0, ..good.writer }, ..good },
            Err(Problem::Zero),
        ),
    ] {
        cases.push(case).unwrap();
    }
    let mut short = good;
    short.json.string = STRING_BYTES.saturating_sub(1);
    cases.push(("tokenizer string", short, Err(Problem::String))).unwrap();
    let mut short = good;
    short.body_bytes = STRING_BYTES.saturating_add(1);
    cases.push(("body string", short, Err(Problem::String))).unwrap();
    let mut short = good;
    short.json.depth = 9;
    cases.push(("tokenizer depth", short, Err(Problem::Depth))).unwrap();
    let mut short = good;
    short.writer.depth = 9;
    cases.push(("writer depth", short, Err(Problem::Depth))).unwrap();
    for (label, limits, expected) in &cases {
        assert_eq!(limits::check(limits), *expected, "{label}");
    }
}

#[test]
fn github_bearer_renders() {
    let token = Secret::new(Box::from(*b"fixture-token"));
    let rendered = credential::authorization(&Credential::Token(token.clone())).unwrap();
    assert_eq!(rendered.expose(), b"Bearer fixture-token");
    assert_eq!(credential::bearer(&token).unwrap(), rendered);
    let debug = alloc::fmt::format(format_args!("{rendered:?}"));
    assert!(!debug.contains("fixture-token"), "Debug redacts the credential");
}

#[test]
fn github_commit_ids() {
    for (label, text, expected) in [
        (
            "sha1",
            SHA.as_slice(),
            Some([
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01,
                0x23, 0x45, 0x67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ]),
        ),
        ("39 digits", b"0123456789abcdef0123456789abcdef0123456", None),
        ("41 digits", b"0123456789abcdef0123456789abcdef012345678", None),
        ("uppercase", b"0123456789ABCDEF0123456789abcdef01234567", None),
        ("sha256", b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123", None),
    ] {
        assert_eq!(id::commit(text), expected, "{label}");
    }
    let commit = id::commit(SHA).unwrap();
    assert_eq!(id::write_commit(&commit), Some(*SHA));
    let mut non_sha1 = commit;
    non_sha1[31] = 1;
    assert_eq!(id::write_commit(&non_sha1), None);
    assert_eq!(id::repository(1u64 << 32), Err(api::Error::TooLarge));
    assert_eq!(id::repository(u64::from(u32::MAX)), Ok(u32::MAX));
}

#[test]
fn github_request_targets() {
    let good = limits();
    for case in requests() {
        let encoded = request::encode(&case.request, &good).unwrap();
        assert_eq!(encoded.target.as_ref(), case.target, "{}", case.label);
        assert_eq!(encoded.method, Method::Get, "{}", case.label);
        assert_eq!(encoded.body, None, "{}", case.label);
        assert_eq!(encoded.auth, Auth::Installation, "{}", case.label);
        assert_eq!(encoded.accept, if case.raw { Accept::Raw } else { Accept::Json }, "{}", case.label);
        let mut exact = good;
        exact.target_bytes = u32::try_from(case.target.len()).unwrap();
        assert_eq!(request::encode(&case.request, &exact), Ok(encoded), "{} at bound", case.label);
        exact.target_bytes = exact.target_bytes.saturating_sub(1);
        assert_eq!(request::encode(&case.request, &exact), Err(Refusal::TooLong), "{} past bound", case.label);
    }
    let invalid = Request::Locate { owner: Box::from([0xff]), name: Box::from(*b"n") };
    assert_eq!(request::encode(&invalid, &good), Err(Refusal::Text));
    let escaped = Request::Locate { owner: Box::from(*b"o /"), name: Box::from("n?é".as_bytes()) };
    assert_eq!(request::encode(&escaped, &good).unwrap().target.as_ref(), b"/repos/o%20%2F/n%3F%C3%A9");
}

#[test]
fn github_request_fields() {
    for (accept, value) in [
        (Accept::Json, b"application/vnd.github+json".as_slice()),
        (Accept::Raw, b"application/vnd.github.raw+json".as_slice()),
    ] {
        let encoded = request::Encoded {
            method: Method::Get,
            target: Box::from(*b"/"),
            body: None,
            accept,
            auth: Auth::Installation,
            validator: None,
        };
        assert_eq!(
            request::fields(&encoded).as_ref(),
            [(b"Accept".as_slice(), value), (b"X-GitHub-Api-Version".as_slice(), b"2026-03-10".as_slice())]
        );
    }
}

#[test]
fn github_head_fields() {
    let limits = limits();
    for (label, value, expected, cursor) in [
        ("next", b"<https://api.github.com/r?page=2>; rel=\"next\"".as_slice(), true, None),
        ("previous", b"</r?page=1>; rel=\"prev\"", false, None),
        ("cursor", b"</r?cursor=abc%2Fdef>; rel=\"next\"", true, Some(b"abc/def".as_slice())),
        (
            "several",
            b"</r?page=1>; rel=\"prev\", </r?cursor=last>; rel=\"next\", </r?page=9>; rel=\"last\"",
            true,
            Some(b"last".as_slice()),
        ),
        ("broken", b"</r; rel=\"next\"", false, None),
        ("broken quote", b"</r>; rel=\"next", false, None),
        ("wrong relation", b"</r>; rel=\"nextish\"", false, None),
        ("relation list", b"</r>; rel=\"prev next\"", true, None),
    ] {
        let mut head = Head::new();
        header::field(&mut head, b"LiNk", value, &limits);
        assert_eq!(head.next, expected, "{label}");
        let kept = cursor_bytes(&head);
        assert_eq!(kept, cursor, "{label}");
    }
    // ETags and cursors share GitHub's 128 printable byte bound.
    let at = [b'x'; 128];
    let over = [b'x'; 129];
    for (label, value, expected) in [
        ("weak", b"W/\"tag\"".as_slice(), Some(b"W/\"tag\"".as_slice())),
        ("at", at.as_slice(), Some(at.as_slice())),
        ("over", over.as_slice(), None),
        ("control", b"\"a\x01\"", None),
        ("trailing control", b"\"tag\"\r", None),
    ] {
        let mut head = Head::new();
        header::field(&mut head, b"ETag", value, &limits);
        assert_eq!(etag_bytes(&head), expected, "{label}");
        assert_eq!(header::Cursor::new(value).is_some(), expected.is_some(), "{label}");
    }
    for (label, value, expected) in [
        ("json", b"application/json; charset=utf-8".as_slice(), true),
        ("raw", b"text/plain", false),
        ("prefix", b"application/jsonish", false),
    ] {
        let mut head = Head::new();
        header::field(&mut head, b"Content-Type", value, &limits);
        assert_eq!(head.json, expected, "{label}");
    }
    let all = [
        (b"x-ratelimit-limit".as_slice(), b"5000".as_slice()),
        (b"x-ratelimit-remaining", b"0"),
        (b"x-ratelimit-reset", b"1792483260"),
    ];
    for (label, fields, expected) in [
        (
            "all",
            all.as_slice(),
            Some(RateFields { limit: 5000, remaining: 0, reset: Wall::from_nanos(1_792_483_260_000_000_000) }),
        ),
        ("partial", &all[..2], None),
    ] {
        let mut head = Head::new();
        for (name, value) in fields {
            header::field(&mut head, name, value, &limits);
        }
        assert_eq!(head.rate, expected, "{label}");
    }
    for (label, retry, date_first, expected) in [
        ("seconds", b"12".as_slice(), true, Some(Duration::from_nanos(12_000_000_000))),
        ("http date", b"Tue, 20 Oct 2026 08:01:00 GMT", true, Some(Duration::from_nanos(60_000_000_000))),
        ("reverse order", b"Tue, 20 Oct 2026 08:01:00 GMT", false, Some(Duration::from_nanos(60_000_000_000))),
        ("past", b"Tue, 20 Oct 2026 07:59:00 GMT", true, Some(Duration::ZERO)),
        ("invalid", b"tomorrow", true, None),
    ] {
        let mut head = Head::new();
        if date_first {
            header::field(&mut head, b"Date", b"Tue, 20 Oct 2026 08:00:00 GMT", &limits);
        }
        header::field(&mut head, b"Retry-After", retry, &limits);
        if !date_first {
            header::field(&mut head, b"Date", b"Tue, 20 Oct 2026 08:00:00 GMT", &limits);
        }
        assert_eq!(head.retry_after, expected, "{label}");
    }
    let mut head = Head::new();
    header::field(&mut head, b"Connection", b"keep-alive, Close", &limits);
    assert!(head.close, "close in a token list");
    for (value, resource) in [
        (b"core".as_slice(), Resource::Core),
        (b"graphql", Resource::Graphql),
        (b"search", Resource::Search),
        (b"unknown", Resource::Other),
    ] {
        header::field(&mut head, b"x-ratelimit-resource", value, &limits);
        assert_eq!(head.resource, Some(resource));
    }
    let location = [b'/'; 1024];
    header::field(&mut head, b"Location", &location, &limits);
    assert_eq!(head.location, Some(Box::from(location)));
    let location = [b'/'; 1025];
    header::field(&mut head, b"Location", &location, &limits);
    assert_eq!(head.location, None);
}

#[test]
fn github_documents_decode() {
    let good = limits();
    for (label, kind, body, expected, expected_rows) in [
        ("repository", Kind::Repository, REPOSITORY, Document::Repository(fixtures::repository()), &[][..]),
        ("branch", Kind::Branch, BRANCH, Document::Branch(fixtures::branch()), &[][..]),
        (
            "branches",
            Kind::BranchRows,
            BRANCHES,
            Document::Rows,
            &[Row::Branch(Box::from(*b"main")), Row::Branch(Box::from(*b"feat/x y"))][..],
        ),
        ("error", Kind::Error, REFUSAL, Document::Error(fixtures::refusal()), &[][..]),
    ] {
        for chunk in [1, 7, u32::try_from(body.len()).unwrap()] {
            let (rows, document) = decode(kind, body, chunk, &good);
            assert_eq!(document, Ok(expected.clone()), "{label}, chunk {chunk}");
            assert_eq!(rows.as_slice(), expected_rows, "{label}, chunk {chunk}");
        }
    }
    for (label, body, expected) in [
        (
            "unknown",
            br#"{"message":"ok","errors":null,"unknown":{"a":[{"z":null},false,2]}}"#.as_slice(),
            Ok(Document::Error(super::document::Refusal { message: Box::from(*b"ok"), errors: Box::from([]) })),
        ),
        ("missing", b"{}", Err(Malformed::Missing { field: b"message" })),
        ("null", br#"{"message":null}"#, Err(Malformed::Missing { field: b"message" })),
        ("wrong type", br#"{"message":3}"#, Err(Malformed::Shape { field: b"message" })),
        ("duplicate", br#"{"message":"a","message":"b"}"#, Err(Malformed::Shape { field: b"message" })),
        ("root shape", b"[]", Err(Malformed::Shape { field: b"" })),
    ] {
        assert_eq!(decode(Kind::Error, body, 7, &good).1, expected, "{label}");
    }
    for (label, body, expected) in [
        (
            "short commit",
            br#"{"name":"main","commit":{"sha":"0123456789abcdef0123456789abcdef0123456"}}"#.as_slice(),
            Malformed::Id { field: b"sha" },
        ),
        (
            "missing name",
            br#"{"commit":{"sha":"0123456789abcdef0123456789abcdef01234567"}}"#,
            Malformed::Missing { field: b"name" },
        ),
        ("null commit", br#"{"name":"main","commit":null}"#, Malformed::Missing { field: b"commit" }),
    ] {
        assert_eq!(decode(Kind::Branch, body, 7, &good).1, Err(expected), "{label}");
    }
    assert_eq!(
        decode(Kind::Repository, br#"{"id":18446744073709551616}"#, 7, &good).1,
        Err(Malformed::Range { field: b"id" })
    );
    document_bounds(good);
}

#[test]
fn github_failures() {
    failures::check();
}
mod failures;

#[test]
fn github_server_round_trip() {
    let limits = limits();
    for case in requests() {
        let encoded = request::encode(&case.request, &limits).unwrap();
        assert_eq!(server::route(encoded.method, &encoded.target, &limits), Some(case.request), "{}", case.label);
    }
    for target in [
        b"/repos/o/n/issues?per_page=1&page=2".as_slice(),
        b"/repos/o/n/branches?per_page=20&page=2&extra=1",
        b"/repos/%GG/n",
        b"/repositories/18446744073709551616",
        b"/user/3#fragment",
        b"/unknown",
    ] {
        assert_eq!(server::route(Method::Get, target, &limits), None, "{target:?}");
    }
    for method in [Method::Post, Method::Put, Method::Patch, Method::Delete] {
        assert_eq!(server::route(method, b"/repos/o/n", &limits), None);
    }
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::repository(&fixtures::repository(), &mut out).unwrap();
    assert_answer(out, Kind::Repository, REPOSITORY, Document::Repository(fixtures::repository()), &[]);
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::branch(&fixtures::branch(), &mut out).unwrap();
    assert_answer(out, Kind::Branch, BRANCH, Document::Branch(fixtures::branch()), &[]);
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::branches(&[b"main", b"feat/x y"], &mut out).unwrap();
    assert_answer(
        out,
        Kind::BranchRows,
        BRANCHES,
        Document::Rows,
        &[Row::Branch(Box::from(*b"main")), Row::Branch(Box::from(*b"feat/x y"))],
    );
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    let refusal = fixtures::refusal();
    server::refusal(&refusal.message, &[b"detail"], &mut out).unwrap();
    assert_answer(out, Kind::Error, REFUSAL, Document::Error(fixtures::refusal()), &[]);
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::link(b"/repos/o/n/branches?per_page=20&page=1", 2, 9, &mut out).unwrap();
    let value = bytes(out);
    assert_eq!(value.as_ref(),b"<https://api.github.com/repos/o/n/branches?per_page=20&page=2>; rel=\"next\", <https://api.github.com/repos/o/n/branches?per_page=20&page=9>; rel=\"last\"");
    assert_eq!(header::link_next(&value), Some((true, None)));
    let mut small = Writer::new(1);
    assert_eq!(server::branch(&fixtures::branch(), &mut small), Err(skein_lib::Overflow));
    assert_eq!(small.written(), 0);
    assert_eq!(id::write_commit(&id::commit(OTHER_SHA).unwrap()), Some(*OTHER_SHA));
}

fn cursor_bytes(head: &Head) -> Option<&[u8]> {
    Some(head.cursor.as_ref()?.as_bytes())
}
fn etag_bytes(head: &Head) -> Option<&[u8]> {
    Some(head.etag.as_ref()?.as_bytes())
}

fn assert_answer(out: Writer, kind: Kind, golden: &[u8], expected: Document, expected_rows: &[Row]) {
    let body = bytes(out);
    assert_eq!(body.as_ref(), golden);
    let (rows, document) = decode(kind, &body, 7, &limits());
    assert_eq!(document, Ok(expected));
    assert_eq!(rows.as_slice(), expected_rows);
}

fn document_bounds(good: limits::Limits) {
    let long = [b'x'; 129];
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::refusal(&long, &[], &mut out).unwrap();
    let body = bytes(out);
    assert_eq!(decode(Kind::Error, &body, 7, &good).1, Err(Malformed::TooLong { field: b"message" }));
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::refusal(&long[..128], &[], &mut out).unwrap();
    let body = bytes(out);
    assert_eq!(
        decode(Kind::Error, &body, 7, &good).1,
        Ok(Document::Error(super::document::Refusal { message: Box::from(&long[..128]), errors: Box::from([]) }))
    );
    for (label, body, expected) in [
        ("at depth", br#"[{"name":"a","unknown":[[[[[[[[]]]]]]]]}]"#.as_slice(), Ok(Document::Rows)),
        (
            "over depth",
            br#"[{"name":"a","unknown":[[[[[[[[[]]]]]]]]]}]"#,
            Err(Malformed::Json(skein_json::tokenizer::Error::TooDeep)),
        ),
    ] {
        assert_eq!(decode(Kind::BranchRows, body, 7, &good).1, expected, "{label}");
    }
    let mut relaxed_depth = good;
    relaxed_depth.json.depth = limits::DEPTH.saturating_add(1);
    assert_eq!(
        decode(Kind::BranchRows, br#"[{"name":"a","unknown":[[[[[[[[[]]]]]]]]]}]"#, 7, &relaxed_depth).1,
        Err(Malformed::Json(skein_json::tokenizer::Error::TooDeep))
    );
    // An unknown string longer than the retention limit is checked without being kept.
    let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
    server::refusal(b"ok", &[&long], &mut out).unwrap();
    let encoded = bytes(out);
    let mut out = Writer::new(encoded.len().saturating_add(b"{\"unknown\":,\"message\":\"ok\"}".len()));
    out.put(b"{\"unknown\":").unwrap();
    out.put(&encoded).unwrap();
    out.put(b",\"message\":\"ok\"}").unwrap();
    assert_eq!(
        decode(Kind::Error, &out.finish(), 7, &good).1,
        Ok(Document::Error(super::document::Refusal { message: Box::from(*b"ok"), errors: Box::from([]) }))
    );
    let mut body_bound = good;
    body_bound.body_bytes = 2;
    assert_eq!(
        decode(Kind::Error, br#"{"message":"abc"}"#, 7, &body_bound).1,
        Err(Malformed::TooLong { field: b"message" })
    );
    let errors = [b"a".as_slice(); 5];
    for (count, expected) in [
        (
            INNER,
            Ok(Document::Error(super::document::Refusal {
                message: Box::from(*b"m"),
                errors: Box::from([Box::<[u8]>::from(*b"a"), Box::from(*b"a"), Box::from(*b"a"), Box::from(*b"a")]),
            })),
        ),
        (INNER.saturating_add(1), Err(Malformed::Bound { field: b"errors" })),
    ] {
        let mut out = Writer::new(usize::try_from(DOCUMENT_BYTES).unwrap());
        server::refusal(b"m", &errors[..usize::try_from(count).unwrap()], &mut out).unwrap();
        assert_eq!(decode(Kind::Error, &bytes(out), 7, &good).1, expected, "{count} errors");
    }
}

mod writes;

mod pull_graphql;

#[expect(clippy::disallowed_methods, reason = "JSON test fixtures format the shared ASCII refusal constant")]
fn own_review_error_text() -> &'static str {
    core::str::from_utf8(failure::OWN_REVIEW_ERROR).unwrap()
}

#[test]
fn github_refusal_errors_accept_strings_objects_and_mixed_entries() {
    let error = own_review_error_text();
    for entries in [
        alloc::fmt::format(format_args!(r#"["{error}"]"#)),
        alloc::fmt::format(format_args!(r#"[{{"message":"{error}"}}]"#)),
        alloc::fmt::format(format_args!(r#"["detail",{{"message":"{error}"}}]"#)),
    ] {
        let input = alloc::fmt::format(format_args!(r#"{{"message":"Unprocessable Entity","errors":{entries}}}"#));
        for chunk in [1, 7, 4096] {
            let (_, decoded) = decode(Kind::Error, input.as_bytes(), chunk, &limits());
            let Document::Error(refusal) = decoded.unwrap() else { panic!("refusal") };
            assert_eq!(refusal.errors.last().unwrap().as_ref(), failure::OWN_REVIEW_ERROR);
            assert_eq!(
                failure::classify(Asked::Review { empty: true }, 422, &Head::new(), Some(&refusal)),
                Verdict::Fail(api::Error::Refused, Drop::Nothing)
            );
        }
    }
}

#[test]
fn github_refusal_strings_encoder_bounds_and_escapes() {
    let error = own_review_error_text();
    let golden = alloc::fmt::format(format_args!(r#"{{"message":"m\"\n","errors":["detail\\\t","{error}"]}}"#));
    let errors = [b"detail\\\t".as_slice(), failure::OWN_REVIEW_ERROR];
    let mut short = Writer::new(golden.len().checked_sub(1).unwrap());
    assert_eq!(server::refusal_strings(b"m\"\n", &errors, &mut short), Err(skein_lib::Overflow));
    assert_eq!(short.written(), 0);
    let mut exact = Writer::new(golden.len());
    server::refusal_strings(b"m\"\n", &errors, &mut exact).unwrap();
    assert_answer(
        exact,
        Kind::Error,
        golden.as_bytes(),
        Document::Error(super::document::Refusal {
            message: Box::from(b"m\"\n".as_slice()),
            errors: Box::from(errors.map(Box::from)),
        }),
        &[],
    );
}

#[test]
fn turn_calls_are_refused_without_a_request() {
    use super::plans::{
        plan::{Bounds, Plan, Step},
        tables::Tables,
    };
    let head = [0; 32];
    let job = api::JobAttempt { head, run: 1, job: 2, attempt: 1 };
    let bounds = Bounds { inner: 4, page_size: 2, rows: 4, answer_bytes: 4096, body_bytes: 4096 };
    for op in [
        api::Op::Read(api::Read::Runs { workflow: Box::from(*b"ci.yaml"), commit: head, page: 1 }),
        api::Op::Read(api::Read::Run { run: 1 }),
        api::Op::Read(api::Read::Tree { object: head, page: 1 }),
        api::Op::Read(api::Read::Artifact { run: 1, name: Box::from(*b"report"), max_bytes: 1 }),
        api::Op::Read(api::Read::Commits { before: head, after: head, page: 1 }),
        api::Op::Write(api::Write::Dispatch {
            workflow: Box::from(*b"ci.yaml"),
            branch: Box::from(*b"main"),
            inputs: Box::new([]),
        }),
        api::Op::Write(api::Write::Rerun { attempt: job }),
        api::Op::Write(api::Write::Merge { number: 1, head, how: api::MergeHow::FastForwardOnly }),
        api::Op::Write(api::Write::Update { number: 1, style: api::UpdateStyle::Rebase }),
    ] {
        let mut plan = Plan::new(api::Repository { forge: 1, repository: 1 }, op.clone());
        let mut tables = Tables::new(1, 1);
        let result = plan.begin(bounds, b"kit", &mut tables).expect("plan begins");
        let cost: u32 = match &result {
            Step::Send(_) | Step::Redirect { .. } => 1,
            Step::Done(_) => 0,
        };
        assert_eq!((result, cost), (Step::Done(Err(api::Error::Refused)), 0), "{op:?}");
        assert_eq!(plan.pending(), None);
    }
}
