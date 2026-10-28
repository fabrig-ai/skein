use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::Auth,
    github::{
        checks,
        given::Given,
        paste::{self, Paste},
    },
    item::Item,
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use core::num::NonZeroU64;
use skein_forge::github::{
    document::{App, Document, Hook},
    request::Request,
};
use skein_lib::{Queue, bytes};
const APP: u64 = 19;
const BOT: u64 = 37;
const URL: &[u8] = b"https://kit-app.invalid/";
const LAST: usize = 11; // Pasted, kept ids, App, bot, hook read/patch, four writes.
fn new() -> Paste {
    Paste::new(&fixtures::given(), NonZeroU64::new(APP).expect("id")).expect("given")
}
#[expect(clippy::disallowed_macros, reason = "inspect every terminal diagnostic for secret bytes")]
fn go(machine: &mut Paste, event: Event) -> Need {
    let mut out = Queue::uncapped();
    paste::step(machine, event, &mut out);
    assert_eq!(out.len(), 1);
    let need = out.pop().expect("one need");
    match &need {
        Need::Done(_) | Need::Stop(_) => diagnostic(&format!("{need:?}")),
        Need::Read(_) | Need::Write { .. } | Need::Send { .. } | Need::Draw(_) => (),
    }
    need
}
fn document(doc: Document) -> Event {
    Event::Reply(Reply::GithubDocument(doc))
}
fn hook(url: &[u8]) -> Event {
    document(Document::HookConfig(Hook { url: url.into() }))
}
fn send(request: Request) -> Need {
    Need::Send { request: request.into(), auth: Auth::App { id: APP, key: secret(fixtures::DER) } }
}
fn failure(subject: &[u8], why: Why) -> Need {
    Need::Stop(Failure { forge: 4, step: Stage::App, subject: subject.into(), why })
}
fn script() -> [Event; 12] {
    [
        Event::Begin,
        Event::Read(Some(fixtures::pem(fixtures::DER, b"\n", b""))),
        Event::Read(Some(secret(&APP.to_le_bytes()))),
        Event::Read(Some(secret(&BOT.to_le_bytes()))),
        document(Document::App(fixtures::app())),
        document(Document::BotUser(BOT)),
        hook(b"wrong"),
        hook(URL),
        Event::Written,
        Event::Written,
        Event::Written,
        Event::Written,
    ]
}
fn at(phase: usize) -> Paste {
    at_with_given(&fixtures::given(), phase)
}
fn at_with_given(given: &Given, phase: usize) -> Paste {
    let mut machine = Paste::new(given, NonZeroU64::new(APP).expect("id")).expect("given");
    for event in script().into_iter().take(phase) {
        go(&mut machine, event);
    }
    machine
}
fn lacking_app() -> App {
    let mut app = fixtures::app();
    app.permissions = Box::new([]);
    app.events = Box::new([]);
    app
}
fn kept(machine: &mut Paste, mut need: Need, lacking: Box<[checks::Lacking]>) {
    for (item, value) in [
        (Item::AppId, APP.to_le_bytes().as_slice()),
        (Item::AppSlug, b"kit-app"),
        (Item::AppKey, fixtures::DER),
        (Item::User, BOT.to_le_bytes().as_slice()),
    ] {
        assert_eq!(need, Need::Write { item, bytes: secret(value) });
        need = go(machine, Event::Written);
    }
    assert_eq!(need, Need::Done(Outcome::Lacking { lacking }));
    silent(machine);
}
fn silent(machine: &mut Paste) {
    for event in
        [Event::Begin, Event::Read(None), Event::Written, Event::Reply(Reply::Down), Event::Drawn(secret(b"late"))]
    {
        let mut out = Queue::uncapped();
        paste::step(machine, event, &mut out);
        assert!(out.is_empty());
        paste::halt(machine, Why::Down, &mut out);
        assert!(out.is_empty());
    }
}
#[test]
fn paste_reads_pem_once_and_proves_the_app_and_bot() {
    let mut machine = new();
    for (event, expected) in [
        (Event::Begin, Need::Read(Item::Pasted)),
        (Event::Read(Some(fixtures::pem(fixtures::DER, b"\n", b""))), Need::Read(Item::AppId)),
        (Event::Read(None), Need::Read(Item::User)),
        (Event::Read(None), send(Request::App)),
        (
            document(Document::App(fixtures::app())),
            Need::Send { request: Request::BotUser { slug: b"kit-app".as_slice().into() }.into(), auth: Auth::None },
        ),
        (document(Document::BotUser(BOT)), send(Request::HookConfig)),
    ] {
        assert_eq!(go(&mut machine, event), expected);
    }
}
#[test]
fn a_pasted_key_uses_the_conversions_pem_grammar() {
    for ending in [b"\n".as_slice(), b"\r\n"] {
        let mut machine = at(1);
        assert_eq!(
            go(&mut machine, Event::Read(Some(fixtures::pem(fixtures::DER, ending, b" \n")))),
            Need::Read(Item::AppId)
        );
    }
}
#[test]
fn a_paste_of_another_kept_or_answered_app_stops_other_app() {
    let mut machine = at(2);
    assert_eq!(go(&mut machine, Event::Read(Some(secret(&20u64.to_le_bytes())))), failure(b"19", Why::OtherApp));
    silent(&mut machine);
    let mut other = fixtures::app();
    other.id = 20;
    other.slug = b"-invalid".as_slice().into();
    let mut machine = at(4);
    assert_eq!(go(&mut machine, document(Document::App(other))), failure(b"19", Why::OtherApp));
    silent(&mut machine);
}
#[test]
fn a_paste_with_another_kept_bot_stops_other_user_before_mutation() {
    let mut machine = at(5);
    assert_eq!(go(&mut machine, document(Document::BotUser(38))), failure(b"kit-app", Why::OtherUser));
    silent(&mut machine);
}
#[test]
fn paste_repairs_the_hook_and_reports_lacking_access() {
    let mut machine = at(4);
    let app = lacking_app();
    let lacking = checks::lacks(&app);
    go(&mut machine, document(Document::App(app)));
    assert_eq!(go(&mut machine, document(Document::BotUser(BOT))), send(Request::HookConfig));
    assert_eq!(go(&mut machine, hook(b"wrong")), send(Request::SetHook { url: URL.into() }));
    let need = go(&mut machine, hook(URL));
    kept(&mut machine, need, lacking);
}
#[test]
fn paste_keeps_four_items_before_done_and_never_writes_pasted() {
    let mut machine = at(6);
    let need = go(&mut machine, hook(URL)); // Matching hook requires no PATCH.
    kept(&mut machine, need, Box::new([]));
}
#[test]
fn read_only_paste_keeps_items_without_repairing_the_hook() {
    let mut given = fixtures::given();
    given.read_only = true;
    let mut machine = at_with_given(&given, 4);
    let app = lacking_app();
    let lacking = checks::lacks(&app);
    go(&mut machine, document(Document::App(app)));
    assert_eq!(go(&mut machine, document(Document::BotUser(BOT))), send(Request::HookConfig));
    let need = go(&mut machine, hook(b"https://elsewhere.test/"));
    kept(&mut machine, need, lacking);
}
#[test]
fn an_unsigned_refused_or_missing_paste_names_its_step() {
    for (phase, event, expected) in [
        (1, Event::Read(None), failure(b"key", Why::Missing)),
        (1, Event::Read(Some(secret(b""))), failure(b"key", Why::Missing)),
        (1, Event::Read(Some(secret(b"bad-PEM-sentinel"))), failure(b"key", Why::KeyForm)),
        (2, Event::Read(Some(secret(b"bad-id"))), failure(b"id", Why::Unexpected)),
        (3, Event::Read(Some(secret(b"bad-user-id"))), failure(b"user", Why::Unexpected)),
        (4, Event::Reply(Reply::Unsigned), failure(b"key", Why::KeyForm)),
        (4, Event::Reply(Reply::Status(401)), failure(b"19", Why::Refused(401))),
        (4, Event::Reply(Reply::Down), failure(b"19", Why::Down)),
        (5, Event::Reply(Reply::Status(404)), failure(b"kit-app", Why::Missing)),
        (5, document(Document::BotUser(0)), failure(b"kit-app", Why::Unexpected)),
        (6, Event::Reply(Reply::Unsigned), failure(b"key", Why::KeyForm)),
        (7, Event::Reply(Reply::Status(403)), failure(b"kit-app", Why::Refused(403))),
    ] {
        let mut machine = at(phase);
        assert_eq!(go(&mut machine, event), expected);
        silent(&mut machine);
    }
    let mut bad = fixtures::given();
    bad.limits.key_bytes = 0;
    assert_eq!(
        Paste::new(&bad, NonZeroU64::new(APP).expect("id")).err(),
        Some(crate::github::given::Refused { place: crate::github::given::Place::KeyBytes, other: None })
    );
}
#[expect(clippy::disallowed_macros, reason = "check raw secret bytes and their Debug spellings")]
fn diagnostic(debug: &str) {
    let pem = fixtures::pem(fixtures::DER, b"\n", b"");
    for value in [fixtures::DER, pem.expose(), b"sentinel-drawn-secret"] {
        assert!(bytes::find(debug.as_bytes(), value).is_none());
        for spelling in [format!("{value:?}"), format!("{value:x?}"), format!("{value:X?}")] {
            assert!(!debug.contains(&spelling));
        }
    }
}
#[test]
#[expect(clippy::disallowed_macros, reason = "inspect live and terminal diagnostics for secret retention")]
fn paste_rejects_unasked_events_and_drops_secrets_on_every_end() {
    for phase in 0..=LAST {
        let mut machine = at(phase);
        diagnostic(&format!("{machine:?}"));
        let need = go(&mut machine, Event::Drawn(secret(b"sentinel-drawn-secret")));
        let Need::Stop(failure) = &need else { panic!("Stop") };
        assert_eq!(failure.why, Why::Unexpected);
        diagnostic(&format!("{machine:?} {need:?}"));
        assert!(!format!("{machine:?}").contains("Secret"));
        silent(&mut machine);
        let mut machine = at(phase);
        let mut out = Queue::uncapped();
        paste::halt(&mut machine, Why::Down, &mut out);
        assert_eq!(out.len(), 1);
        diagnostic(&format!("{machine:?} {:?}", out.pop()));
        assert!(!format!("{machine:?}").contains("Secret"));
        silent(&mut machine);
    }
    let mut machine = at(LAST);
    let need = go(&mut machine, Event::Written);
    assert_eq!(need, Need::Done(Outcome::Lacking { lacking: Box::new([]) }));
    diagnostic(&format!("{machine:?} {need:?}"));
    assert!(!format!("{machine:?}").contains("Secret"));
    silent(&mut machine);
}

#[test]
fn paste_rejects_each_unasked_event() {
    for (phases, events) in [
        (&[0][..], &[Event::Read(None), Event::Reply(Reply::Down), Event::Written][..]),
        (&[1, 2, 3][..], &[Event::Begin, Event::Reply(Reply::Down), Event::Written][..]),
        (&[4, 5, 6, 7][..], &[Event::Begin, Event::Read(None), Event::Written][..]),
        (&[8, 9, 10, 11][..], &[Event::Begin, Event::Read(None), Event::Reply(Reply::Empty)][..]),
    ] {
        for phase in phases {
            for event in events {
                let mut machine = at(*phase);
                let Need::Stop(failure) = go(&mut machine, event.clone()) else { panic!("Stop") };
                assert_eq!(failure.why, Why::Unexpected);
                silent(&mut machine);
            }
        }
    }
}

#[test]
fn paste_requires_an_admitted_returned_slug_and_app_document() {
    let mut invalid = fixtures::app();
    invalid.slug = b"-invalid".as_slice().into();
    let mut long = fixtures::app();
    long.slug = Box::new([b'a'; 35]); // One past the given's name bound.
    for event in [
        document(Document::App(invalid)),
        document(Document::App(long)),
        document(Document::Done),
        Event::Reply(Reply::Empty),
    ] {
        let mut machine = at(4);
        assert_eq!(go(&mut machine, event), failure(b"19", Why::Unexpected));
        silent(&mut machine);
    }
}
