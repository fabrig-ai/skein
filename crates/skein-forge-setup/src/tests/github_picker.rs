use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::{Auth, Call},
    github::given::{Given, Place, Refused},
    item::{self, Item},
    reachable::{self, Reachable, Reached},
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::github::{
    document::{Document, Minted, Reachable as Repository, Row},
    request::{Request, Scope},
};
use skein_lib::{List, Queue, Secret, Wall};

const INSTALLATION: u64 = 7;
const APP: u64 = 19;
const KEY: &[u8] = b"hunter2-app-key";
const TOKEN: &[u8] = b"hunter2-token";
const ACCEPTED: usize = 1;
const ID: usize = 2;
const KEY_READ: usize = 3;
const MINT: usize = 4;
const PAGE: usize = 5;

fn queue() -> Queue<Need> {
    Queue::uncapped()
}
fn one(out: &mut Queue<Need>) -> Need {
    assert_eq!(out.len(), 1);
    out.pop().expect("one need")
}
fn go(machine: &mut Reachable, event: Event) -> Need {
    let mut out = queue();
    reachable::step(machine, event, &mut out);
    one(&mut out)
}
fn id_bytes(id: u64) -> Secret {
    secret(&id.to_le_bytes())
}
fn key() -> Secret {
    secret(KEY)
}
fn minted() -> Event {
    Event::Reply(Reply::GithubDocument(Document::Minted(Minted {
        permissions: skein_forge::github::document::Levels::NONE,
        token: secret(TOKEN),
        expires: Wall::from_nanos(100),
    })))
}
fn at_given(phase: usize, given: &Given, page: u32) -> Reachable {
    let mut machine = Reachable::github(given, INSTALLATION, page).expect("given admitted");
    for event in [
        Event::Begin,
        Event::Read(Some(item::accepted_item(&[INSTALLATION, 11]))),
        Event::Read(Some(id_bytes(APP))),
        Event::Read(Some(key())),
        minted(),
    ]
    .into_iter()
    .take(phase)
    {
        go(&mut machine, event);
    }
    machine
}
fn at(phase: usize) -> Reachable {
    at_given(phase, &fixtures::given(), 1)
}
fn send_page(page: u32, per_page: u32) -> Need {
    Need::Send { request: Call::GitHub(Request::Reachable { page, per_page }), auth: Auth::Bearer(secret(TOKEN)) }
}
fn failure(stage: Stage, subject: &[u8], why: Why) -> Need {
    Need::Stop(Failure { forge: fixtures::given().forge, step: stage, subject: subject.into(), why })
}
fn installation_failure(why: Why) -> Need {
    failure(Stage::Installation, b"7", why)
}
fn row(id: u64, owner: u64, login: &[u8], name: &[u8], private: bool) -> Row {
    Row::Reachable(Repository { id, owner, login: login.into(), name: name.into(), private })
}
fn rows(count: u32) -> Box<[Row]> {
    let mut rows = List::with_capacity(count);
    for id in 1..=count {
        rows.push(row(u64::from(id), 13, b"ada", b"work", false)).expect("fixture rows");
    }
    rows.into_boxed()
}
fn page(count: u32) -> Event {
    Event::Reply(Reply::GithubRows(rows(count)))
}
fn outcome(count: u32, more: bool) -> Need {
    let mut rows = List::with_capacity(count);
    for id in 1..=count {
        rows.push(Reached {
            id: u64::from(id),
            owner_id: 13,
            owner: b"ada".as_slice().into(),
            name: b"work".as_slice().into(),
            private: false,
        })
        .expect("fixture rows");
    }
    Need::Done(Outcome::Reachable { rows: rows.into_boxed(), more })
}

#[test]
fn the_picker_reads_accepted_then_id_then_key_then_mints_then_lists() {
    let mut machine = at(0);
    assert_eq!(go(&mut machine, Event::Begin), Need::Read(Item::Accepted));
    assert_eq!(go(&mut machine, Event::Read(Some(item::accepted_item(&[7, 11])))), Need::Read(Item::AppId));
    assert_eq!(go(&mut machine, Event::Read(Some(id_bytes(APP)))), Need::Read(Item::AppKey));
    assert_eq!(
        go(&mut machine, Event::Read(Some(key()))),
        Need::Send {
            request: Call::GitHub(Request::Mint { installation: INSTALLATION, scope: Scope::Full }),
            auth: Auth::App { id: APP, key: key() },
        }
    );
    assert_eq!(go(&mut machine, minted()), send_page(1, 4));
    assert_eq!(go(&mut machine, page(1)), outcome(1, false));
}
#[test]
fn the_page_size_is_the_least_of_the_limit_and_the_pages_cap() {
    for (limit, per_page) in [(4, 4), (100, 100), (500, 100)] {
        let mut given = fixtures::given();
        given.limits.reachable = limit;
        let mut machine = at_given(MINT, &given, 1);
        assert_eq!(go(&mut machine, minted()), send_page(1, per_page));
        assert_eq!(go(&mut machine, page(per_page)), outcome(per_page, true));
    }
}
#[test]
fn the_page_asked_for_is_the_page_sent() {
    for page in [3, u32::MAX] {
        assert_eq!(go(&mut at_given(MINT, &fixtures::given(), page), minted()), send_page(page, 4));
    }
}
#[test]
fn a_full_page_says_there_is_more() {
    assert_eq!(go(&mut at(PAGE), page(4)), outcome(4, true));
}
#[test]
fn a_short_page_says_there_is_no_more() {
    assert_eq!(go(&mut at(PAGE), page(3)), outcome(3, false));
}
#[test]
fn an_empty_page_says_there_is_no_more_and_holds_no_row() {
    assert_eq!(go(&mut at(PAGE), page(0)), outcome(0, false));
}
#[test]
fn each_row_carries_its_id_owner_name_and_privacy() {
    assert_eq!(
        go(
            &mut at(PAGE),
            Event::Reply(Reply::GithubRows(Box::new([
                row(9, 13, b"grace", b"private", true),
                row(2, 5, b"ada", b"public", false),
            ])))
        ),
        Need::Done(Outcome::Reachable {
            rows: Box::new([
                Reached {
                    id: 9,
                    owner_id: 13,
                    owner: b"grace".as_slice().into(),
                    name: b"private".as_slice().into(),
                    private: true
                },
                Reached {
                    id: 2,
                    owner_id: 5,
                    owner: b"ada".as_slice().into(),
                    name: b"public".as_slice().into(),
                    private: false
                },
            ]),
            more: false
        })
    );
}
#[test]
fn an_installation_not_accepted_stops_missing_and_reads_no_more() {
    for read in [None, Some(secret(b"")), Some(item::accepted_item(&[8, 11]))] {
        let mut machine = at(ACCEPTED);
        assert_eq!(go(&mut machine, Event::Read(read)), installation_failure(Why::Missing));
        let mut out = queue();
        reachable::step(&mut machine, Event::Read(Some(id_bytes(APP))), &mut out);
        assert!(out.is_empty());
    }
    for (id, subject) in [(0, b"0".as_slice()), (u64::MAX, b"18446744073709551615")] {
        let mut machine = Reachable::github(&fixtures::given(), id, 1).expect("given admitted");
        go(&mut machine, Event::Begin);
        assert_eq!(go(&mut machine, Event::Read(None)), failure(Stage::Installation, subject, Why::Missing));
    }
}
#[test]
fn a_missing_app_id_or_key_stops_missing_naming_it() {
    for (phase, subject) in [(ID, b"id".as_slice()), (KEY_READ, b"key")] {
        for read in [None, Some(secret(b""))] {
            assert_eq!(go(&mut at(phase), Event::Read(read)), failure(Stage::App, subject, Why::Missing));
        }
    }
    for id in [id_bytes(0), secret(b"invalid-id")] {
        assert_eq!(go(&mut at(ID), Event::Read(Some(id))), failure(Stage::App, b"id", Why::Missing));
    }
}
#[test]
fn an_installation_gone_at_the_mint_stops_missing() {
    assert_eq!(go(&mut at(MINT), Event::Reply(Reply::Status(404))), installation_failure(Why::Missing));
}
#[test]
fn a_refused_mint_names_the_installation_and_the_status() {
    for status in [403, 401] {
        assert_eq!(go(&mut at(MINT), Event::Reply(Reply::Status(status))), installation_failure(Why::Refused(status)));
    }
}
#[test]
fn a_key_that_could_not_sign_stops_key_form_naming_the_key() {
    assert_eq!(go(&mut at(MINT), Event::Reply(Reply::Unsigned)), failure(Stage::App, b"key", Why::KeyForm));
}
#[test]
fn a_refused_or_lost_page_names_the_installation() {
    for (reply, why) in
        [(Reply::Status(403), Why::Refused(403)), (Reply::Status(404), Why::Refused(404)), (Reply::Down, Why::Down)]
    {
        assert_eq!(go(&mut at(PAGE), Event::Reply(reply)), installation_failure(why));
    }
}
#[test]
fn a_forge_that_stops_answering_stops_the_picker_at_the_mint() {
    assert_eq!(go(&mut at(MINT), Event::Reply(Reply::Down)), installation_failure(Why::Down));
}
#[test]
fn a_page_of_more_rows_than_asked_is_unexpected() {
    assert_eq!(go(&mut at(PAGE), page(5)), installation_failure(Why::Unexpected));
}
#[test]
fn a_row_of_another_kind_is_unexpected() {
    assert_eq!(
        go(
            &mut at(PAGE),
            Event::Reply(Reply::GithubRows(Box::new([
                row(9, 13, b"ada", b"work", false),
                Row::Branch(b"main".as_slice().into()),
            ])))
        ),
        installation_failure(Why::Unexpected)
    );
}
#[test]
fn a_reply_of_another_kind_is_unexpected_at_each_request() {
    for phase in [MINT, PAGE] {
        for reply in [
            Reply::Empty,
            Reply::Document(skein_forge::forgejo::document::Document::RepoSearch { ok: true }),
            Reply::Rows(Box::new([])),
            Reply::GithubDocument(Document::Done),
        ] {
            assert_eq!(go(&mut at(phase), Event::Reply(reply)), installation_failure(Why::Unexpected));
        }
    }
    assert_eq!(go(&mut at(MINT), page(0)), installation_failure(Why::Unexpected));
    assert_eq!(go(&mut at(PAGE), minted()), installation_failure(Why::Unexpected));
    assert_eq!(go(&mut at(PAGE), Event::Reply(Reply::Unsigned)), installation_failure(Why::Unexpected));
}
#[test]
fn an_out_of_order_event_stops_each_github_phase() {
    for phase in 0..=PAGE {
        for event in [Event::Begin, Event::Read(None), Event::Reply(Reply::Empty), Event::Written, Event::Drawn(key())]
        {
            if (phase == 0 && event == Event::Begin)
                || ((ACCEPTED..=KEY_READ).contains(&phase) && event == Event::Read(None))
                || (phase >= MINT && event == Event::Reply(Reply::Empty))
            {
                continue;
            }
            let (stage, subject) = match phase {
                ID => (Stage::App, b"id".as_slice()),
                KEY_READ => (Stage::App, b"key".as_slice()),
                0 | ACCEPTED | MINT | PAGE => (Stage::Installation, b"7".as_slice()),
                _ => unreachable!("GitHub phases"),
            };
            assert_eq!(go(&mut at(phase), event), failure(stage, subject, Why::Unexpected), "phase {phase}");
        }
    }
}
#[test]
fn an_ended_github_picker_ignores_every_event() {
    let mut done = at(PAGE);
    go(&mut done, page(0));
    let mut stopped = at(MINT);
    go(&mut stopped, Event::Reply(Reply::Down));
    for mut machine in [done, stopped] {
        for event in
            [Event::Begin, Event::Read(None), Event::Read(Some(key())), minted(), Event::Written, Event::Drawn(key())]
        {
            let mut out = queue();
            reachable::step(&mut machine, event, &mut out);
            assert!(out.is_empty());
        }
    }
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the test inspects machine Debug for retained secrets")]
fn the_key_is_dropped_when_the_mint_answers_and_the_token_is_never_kept() {
    let mut machine = at(MINT);
    assert!(format!("{machine:?}").contains("Secret"));
    go(&mut machine, minted());
    assert!(!format!("{machine:?}").contains("Secret"));
    go(&mut machine, page(0));
    assert!(!format!("{machine:?}").contains("Secret"));
    for phase in [MINT, PAGE] {
        let mut stopped = at(phase);
        go(&mut stopped, Event::Reply(Reply::Down));
        assert!(!format!("{stopped:?}").contains("Secret"));
    }
}
#[test]
#[expect(
    clippy::disallowed_macros,
    reason = "the redaction test inspects failures, outcomes and machine Debug as text and hex"
)]
fn no_secret_is_in_a_github_pickers_failure_or_outcome_debug() {
    for phase in 0..=PAGE {
        let mut machine = at(phase);
        let stop = go(&mut machine, Event::Written);
        let debug = format!("{stop:?} {machine:?}");
        for value in [KEY, TOKEN] {
            assert!(skein_lib::bytes::find(debug.as_bytes(), value).is_none());
            assert!(!debug.contains(&format!("{value:?}")));
            assert!(!debug.contains(&format!("{value:x?}")));
            assert!(!debug.contains(&format!("{value:X?}")));
        }
        assert!(!debug.contains("Secret"));
    }
    let mut machine = at(PAGE);
    let done = go(&mut machine, page(1));
    let debug = format!("{done:?} {machine:?}");
    for value in [KEY, TOKEN] {
        assert!(skein_lib::bytes::find(debug.as_bytes(), value).is_none());
        assert!(!debug.contains(&format!("{value:?}")));
        assert!(!debug.contains(&format!("{value:x?}")));
        assert!(!debug.contains(&format!("{value:X?}")));
    }
    assert!(!debug.contains("Secret"));
}
#[test]
fn github_picker_refuses_page_zero_naming_its_place() {
    assert_eq!(
        Reachable::github(&fixtures::given(), INSTALLATION, 0).expect_err("zero page"),
        Refused { place: Place::Page, other: None }
    );
}
#[test]
fn github_picker_refuses_a_given_that_is_refused() {
    let mut given = fixtures::given();
    given.limits.reachable = 0;
    for page in [0, 1] {
        assert_eq!(
            Reachable::github(&given, INSTALLATION, page).expect_err("zero reachable"),
            Refused { place: Place::Reachable, other: None }
        );
    }
}
