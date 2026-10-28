use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::Auth,
    github::{
        checks::Lacking,
        given::{Given, Place, Refused},
        reconcile::{self, Reconcile},
    },
    item::{self, Item},
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::github::{
    document::{Account, App, Document, Hook, Installation, Row},
    request::Request,
    setup::Access,
};
use skein_lib::{Queue, Wall, bytes};

const APP: u64 = 19;
const URL: &[u8] = b"https://kit-app.invalid/";
const NOW: Wall = Wall::from_nanos(1_200_000_000_000);
const ACCEPTED: &[u64] = &[1, 9];
const LAST_LIVE: usize = 10; // id, key, accepted, App, hook read/patch, two pages, write, delete.

fn new() -> Reconcile {
    Reconcile::new(&fixtures::given(), NOW).expect("checked given")
}
fn go(machine: &mut Reconcile, event: Event) -> Need {
    let mut out = Queue::uncapped();
    reconcile::step(machine, event, &mut out);
    assert_eq!(out.len(), 1, "one need or terminal event");
    let need = out.pop().expect("one need");
    assert!(out.is_empty());
    need
}
fn send(request: Request) -> Need {
    Need::Send { request: request.into(), auth: Auth::App { id: APP, key: secret(fixtures::DER) } }
}
fn document(app: App) -> Event {
    Event::Reply(Reply::GithubDocument(Document::App(app)))
}
fn hook(url: &[u8]) -> Event {
    Event::Reply(Reply::GithubDocument(Document::HookConfig(Hook { url: url.into() })))
}
fn rows(ids: &[u64]) -> Event {
    let mut rows = skein_lib::List::with_capacity(u32::try_from(ids.len()).expect("fixture count"));
    for id in ids {
        rows.push(Row::Installation(Installation {
            id: *id,
            login: b"acme".as_slice().into(),
            account: 27,
            kind: Account::Organization,
            created: Wall::EPOCH,
            suspended: false,
        }))
        .expect("fixture rows");
    }
    Event::Reply(Reply::GithubRows(rows.into_boxed()))
}
fn failure(subject: &[u8], why: Why) -> Need {
    Need::Stop(Failure { forge: 4, step: Stage::App, subject: subject.into(), why })
}
fn done(lacking: Box<[Lacking]>) -> Need {
    Need::Done(Outcome::Lacking { lacking })
}
fn silent(machine: &mut Reconcile) {
    for event in
        [Event::Begin, Event::Read(None), Event::Reply(Reply::Down), Event::Written, Event::Drawn(secret(b"late"))]
    {
        let mut out = Queue::uncapped();
        reconcile::step(machine, event, &mut out);
        assert!(out.is_empty());
        reconcile::halt(machine, Why::Down, &mut out);
        assert!(out.is_empty());
    }
}
fn at(phase: usize) -> Reconcile {
    at_with_given(&fixtures::given(), ACCEPTED, phase)
}
fn at_with_given(given: &Given, accepted: &[u64], phase: usize) -> Reconcile {
    let mut machine = Reconcile::new(given, NOW).expect("checked given");
    for event in [
        Event::Begin,
        Event::Read(Some(secret(&APP.to_le_bytes()))),
        Event::Read(Some(secret(fixtures::DER))),
        Event::Read(Some(item::accepted_item(accepted))),
        document(fixtures::app()),
        hook(b"https://wrong.invalid/"),
        hook(URL),
        rows(&[3, 1]),
        rows(&[]),
        Event::Written,
        Event::Reply(Reply::Empty),
    ]
    .into_iter()
    .take(phase)
    {
        go(&mut machine, event);
    }
    machine
}
fn read_only_listing(accepted: &[u64]) -> (Reconcile, Need) {
    let mut given = fixtures::given();
    given.read_only = true;
    let mut machine = at_with_given(&given, accepted, 4);
    let mut app = fixtures::app();
    app.permissions = Box::new([]);
    assert_eq!(go(&mut machine, document(app)), send(Request::HookConfig));
    assert_eq!(go(&mut machine, hook(b"https://elsewhere.test/")), send(Request::Installations { page: 1 }));
    // Id 3 is older than state_life; accepted id 9 is absent.
    assert_eq!(go(&mut machine, rows(&[3, 1])), send(Request::Installations { page: 2 }));
    let need = go(&mut machine, rows(&[]));
    (machine, need)
}

#[test]
fn read_only_reconcile_prunes_once_and_leaves_the_app_untouched() {
    let mut app = fixtures::app();
    app.permissions = Box::new([]);
    let expected = done(crate::github::checks::lacks(&app));
    let (mut first, need) = read_only_listing(ACCEPTED);
    assert_eq!(need, Need::Write { item: Item::Accepted, bytes: item::accepted_item(&[1]) });
    assert_eq!(go(&mut first, Event::Written), expected);
    silent(&mut first);
    let (mut second, need) = read_only_listing(&[1]);
    assert_eq!(need, expected);
    silent(&mut second);
}

#[test]
fn reconcile_reads_the_app_before_any_mutation() {
    let mut machine = new();
    for (event, expected) in [
        (Event::Begin, Need::Read(Item::AppId)),
        (Event::Read(Some(secret(&APP.to_le_bytes()))), Need::Read(Item::AppKey)),
        (Event::Read(Some(secret(fixtures::DER))), Need::Read(Item::Accepted)),
        (Event::Read(None), send(Request::App)),
    ] {
        assert_eq!(go(&mut machine, event), expected);
    }
}

#[test]
fn reconcile_checks_lacks_then_repairs_hook_then_sweeps_installations() {
    let mut app = fixtures::app();
    app.permissions = Box::new([]);
    let mut machine = at(4);
    assert_eq!(go(&mut machine, document(app)), send(Request::HookConfig));
    assert_eq!(go(&mut machine, hook(b"")), send(Request::SetHook { url: URL.into() }));
    assert_eq!(go(&mut machine, hook(URL)), send(Request::Installations { page: 1 }));
    assert_eq!(go(&mut machine, rows(&[3, 1])), send(Request::Installations { page: 2 }));
    assert_eq!(go(&mut machine, rows(&[])), Need::Write { item: Item::Accepted, bytes: item::accepted_item(&[1]) });
    assert_eq!(go(&mut machine, Event::Written), send(Request::Uninstall { id: 3 }));
    assert_eq!(
        go(&mut machine, Event::Reply(Reply::Empty)),
        done(Box::new([
            Lacking::Permission { name: b"contents".as_slice().into(), access: Access::Write },
            Lacking::Permission { name: b"pull_requests".as_slice().into(), access: Access::Write },
            Lacking::Permission { name: b"issues".as_slice().into(), access: Access::Write },
            Lacking::Permission { name: b"merge_queues".as_slice().into(), access: Access::Write },
            Lacking::Permission { name: b"checks".as_slice().into(), access: Access::Read },
            Lacking::Permission { name: b"statuses".as_slice().into(), access: Access::Read },
            Lacking::Permission { name: b"actions".as_slice().into(), access: Access::Read },
            Lacking::Permission { name: b"administration".as_slice().into(), access: Access::Read },
            Lacking::Permission { name: b"metadata".as_slice().into(), access: Access::Read },
        ]))
    );
    silent(&mut machine);
}

#[test]
fn a_matching_second_reconcile_writes_nothing() {
    for _ in 0..2u8 {
        let mut machine = at(4);
        assert_eq!(go(&mut machine, document(fixtures::app())), send(Request::HookConfig));
        assert_eq!(go(&mut machine, hook(URL)), send(Request::Installations { page: 1 }));
        assert_eq!(go(&mut machine, rows(&[9, 1])), send(Request::Installations { page: 2 }));
        assert_eq!(go(&mut machine, rows(&[])), done(Box::new([])));
        silent(&mut machine);
    }
}

#[test]
fn a_refused_or_other_app_reconcile_stops_before_the_hook() {
    let mut other = fixtures::app();
    other.id = 20;
    // OtherApp takes precedence over the returned slug's syntax.
    other.slug = b"-invalid".as_slice().into();
    for (event, expected) in [
        (Event::Reply(Reply::Status(401)), failure(b"19", Why::Refused(401))),
        (Event::Reply(Reply::Status(403)), failure(b"19", Why::Refused(403))),
        (document(other), failure(b"19", Why::OtherApp)),
        (Event::Reply(Reply::Unsigned), failure(b"key", Why::KeyForm)),
        (Event::Reply(Reply::Down), failure(b"19", Why::Down)),
    ] {
        let mut machine = at(4);
        assert_eq!(go(&mut machine, event), expected);
        silent(&mut machine);
    }
}

#[test]
fn a_reconcile_waits_for_accepted_written_before_done() {
    let mut machine = at(5);
    go(&mut machine, hook(URL));
    assert_eq!(go(&mut machine, rows(&[])), Need::Write { item: Item::Accepted, bytes: item::accepted_item(&[]) });
    assert_eq!(go(&mut machine, Event::Written), done(Box::new([])));
    silent(&mut machine);
}

fn released(debug: &str) {
    assert!(!debug.contains("Secret"));
    assert!(!debug.contains("Kept"));
    assert!(!debug.contains("Set"));
}
#[expect(clippy::disallowed_macros, reason = "check raw secret bytes and every numeric Debug spelling")]
fn diagnostic(debug: &str) {
    for value in
        [fixtures::DER, b"sentinel-drawn-secret", b"sentinel-token", b"sentinel-webhook", b"sentinel-pasted-key"]
    {
        assert!(bytes::find(debug.as_bytes(), value).is_none());
        assert!(!debug.contains(&format!("{value:?}")));
        assert!(!debug.contains(&format!("{value:x?}")));
        assert!(!debug.contains(&format!("{value:X?}")));
    }
}

#[expect(clippy::disallowed_macros, reason = "check all diagnostics for secret bytes and confirm state is released")]
fn released_at(phase: usize) {
    let mut stopped = at(phase);
    let live = format!("{stopped:?}");
    let stop = go(&mut stopped, Event::Drawn(secret(b"sentinel-drawn-secret")));
    let stop_debug = format!("{stopped:?} {stop:?}");
    released(&stop_debug);
    let mut halted = at(phase);
    let mut out = Queue::uncapped();
    reconcile::halt(&mut halted, Why::Down, &mut out);
    assert_eq!(out.len(), 1);
    let halt_debug = format!("{halted:?} {:?}", out.pop().expect("halt"));
    released(&halt_debug);
    diagnostic(&format!("{live} {stop_debug} {halt_debug}"));
    silent(&mut stopped);
    silent(&mut halted);
}
#[test]
fn reconcile_drops_secrets_at_phase_0() {
    released_at(0);
}
#[test]
fn reconcile_drops_secrets_at_phase_1() {
    released_at(1);
}
#[test]
fn reconcile_drops_secrets_at_phase_2() {
    released_at(2);
}
#[test]
fn reconcile_drops_secrets_at_phase_3() {
    released_at(3);
}
#[test]
fn reconcile_drops_secrets_at_phase_4() {
    released_at(4);
}
#[test]
fn reconcile_drops_secrets_at_phase_5() {
    released_at(5);
}
#[test]
fn reconcile_drops_secrets_at_phase_6() {
    released_at(6);
}
#[test]
fn reconcile_drops_secrets_at_phase_7() {
    released_at(7);
}
#[test]
fn reconcile_drops_secrets_at_phase_8() {
    released_at(8);
}
#[test]
fn reconcile_drops_secrets_at_phase_9() {
    released_at(9);
}
#[test]
fn reconcile_drops_secrets_at_phase_10() {
    released_at(10);
}
#[test]
#[expect(clippy::disallowed_macros, reason = "check completed diagnostics for secret bytes")]
fn reconcile_drops_secrets_on_done() {
    let mut completed = at(LAST_LIVE);
    let need = go(&mut completed, Event::Reply(Reply::Empty));
    assert_eq!(need, done(Box::new([])));
    let done_debug = format!("{completed:?} {need:?}");
    released(&done_debug);
    diagnostic(&format!("{done_debug} {:?}", send(Request::App)));
    silent(&mut completed);
}

#[test]
fn reconcile_rejects_unasked_events_and_ignores_ended_events() {
    for (phases, bad) in [
        (&[0][..], &[Event::Read(None), Event::Written, Event::Reply(Reply::Down)][..]),
        (&[1, 2, 3][..], &[Event::Begin, Event::Written, Event::Reply(Reply::Down)][..]),
        (&[4, 5, 6, 7, 8, 10][..], &[Event::Begin, Event::Read(None), Event::Written][..]),
        (&[9][..], &[Event::Begin, Event::Read(None), Event::Reply(Reply::Empty)][..]),
    ] {
        for phase in phases {
            for event in bad {
                let mut machine = at(*phase);
                let Need::Stop(failure) = go(&mut machine, event.clone()) else { panic!("one Stop") };
                assert_eq!(failure.why, Why::Unexpected);
                silent(&mut machine);
            }
        }
    }
}

#[test]
fn missing_and_malformed_items_stop_at_the_item() {
    for (phase, event, expected) in [
        (1, Event::Read(None), failure(b"id", Why::Missing)),
        (1, Event::Read(Some(secret(&[]))), failure(b"id", Why::Unexpected)),
        (1, Event::Read(Some(secret(&0u64.to_le_bytes()))), failure(b"id", Why::Unexpected)),
        (2, Event::Read(None), failure(b"key", Why::Missing)),
        (2, Event::Read(Some(secret(b"not-der"))), failure(b"key", Why::Unexpected)),
        (3, Event::Read(Some(secret(b"bad-set"))), failure(b"accepted", Why::Unexpected)),
        (3, Event::Read(Some(item::accepted_item(&[9, 1]))), failure(b"accepted", Why::Unexpected)),
    ] {
        let mut machine = at(phase);
        assert_eq!(go(&mut machine, event), expected);
        silent(&mut machine);
    }
}

#[test]
fn missing_or_empty_accepted_means_no_accepted_installations() {
    for read in [None, Some(secret(&[]))] {
        let mut machine = at(3);
        assert_eq!(go(&mut machine, Event::Read(read)), send(Request::App));
        go(&mut machine, document(fixtures::app()));
        go(&mut machine, hook(URL));
        assert_eq!(go(&mut machine, rows(&[])), done(Box::new([])));
        silent(&mut machine);
    }
}

#[test]
fn an_unusable_app_document_stops_before_the_hook() {
    let mut invalid_slug = fixtures::app();
    invalid_slug.slug = b"-invalid".as_slice().into();
    let mut long_slug = fixtures::app();
    long_slug.slug = Box::new([b'a'; 35]); // one past the given's 34-byte name bound.
    for event in [
        document(invalid_slug),
        document(long_slug),
        Event::Reply(Reply::GithubDocument(Document::Done)),
        Event::Reply(Reply::GithubRows(Box::new([]))),
        Event::Reply(Reply::Rows(Box::new([]))),
        Event::Reply(Reply::Empty),
    ] {
        let mut machine = at(4);
        assert_eq!(go(&mut machine, event), failure(b"19", Why::Unexpected));
        silent(&mut machine);
    }
}

#[test]
fn the_returned_slug_names_hook_repairs_and_failures() {
    let mut app = fixtures::app();
    app.slug = b"proved-app".as_slice().into();
    let mut machine = at(4);
    go(&mut machine, document(app));
    assert_eq!(
        go(&mut machine, hook(b"")),
        send(Request::SetHook { url: b"https://proved-app.invalid/".as_slice().into() })
    );
    assert_eq!(go(&mut machine, Event::Reply(Reply::Status(403))), failure(b"proved-app", Why::Refused(403)));
    silent(&mut machine);
}

#[test]
fn the_reconcile_constructor_checks_given_and_its_heap_bound() {
    let mut given: Given = fixtures::given();
    given.limits.key_bytes = 0;
    assert_eq!(Reconcile::new(&given, NOW).err(), Some(Refused { place: Place::KeyBytes, other: None }));
    assert!(reconcile::worst_case(&fixtures::given()).is_some());
    given.limits.pages = u32::MAX;
    assert_eq!(reconcile::worst_case(&given), None);
}
