use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::Auth,
    github::{
        app::Kept,
        checks,
        given::Given,
        installations::{self, Progress, Sweep},
    },
    item::{self, Item},
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use core::num::NonZeroU64;
use skein_forge::github::{
    document::{Account, Document, Installation, Row},
    pin::PAGE_CAP,
    request::Request,
};
use skein_lib::{List, Queue, Wall, bytes};

const APP: u64 = 19;
const KEY: &[u8] = b"sentinel-sweep-app-key";
// Two accepted ids and two nonempty pages plus their terminal response.
const ACCEPTED_CAP: u32 = 2;
const PAGES: u32 = 3;
// The fixtures' state_life is 600 seconds; this snapshot is 1200 seconds.
const NOW: Wall = Wall::from_nanos(1_200_000_000_000);
const BOUND: Wall = Wall::from_nanos(600_000_000_000);
const PAST_BOUND: Wall = Wall::from_nanos(599_999_999_999);
const YOUNGER: Wall = Wall::from_nanos(600_000_000_001);
const FUTURE: Wall = Wall::from_nanos(1_200_000_000_001);

fn given() -> Given {
    let mut given = fixtures::given();
    given.limits.installations = ACCEPTED_CAP;
    given.limits.pages = PAGES;
    given
}
fn new(given: &Given, ids: &[u64]) -> Sweep {
    crate::github::given::check(given).expect("checked inputs");
    let accepted = item::accepted(item::accepted_item(ids).expose(), given.limits.installations).expect("decoded ids");
    Sweep::new(given, Kept::new(NonZeroU64::new(APP).expect("nonzero App"), secret(KEY)), accepted, NOW)
}
fn installation(id: u64, created: Wall, suspended: bool) -> Installation {
    Installation { id, login: b"acme".as_slice().into(), account: 27, kind: Account::Organization, created, suspended }
}
fn row(id: u64, created: Wall, suspended: bool) -> Row {
    Row::Installation(installation(id, created, suspended))
}
fn rows(rows: &[Row]) -> Event {
    Event::Reply(Reply::GithubRows(rows.into()))
}
fn go(machine: &mut Sweep, event: Event) -> Progress {
    let mut out = Queue::uncapped();
    installations::step(machine, event, &mut out);
    assert_eq!(out.len(), 1, "one pending need or internal completion");
    out.pop().expect("one progress")
}
fn send(request: Request) -> Progress {
    Progress::Pending(Need::Send { request: request.into(), auth: Auth::App { id: APP, key: secret(KEY) } })
}
fn page(page: u32) -> Progress {
    send(Request::Installations { page })
}
fn write(ids: &[u64]) -> Progress {
    Progress::Pending(Need::Write { item: Item::Accepted, bytes: item::accepted_item(ids) })
}
fn failure(stage: Stage, subject: &[u8], why: Why) -> Progress {
    Progress::Pending(Need::Stop(Failure { forge: given().forge, step: stage, subject: subject.into(), why }))
}
fn listing_failure(why: Why) -> Progress {
    failure(Stage::Installation, b"installations", why)
}
fn silent(machine: &mut Sweep) {
    for event in [
        Event::Begin,
        Event::Read(None),
        Event::Read(Some(secret(KEY))),
        rows(&[]),
        Event::Reply(Reply::Empty),
        Event::Reply(Reply::Unsigned),
        Event::Written,
        Event::Drawn(secret(KEY)),
    ] {
        let mut out = Queue::uncapped();
        installations::step(machine, event, &mut out);
        assert!(out.is_empty(), "terminal child stays silent");
    }
}
fn begun(ids: &[u64]) -> Sweep {
    let mut machine = new(&given(), ids);
    go(&mut machine, Event::Begin);
    machine
}
// Begin, page, terminal page (writes Accepted), Written (deletes id 3).
fn at(phase: usize) -> Sweep {
    let mut machine = new(&given(), &[1, 9]);
    for event in
        [Event::Begin, rows(&[row(3, Wall::EPOCH, false), row(1, Wall::EPOCH, true)]), rows(&[]), Event::Written]
            .into_iter()
            .take(phase)
    {
        go(&mut machine, event);
    }
    machine
}

#[test]
fn a_sweep_reads_every_page_before_it_mutates() {
    let mut machine = new(&given(), &[7, 9]);
    assert_eq!(go(&mut machine, Event::Begin), page(1));
    assert_eq!(go(&mut machine, rows(&[row(3, Wall::EPOCH, false)])), page(2));
    assert_eq!(go(&mut machine, rows(&[row(7, Wall::EPOCH, false)])), page(3));
    assert_eq!(go(&mut machine, rows(&[])), write(&[7]));
    assert_eq!(go(&mut machine, Event::Written), send(Request::Uninstall { id: 3 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), Progress::Complete);
    silent(&mut machine);
}

#[test]
fn a_short_nonempty_installations_page_is_not_the_end() {
    let mut machine = begun(&[7]);
    assert_eq!(go(&mut machine, rows(&[row(7, NOW, false)])), page(2));
    assert_eq!(go(&mut machine, rows(&[row(3, Wall::EPOCH, false)])), page(3));
    assert_eq!(go(&mut machine, rows(&[])), send(Request::Uninstall { id: 3 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), Progress::Complete);
    silent(&mut machine);
}

#[test]
fn a_complete_sweep_drops_only_missing_accepted_ids() {
    for (ids, listed, expected) in [(&[1, 9][..], &[row(1, NOW, false)][..], &[1][..]), (&[1, 9][..], &[][..], &[][..])]
    {
        let mut machine = begun(ids);
        if !listed.is_empty() {
            go(&mut machine, rows(listed));
        }
        assert_eq!(go(&mut machine, rows(&[])), write(expected));
        assert_eq!(go(&mut machine, Event::Written), Progress::Complete);
        silent(&mut machine);
    }
}

#[test]
fn an_agreeing_accepted_set_is_not_written() {
    // Encounter order differs from the stored ascending order.
    for ids in [&[1, 9][..], &[][..]] {
        let mut machine = begun(ids);
        let mut listed = List::with_capacity(ACCEPTED_CAP);
        for id in ids.iter().rev() {
            listed.push(row(*id, Wall::EPOCH, true)).expect("accepted fixture rows");
        }
        if !listed.is_empty() {
            go(&mut machine, rows(listed.as_slice()));
        }
        assert_eq!(go(&mut machine, rows(&[])), Progress::Complete);
        silent(&mut machine);
    }
}

#[test]
fn retained_accepted_ids_are_written_in_ascending_order() {
    // Leave room for one missing id as well as two retained ids.
    let mut given = given();
    given.limits.installations = 3;
    let mut machine = new(&given, &[1, 7, 9]);
    go(&mut machine, Event::Begin);
    assert_eq!(go(&mut machine, rows(&[row(7, NOW, true), row(1, NOW, false)])), page(2));
    assert_eq!(go(&mut machine, rows(&[])), write(&[1, 7]));
    assert_eq!(go(&mut machine, Event::Written), Progress::Complete);
    silent(&mut machine);
}

#[test]
fn a_stray_is_uninstalled_only_past_the_state_life() {
    for (created, suspended) in [(BOUND, false), (YOUNGER, false), (FUTURE, false), (NOW, true)] {
        let mut machine = begun(&[]);
        go(&mut machine, rows(&[row(3, created, suspended)]));
        assert_eq!(go(&mut machine, rows(&[])), Progress::Complete);
        silent(&mut machine);
    }
    for (created, suspended) in [(PAST_BOUND, false), (Wall::EPOCH, true)] {
        let mut machine = begun(&[7]);
        go(&mut machine, rows(&[row(3, created, suspended), row(7, Wall::EPOCH, true)]));
        assert_eq!(go(&mut machine, rows(&[])), send(Request::Uninstall { id: 3 }));
        assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), Progress::Complete);
        silent(&mut machine);
    }
}

#[test]
fn a_sweep_deletes_strays_in_id_order_and_accepts_404() {
    let mut machine = begun(&[]);
    assert_eq!(go(&mut machine, rows(&[row(9, Wall::EPOCH, false), row(1, Wall::EPOCH, true)])), page(2));
    assert_eq!(go(&mut machine, rows(&[row(7, Wall::EPOCH, false)])), page(3));
    assert_eq!(go(&mut machine, rows(&[])), send(Request::Uninstall { id: 1 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Status(404))), send(Request::Uninstall { id: 7 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), send(Request::Uninstall { id: 9 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Status(404))), Progress::Complete);
    silent(&mut machine);
}

#[test]
fn a_failed_listing_at_the_bound_neither_prunes_nor_uninstalls() {
    let mut given = given();
    given.limits.pages = 2;
    for (reply, expected) in [
        (
            Reply::GithubRows(Box::from([row(7, Wall::EPOCH, false), row(0, NOW, false)])),
            listing_failure(Why::Unexpected),
        ),
        (Reply::GithubRows(Box::from([row(3, Wall::EPOCH, false)])), listing_failure(Why::Unexpected)),
        (
            Reply::GithubRows(full_page(PAGE_CAP.checked_add(1).expect("one past cap"))),
            listing_failure(Why::Unexpected),
        ),
        (Reply::Status(403), listing_failure(Why::Refused(403))),
        (Reply::Down, listing_failure(Why::Down)),
        (Reply::Unsigned, failure(Stage::App, b"key", Why::KeyForm)),
    ] {
        let mut machine = new(&given, &[1, 9]);
        assert_eq!(go(&mut machine, Event::Begin), page(1));
        assert_eq!(go(&mut machine, rows(&[row(3, Wall::EPOCH, false)])), page(2));
        assert_eq!(go(&mut machine, Event::Reply(reply)), expected);
        silent(&mut machine);
    }
}

#[test]
fn an_empty_terminal_page_holds_at_the_page_bound() {
    for pages in [1, PAGES] {
        let mut given = given();
        given.limits.pages = pages;
        let mut machine = new(&given, &[]);
        go(&mut machine, Event::Begin);
        for page in 1..pages {
            go(&mut machine, rows(&[row(u64::from(page), NOW, false)]));
        }
        assert_eq!(go(&mut machine, rows(&[])), Progress::Complete);
        silent(&mut machine);
    }
}

#[test]
fn a_nonempty_page_at_the_page_bound_is_crowded() {
    for pages in [1, PAGES] {
        let mut given = given();
        given.limits.pages = pages;
        let mut machine = new(&given, &[9]);
        go(&mut machine, Event::Begin);
        for number in 1..pages {
            assert_eq!(
                go(&mut machine, rows(&[row(u64::from(number), NOW, false)])),
                page(number.checked_add(1).expect("small page"))
            );
        }
        assert_eq!(go(&mut machine, rows(&[row(7, NOW, false)])), listing_failure(Why::Crowded));
        silent(&mut machine);
    }
}

#[test]
fn a_page_at_the_bound_uninstalls_strays_then_ends_crowded() {
    let mut machine = begun(&[1, 9]);
    assert_eq!(go(&mut machine, rows(&[row(8, Wall::EPOCH, false), row(1, Wall::EPOCH, true)])), page(2));
    assert_eq!(go(&mut machine, rows(&[row(3, PAST_BOUND, true)])), page(3));
    assert_eq!(
        go(&mut machine, rows(&[row(2, Wall::EPOCH, false), row(7, FUTURE, false)])),
        send(Request::Uninstall { id: 2 })
    );
    assert_eq!(go(&mut machine, Event::Reply(Reply::Status(404))), send(Request::Uninstall { id: 3 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), send(Request::Uninstall { id: 8 }));
    assert_eq!(go(&mut machine, Event::Reply(Reply::Status(404))), listing_failure(Why::Crowded));
    silent(&mut machine);
}

#[test]
fn a_crowded_sweep_with_no_stray_ends_crowded_at_once() {
    let mut given = given();
    given.limits.pages = 1;
    for accepted in [&[][..], &[1, 9][..]] {
        let mut machine = new(&given, accepted);
        assert_eq!(go(&mut machine, Event::Begin), page(1));
        assert_eq!(go(&mut machine, rows(&[row(1, NOW, false), row(7, FUTURE, false)])), listing_failure(Why::Crowded));
        silent(&mut machine);
    }
}

#[test]
fn a_read_only_sweep_leaves_strays_and_ends_crowded() {
    for pages in [1, PAGES] {
        let mut given = given();
        given.read_only = true;
        given.limits.pages = pages;
        let mut machine = new(&given, &[1, 9]);
        assert_eq!(go(&mut machine, Event::Begin), page(1));
        for number in 1..pages {
            assert_eq!(
                go(&mut machine, rows(&[row(u64::from(number), Wall::EPOCH, false)])),
                page(number.checked_add(1).expect("small page"))
            );
        }
        assert_eq!(go(&mut machine, rows(&[row(7, Wall::EPOCH, true)])), listing_failure(Why::Crowded));
        silent(&mut machine);
    }
}

#[test]
fn a_failed_delete_at_the_bound_names_its_installation() {
    for (reply, expected) in [
        (Reply::Status(403), failure(Stage::Installation, b"12345", Why::Refused(403))),
        (Reply::Down, failure(Stage::Installation, b"12345", Why::Down)),
        (Reply::GithubRows(Box::new([])), failure(Stage::Installation, b"12345", Why::Unexpected)),
        (Reply::Unsigned, failure(Stage::App, b"key", Why::KeyForm)),
    ] {
        let mut given = given();
        given.limits.pages = 1;
        let mut machine = new(&given, &[1, 9]);
        assert_eq!(go(&mut machine, Event::Begin), page(1));
        assert_eq!(
            go(
                &mut machine,
                rows(&[row(12346, Wall::EPOCH, false), row(3, Wall::EPOCH, false), row(12345, Wall::EPOCH, false)])
            ),
            send(Request::Uninstall { id: 3 })
        );
        assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), send(Request::Uninstall { id: 12345 }));
        assert_eq!(go(&mut machine, Event::Reply(reply)), expected);
        silent(&mut machine);
    }
}

#[test]
fn bad_duplicate_or_excess_rows_stop_before_any_mutation() {
    for bad in [
        rows(&[row(0, NOW, false)]),
        rows(&[row(3, Wall::EPOCH, false)]), // Duplicate across pages.
        rows(&[row(7, NOW, false), row(7, NOW, false)]),
        rows(&[row(7, NOW, false), Row::Branch(b"main".as_slice().into())]),
        Event::Reply(Reply::GithubDocument(Document::Installation(installation(7, NOW, false)))),
        Event::Reply(Reply::GithubDocument(Document::Done)),
        Event::Reply(Reply::Rows(Box::new([]))),
        Event::Reply(Reply::Document(skein_forge::forgejo::document::Document::RepoSearch { ok: true })),
        Event::Reply(Reply::Empty),
        Event::Reply(Reply::GithubRows(full_page(PAGE_CAP.checked_add(1).expect("one past cap")))),
    ] {
        let mut machine = begun(&[1, 9]);
        assert_eq!(go(&mut machine, rows(&[row(3, Wall::EPOCH, false)])), page(2));
        assert_eq!(go(&mut machine, bad), listing_failure(Why::Unexpected));
        silent(&mut machine);
    }
}
fn full_page(count: u32) -> Box<[Row]> {
    let mut rows = List::with_capacity(count);
    for index in 0..count {
        let id = u64::from(index).checked_add(100).expect("small fixture id");
        rows.push(row(id, NOW, false)).expect("page fixture");
    }
    rows.into_boxed()
}

#[test]
fn a_page_at_the_row_cap_is_followed() {
    let mut machine = begun(&[]);
    assert_eq!(go(&mut machine, Event::Reply(Reply::GithubRows(full_page(PAGE_CAP)))), page(2));
    assert_eq!(go(&mut machine, rows(&[])), Progress::Complete);
    silent(&mut machine);
}

#[test]
fn a_sweep_names_the_installation_whose_delete_failed() {
    for (reply, why) in [
        (Reply::Status(200), Why::Refused(200)),
        (Reply::Status(401), Why::Refused(401)),
        (Reply::Status(403), Why::Refused(403)),
        (Reply::Status(500), Why::Refused(500)),
        (Reply::Down, Why::Down),
        (Reply::GithubDocument(Document::Done), Why::Unexpected),
        (Reply::GithubRows(Box::new([])), Why::Unexpected),
        (Reply::Rows(Box::new([])), Why::Unexpected),
    ] {
        let mut machine = begun(&[]);
        go(&mut machine, rows(&[row(3, Wall::EPOCH, false), row(12345, Wall::EPOCH, false)]));
        go(&mut machine, rows(&[]));
        assert_eq!(go(&mut machine, Event::Reply(Reply::Empty)), send(Request::Uninstall { id: 12345 }));
        assert_eq!(go(&mut machine, Event::Reply(reply)), failure(Stage::Installation, b"12345", why));
        silent(&mut machine);
    }
}

#[test]
fn a_sweep_names_list_failures_and_an_unsigned_key() {
    for (reply, expected) in [
        (Reply::Status(404), listing_failure(Why::Refused(404))),
        (Reply::Status(401), listing_failure(Why::Refused(401))),
        (Reply::Down, listing_failure(Why::Down)),
        (Reply::Unsigned, failure(Stage::App, b"key", Why::KeyForm)),
    ] {
        let mut machine = at(1);
        assert_eq!(go(&mut machine, Event::Reply(reply)), expected);
        silent(&mut machine);
    }
    let mut machine = at(4);
    assert_eq!(go(&mut machine, Event::Reply(Reply::Unsigned)), failure(Stage::App, b"key", Why::KeyForm));
    silent(&mut machine);
}

#[test]
fn a_sweep_rejects_an_unasked_event_and_ends_once() {
    let bad_begin = [Event::Read(None), rows(&[]), Event::Written, Event::Drawn(secret(KEY))];
    let bad_reply = [Event::Begin, Event::Read(None), Event::Written, Event::Drawn(secret(KEY))];
    let bad_written = [Event::Begin, Event::Read(None), rows(&[]), Event::Drawn(secret(KEY))];
    for (phase, events, expected) in [
        (0, bad_begin, listing_failure(Why::Unexpected)),
        (1, bad_reply.clone(), listing_failure(Why::Unexpected)),
        (2, bad_reply.clone(), listing_failure(Why::Unexpected)),
        (3, bad_written, listing_failure(Why::Unexpected)),
        (4, bad_reply, failure(Stage::Installation, b"3", Why::Unexpected)),
    ] {
        for event in events {
            let mut machine = at(phase);
            assert_eq!(go(&mut machine, event), expected);
            silent(&mut machine);
        }
    }
}

#[test]
fn an_overflowing_sweep_capacity_stops_before_sending() {
    let mut given = given();
    given.limits.pages = u32::MAX;
    let mut machine = new(&given, &[]);
    assert_eq!(go(&mut machine, Event::Begin), listing_failure(Why::Unexpected));
    assert_eq!(installations::worst_case(&given.limits), None);
    assert!(installations::worst_case(&fixtures::limits()).is_some());
    silent(&mut machine);
}

// The parent retains its Lacking result while forwarding the child's needs.
struct Parent {
    sweep: Sweep,
    outcome: Option<Outcome>,
}
fn parent_step(parent: &mut Parent, event: Event, out: &mut Queue<Need>) {
    let mut child = Queue::uncapped();
    installations::step(&mut parent.sweep, event, &mut child);
    match child.pop() {
        Some(Progress::Pending(need)) => out.push(need),
        Some(Progress::Complete) => out.push(Need::Done(parent.outcome.take().expect("parent continues once"))),
        None => (),
    }
}

#[test]
fn the_parent_continues_only_after_the_sweep_completes() {
    let expected = Outcome::Lacking { lacking: checks::lacks(&fixtures::app()) };
    let mut parent = Parent { sweep: new(&given(), &[1, 9]), outcome: Some(expected.clone()) };
    let mut out = Queue::uncapped();
    for (event, expected) in [
        (Event::Begin, page(1)),
        (rows(&[row(3, Wall::EPOCH, false), row(1, NOW, false)]), page(2)),
        (rows(&[]), write(&[1])),
        (Event::Written, send(Request::Uninstall { id: 3 })),
    ] {
        parent_step(&mut parent, event, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(Some(Progress::Pending(out.pop().expect("forwarded need"))), Some(expected));
        assert!(parent.outcome.is_some());
        assert!(out.is_empty());
    }
    parent_step(&mut parent, Event::Reply(Reply::Empty), &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out.pop(), Some(Need::Done(expected)));
    parent_step(&mut parent, Event::Begin, &mut out);
    assert!(out.is_empty());
}

#[test]
#[expect(clippy::disallowed_macros, reason = "check secret bytes in diagnostic text and numeric Debug forms")]
fn a_sweep_failure_and_completion_contain_no_key() {
    for phase in 0..=4 {
        let mut machine = at(phase);
        let live = format!("{machine:?}");
        let stopped = go(&mut machine, Event::Read(Some(secret(KEY))));
        let ended = format!("{machine:?} {stopped:?}");
        assert!(!ended.contains("Secret"));
        assert!(!ended.contains("Set"), "ended machine drops all scratch");
        assert!(!ended.contains("Kept"));
        let need = format!("{:?}", send(Request::Installations { page: 1 }));
        let mut complete = at(4);
        let progress = go(&mut complete, Event::Reply(Reply::Empty));
        assert_eq!(progress, Progress::Complete);
        let done = format!("{complete:?} {progress:?}");
        assert!(!done.contains("Secret"));
        assert!(!done.contains("Set"));
        assert!(!done.contains("Kept"));
        for debug in [live, ended, done, need] {
            for value in [KEY, fixtures::DER, b"sentinel-token", b"sentinel-webhook", b"sentinel-pasted-key"] {
                assert!(bytes::find(debug.as_bytes(), value).is_none());
                assert!(!debug.contains(&format!("{value:?}")));
                assert!(!debug.contains(&format!("{value:x?}")));
                assert!(!debug.contains(&format!("{value:X?}")));
            }
        }
        silent(&mut machine);
        silent(&mut complete);
    }
}
