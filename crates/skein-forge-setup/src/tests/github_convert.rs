use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::{Auth, Call},
    github::{
        convert::{self, Convert},
        given::{Given, Place, Refused, URL_BYTES},
    },
    item::Item,
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::github::{
    document::{App, Conversion, Document},
    request::{self, Request},
};
use skein_lib::{Queue, Secret, Writer, bytes};

const CODE: &[u8] = b"sentinel-conversion-code";
const WEBHOOK: &[u8] = b"sentinel-webhook-secret";
const SLUG: &[u8] = b"renamed-app";
const APP: u64 = 19;
const BOT: u64 = 37;

fn conversion(slug: &[u8], pem: Secret) -> Conversion {
    Conversion {
        id: APP,
        slug: slug.into(),
        owner: b"person".as_slice().into(),
        owner_id: 23,
        pem,
        webhook_secret: secret(WEBHOOK),
    }
}
fn converted(row: Conversion) -> Event {
    Event::Reply(Reply::GithubDocument(Document::Conversion(row)))
}
fn converted_key(pem: Secret) -> Event {
    converted(conversion(SLUG, pem))
}
fn app(id: u64, slug: &[u8]) -> Event {
    Event::Reply(Reply::GithubDocument(Document::App(App {
        id,
        slug: slug.into(),
        permissions: Box::new([]),
        events: Box::new([]),
    })))
}
fn bot(id: u64) -> Event {
    Event::Reply(Reply::GithubDocument(Document::BotUser(id)))
}
fn go(machine: &mut Convert, event: Event) -> Need {
    let mut out = Queue::uncapped();
    convert::step(machine, event, &mut out);
    assert_eq!(out.len(), 1, "one outstanding need");
    out.pop().expect("one need")
}
fn at_given(phase: usize, given: &Given) -> Convert {
    let mut machine = Convert::new(given, CODE).expect("given and code admitted");
    for event in [
        Event::Begin,
        converted_key(fixtures::pem(fixtures::DER, b"\n", b"")),
        app(APP, SLUG),
        bot(BOT),
        Event::Written,
        Event::Written,
        Event::Written,
        Event::Written,
    ]
    .into_iter()
    .take(phase)
    {
        go(&mut machine, event);
    }
    machine
}
fn at(phase: usize) -> Convert {
    at_given(phase, &fixtures::given())
}
fn failure(subject: &[u8], why: Why) -> Need {
    Need::Stop(Failure { forge: fixtures::given().forge, step: Stage::App, subject: subject.into(), why })
}
fn silent(machine: &mut Convert) {
    for event in
        [Event::Begin, Event::Read(None), Event::Reply(Reply::Down), Event::Written, Event::Drawn(secret(CODE))]
    {
        let mut out = Queue::uncapped();
        convert::step(machine, event, &mut out);
        assert!(out.is_empty(), "ended conversion is silent");
    }
}

#[test]
fn a_conversion_proves_the_app_then_resolves_its_bot() {
    let mut machine = at(0);
    // Inspect the code at its use, without formatting the outgoing raw request.
    match go(&mut machine, Event::Begin) {
        Need::Send { request: Call::GitHub(Request::Conversion { code }), auth: Auth::None } => {
            assert!(code.as_ref() == CODE);
        }
        Need::Send { .. } | Need::Read(_) | Need::Write { .. } | Need::Draw(_) | Need::Done(_) | Need::Stop(_) => {
            unreachable!("conversion is unauthenticated")
        }
    }
    assert_eq!(
        go(&mut machine, converted_key(fixtures::pem(fixtures::DER, b"\n", b""))),
        Need::Send { request: Request::App.into(), auth: Auth::App { id: APP, key: secret(fixtures::DER) } }
    );
    let need = go(&mut machine, app(APP, SLUG));
    assert_eq!(need, Need::Send { request: Request::BotUser { slug: SLUG.into() }.into(), auth: Auth::None });
    let limits = skein_forge::github::limits::Limits {
        json: skein_json::tokenizer::Limits { depth: 10, string: 4096, number: 20, length: 4096, chunk: 4096 },
        writer: skein_json::writer::Limits { depth: 10, length: 4096 },
        string_bytes: 1024,
        body_bytes: 4096,
        inner: 32,
        target_bytes: 4096,
    };
    let encoded = request::encode(&Request::BotUser { slug: SLUG.into() }, &limits).expect("bot request encodes");
    assert_eq!(encoded.target.as_ref(), b"/users/renamed-app%5Bbot%5D");
    assert_eq!(encoded.method, request::Method::Get);
    assert_eq!(encoded.auth, request::Auth::None);
    assert_eq!(go(&mut machine, bot(BOT)), Need::Write { item: Item::AppId, bytes: secret(&APP.to_le_bytes()) });
}

#[test]
fn a_conversion_keeps_four_items_before_done() {
    let mut machine = at(3);
    let mut event = bot(BOT);
    for (item, value) in [
        (Item::AppId, APP.to_le_bytes().as_slice()),
        (Item::AppSlug, SLUG),
        (Item::AppKey, fixtures::DER),
        (Item::User, BOT.to_le_bytes().as_slice()),
    ] {
        let mut out = Queue::uncapped();
        convert::step(&mut machine, event, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out.pop(), Some(Need::Write { item, bytes: secret(value) }));
        // Withhold Written: this step has emitted only the outstanding write.
        assert!(out.is_empty());
        event = Event::Written;
    }
    assert_eq!(go(&mut machine, event), Need::Done(Outcome::Nothing));
    silent(&mut machine);
}

#[test]
fn a_conversion_keeps_the_slug_github_answers() {
    for slug in [b"different-name".as_slice(), b"a", b"trailing-", &[b'z'; 34]] {
        let mut machine = at(1);
        go(&mut machine, converted(conversion(slug, fixtures::pem(fixtures::DER, b"\n", b""))));
        assert_eq!(
            go(&mut machine, app(APP, slug)),
            Need::Send { request: Request::BotUser { slug: slug.into() }.into(), auth: Auth::None }
        );
        go(&mut machine, bot(BOT));
        assert_eq!(go(&mut machine, Event::Written), Need::Write { item: Item::AppSlug, bytes: secret(slug) });
    }
    assert_eq!(go(&mut at(2), app(APP, b"different-name")), failure(b"19", Why::Unexpected));
}

fn bound_der() -> Box<[u8]> {
    // The 0x81 envelope of the item decoder fixture, large enough for
    // a PEM at twice the DER length, without an RSA key or a signature.
    let mut out = Writer::new(131);
    out.put(b"\x30\x81\x80").expect("header fits");
    for _ in 0..128_usize {
        out.put(b"q").expect("bounded body");
    }
    out.finish()
}

#[test]
fn a_pem_key_holds_at_its_der_bound() {
    let der = bound_der();
    for (ending, surround) in [(b"\n".as_slice(), b"".as_slice()), (b"\r\n", b" \t\r\n\x0b\x0c")] {
        let mut given = fixtures::given();
        given.limits.key_bytes = u32::try_from(der.len()).expect("small DER");
        let pem = fixtures::pem(&der, ending, surround);
        let mut machine = at_given(1, &given);
        assert_eq!(
            go(&mut machine, converted_key(pem)),
            Need::Send { request: Request::App.into(), auth: Auth::App { id: APP, key: secret(&der) } }
        );
    }
    // Payload lines may be mixed LF and CRLF; only line endings are stripped.
    let mut machine = at(1);
    assert_eq!(
        go(&mut machine, converted_key(fixtures::pem_text(b"MA\r\nMC\nAQc=", b"\n", b""))),
        Need::Send { request: Request::App.into(), auth: Auth::App { id: APP, key: secret(fixtures::DER) } }
    );
    // Surrounding whitespace can fill the separate PEM input bound exactly.
    let pem = fixtures::pem(fixtures::DER, b"\n", b"");
    let bound = fixtures::given().limits.key_bytes.checked_mul(2).expect("fixture input bound");
    let mut padded = Writer::new(usize::try_from(bound).expect("small bound"));
    padded.put(pem.expose()).expect("PEM fits");
    for _ in pem.len()..usize::try_from(bound).expect("small bound") {
        padded.put(b" ").expect("padding fits");
    }
    let pem = Secret::new(padded.finish());
    assert_eq!(
        go(&mut at(1), converted_key(pem)),
        Need::Send { request: Request::App.into(), auth: Auth::App { id: APP, key: secret(fixtures::DER) } }
    );
}

#[test]
fn a_bad_or_oversized_pem_stops_key_form() {
    let pem = fixtures::pem(fixtures::DER, b"\n", b"");
    let mut duplicate = Writer::new(pem.len().checked_mul(2).expect("two blocks"));
    duplicate.put(pem.expose()).expect("first block");
    duplicate.put(pem.expose()).expect("second block");
    for pem in [
        secret(b""),
        secret(b"-----BEGIN PRIVATE KEY-----\nMAMCAQc=\n-----END PRIVATE KEY-----"),
        Secret::new(duplicate.finish()),
        fixtures::pem(fixtures::DER, b"\n", b"junk"),
        secret(b"-----BEGIN RSA PRIVATE KEY-----MAMCAQc=\n-----END RSA PRIVATE KEY-----"),
        secret(b"-----BEGIN RSA PRIVATE KEY-----\nMAMCAQc=-----END RSA PRIVATE KEY-----"),
        fixtures::pem_text(b"MAMCAQc=", b"\r", b""),
        fixtures::pem_text(b"MAMC\rAQc=", b"\n", b""),
        fixtures::pem_text(b"MAMC AQc=", b"\n", b""),
        fixtures::pem_text(b"MAMC\tAQc=", b"\n", b""),
        fixtures::pem_text(b"MAMCAQc!", b"\n", b""),
        fixtures::pem_text(b"MAMCAQc", b"\n", b""),
        fixtures::pem_text(b"MAMCAQc==", b"\n", b""),
        fixtures::pem_text(b"MAMCAQd=", b"\n", b""),
        fixtures::pem_text(b"=MAMCAQc", b"\n", b""),
        fixtures::pem_text(b"MA_CAQc=", b"\n", b""),
        fixtures::pem_text(b"MAMCAQc=\x80", b"\n", b""),
        fixtures::pem_text(b"", b"\n", b""),
        fixtures::pem(b"\x31\x03\x02\x01\x07", b"\n", b""),
        fixtures::pem(b"\x30\x04\x02\x01\x07", b"\n", b""),
        fixtures::pem(b"\x30\x81\x03\x02\x01\x07", b"\n", b""),
        fixtures::pem(b"\x30\x03\x02\x01\x07x", b"\n", b""),
    ] {
        let mut machine = at(1);
        assert_eq!(go(&mut machine, converted_key(pem)), failure(b"key", Why::KeyForm));
        silent(&mut machine);
    }
    let der = bound_der();
    let mut given = fixtures::given();
    given.limits.key_bytes = u32::try_from(der.len()).expect("small DER").checked_sub(1).expect("one short");
    assert_eq!(
        go(&mut at_given(1, &given), converted_key(fixtures::pem(&der, b"\n", b""))),
        failure(b"key", Why::KeyForm)
    );
    // Larger text fails before copying an intermediate base64 buffer.
    let mut given = fixtures::given();
    given.limits.key_bytes = 128;
    assert_eq!(
        go(&mut at_given(1, &given), converted_key(fixtures::pem_text(&[b'A'; 176], b"\n", b""))),
        failure(b"key", Why::KeyForm)
    );
    let pem = fixtures::pem(fixtures::DER, b"\n", b"");
    let mut input = Writer::new(2393);
    input.put(pem.expose()).expect("PEM fits");
    for _ in pem.len()..2393 {
        input.put(b" ").expect("oversized input");
    }
    assert_eq!(go(&mut at(1), converted_key(Secret::new(input.finish()))), failure(b"key", Why::KeyForm));
}

#[test]
fn an_unsigned_or_other_app_conversion_writes_nothing() {
    for (reply, expected) in [
        (Event::Reply(Reply::Unsigned), failure(b"key", Why::KeyForm)),
        (app(20, SLUG), failure(b"19", Why::OtherApp)),
        (app(0, SLUG), failure(b"19", Why::OtherApp)),
    ] {
        let mut machine = at(2);
        assert_eq!(go(&mut machine, reply), expected);
        silent(&mut machine);
    }
    assert_eq!(go(&mut at(1), Event::Reply(Reply::Status(404))), failure(b"convert", Why::Refused(404)));
}

#[test]
fn a_bot_not_found_stops_missing_and_writes_nothing() {
    let mut machine = at(3);
    assert_eq!(go(&mut machine, Event::Reply(Reply::Status(404))), failure(SLUG, Why::Missing));
    silent(&mut machine);
    assert_eq!(go(&mut at(3), bot(0)), failure(SLUG, Why::Unexpected));
}

#[test]
fn a_conversion_names_status_down_and_wrong_document_at_its_step() {
    for (phase, subject) in [(1, b"convert".as_slice()), (2, b"19"), (3, SLUG)] {
        for (reply, why) in [
            (Reply::Status(401), Why::Refused(401)),
            (Reply::Status(403), Why::Refused(403)),
            (Reply::Status(500), Why::Refused(500)),
            (Reply::Down, Why::Down),
            (Reply::GithubDocument(Document::Done), Why::Unexpected),
            (Reply::Empty, Why::Unexpected),
            (Reply::Rows(Box::new([])), Why::Unexpected),
            (Reply::GithubRows(Box::new([])), Why::Unexpected),
            (Reply::Document(skein_forge::forgejo::document::Document::RepoSearch { ok: true }), Why::Unexpected),
        ] {
            let mut machine = at(phase);
            assert_eq!(go(&mut machine, Event::Reply(reply)), failure(subject, why));
            silent(&mut machine);
        }
    }
    assert_eq!(go(&mut at(2), Event::Reply(Reply::Status(404))), failure(b"19", Why::Refused(404)));
    for phase in [1, 3] {
        let subject = if phase == 1 { b"convert".as_slice() } else { SLUG };
        assert_eq!(go(&mut at(phase), Event::Reply(Reply::Unsigned)), failure(subject, Why::Unexpected));
    }
    for (phase, reply) in [(1, app(APP, SLUG)), (2, bot(BOT)), (3, app(APP, SLUG))] {
        let subject = if phase == 1 {
            b"convert".as_slice()
        } else if phase == 2 {
            b"19"
        } else {
            SLUG
        };
        assert_eq!(go(&mut at(phase), reply), failure(subject, Why::Unexpected));
    }
}

#[test]
fn a_conversion_rejects_an_unasked_event_and_ends_once() {
    for phase in 0..=7 {
        for (kind, event) in
            [Event::Begin, Event::Read(None), Event::Reply(Reply::Empty), Event::Written, Event::Drawn(secret(CODE))]
                .into_iter()
                .enumerate()
        {
            if (phase == 0 && kind == 0)
                || ((1..=3).contains(&phase) && kind == 2)
                || ((4..=7).contains(&phase) && kind == 3)
            {
                continue;
            }
            let subject = match phase {
                0 | 1 => b"convert".as_slice(),
                2 => b"19",
                3..=7 => SLUG,
                _ => unreachable!("phases"),
            };
            let mut machine = at(phase);
            assert_eq!(go(&mut machine, event), failure(subject, Why::Unexpected));
            silent(&mut machine);
        }
    }
}

#[test]
fn a_conversion_admits_only_checked_given_and_bounded_nonempty_code() {
    for code in [b"".as_slice(), &[b'c'; URL_BYTES.checked_add(1).expect("one past cap")]] {
        assert_eq!(
            Convert::new(&fixtures::given(), code).expect_err("bad code"),
            Refused { place: Place::Code, other: None }
        );
    }
    for code in [b"x".as_slice(), &[b'c'; URL_BYTES]] {
        Convert::new(&fixtures::given(), code).expect("bounded nonempty code admitted");
    }
    let mut given = fixtures::given();
    given.limits.key_bytes = 0;
    assert_eq!(
        Convert::new(&given, b"").expect_err("given checked first"),
        Refused { place: Place::KeyBytes, other: None }
    );
}

#[test]
fn a_conversion_admits_only_nonzero_id_and_bounded_valid_slug() {
    for slug in [b"".as_slice(), b"-bad", b"Bad", b"a/b", b"a%2fb", &[b'a'; 35]] {
        let mut machine = at(1);
        assert_eq!(
            go(&mut machine, converted(conversion(slug, fixtures::pem(fixtures::DER, b"\n", b"")))),
            failure(b"19", Why::Unexpected)
        );
        silent(&mut machine);
    }
    let mut row = conversion(SLUG, fixtures::pem(fixtures::DER, b"\n", b""));
    row.id = 0;
    assert_eq!(go(&mut at(1), converted(row)), failure(b"convert", Why::Unexpected));
}

#[test]
fn a_halted_conversion_names_its_step_and_ends_once() {
    for phase in 0..=8 {
        let mut machine = at(phase);
        let mut out = Queue::uncapped();
        convert::halt(&mut machine, Why::Down, &mut out);
        if phase == 8 {
            assert!(out.is_empty());
        } else {
            let subject = match phase {
                0 | 1 => b"convert".as_slice(),
                2 => b"19",
                3..=7 => SLUG,
                _ => unreachable!("live phases"),
            };
            assert_eq!(out.len(), 1);
            assert_eq!(out.pop(), Some(failure(subject, Why::Down)));
        }
        convert::halt(&mut machine, Why::Down, &mut out);
        assert!(out.is_empty());
        silent(&mut machine);
    }
}

#[test]
#[expect(
    clippy::disallowed_macros,
    reason = "the test checks diagnostics for secret bytes in text and numeric Debug forms"
)]
fn a_conversion_debug_contains_no_key_secret_or_code() {
    let pem = fixtures::pem(fixtures::DER, b"\n", b"");
    for phase in 0..=8 {
        let mut machine = at(phase);
        let live = format!("{machine:?}");
        assert_eq!(live.contains("Secret"), phase == 0 || (2..=5).contains(&phase));
        let mut out = Queue::uncapped();
        convert::halt(&mut machine, Why::Down, &mut out);
        let failure = out.pop();
        let stopped = format!("{machine:?} {failure:?}");
        for debug in [live, stopped] {
            for value in [CODE, pem.expose(), fixtures::DER, WEBHOOK] {
                assert!(bytes::find(debug.as_bytes(), value).is_none());
                assert!(!debug.contains(&format!("{value:?}")));
                assert!(!debug.contains(&format!("{value:x?}")));
                assert!(!debug.contains(&format!("{value:X?}")));
            }
        }
        assert!(!format!("{machine:?}").contains("Secret"));
    }
    let mut machine = at(7);
    let outcome = go(&mut machine, Event::Written);
    let debug = format!("{machine:?} {outcome:?}");
    for value in [CODE, pem.expose(), fixtures::DER, WEBHOOK] {
        assert!(bytes::find(debug.as_bytes(), value).is_none());
        assert!(!debug.contains(&format!("{value:?}")));
    }
    assert!(!debug.contains("Secret"));
    for phase in [1, 2, 3] {
        let mut machine = at(phase);
        let failure = go(&mut machine, Event::Reply(Reply::Down));
        assert!(!format!("{machine:?} {failure:?}").contains("Secret"));
    }
}
