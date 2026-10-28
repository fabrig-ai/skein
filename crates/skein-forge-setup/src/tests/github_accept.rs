use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::{Auth, Call},
    github::accept::{self, Accept},
    item::{self, Item},
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use core::num::NonZeroU64;
use skein_forge::github::{
    document::{Account, Document, Installation},
    request::Request,
};
use skein_lib::{Queue, Secret, Wall, bytes};

const ID: u64 = 3;
fn machine() -> Accept {
    Accept::new(&fixtures::given(), NonZeroU64::new(ID).expect("fixture id")).expect("valid given")
}
#[expect(clippy::disallowed_macros, reason = "test checks diagnostics for key byte spellings")]
fn go(machine: &mut Accept, event: Event) -> Need {
    let mut out = Queue::uncapped();
    accept::step(machine, event, &mut out);
    assert_eq!(out.len(), 1);
    let need = out.pop().expect("one need");
    let text = format!("{machine:?} {need:?}");
    assert!(bytes::find(text.as_bytes(), fixtures::DER).is_none());
    assert!(!text.contains(&format!("{:?}", fixtures::DER)));
    need
}
fn stop(step: Stage, subject: &[u8], why: Why) -> Need {
    Need::Stop(Failure { forge: 4, step, subject: subject.into(), why })
}
fn reply(id: u64, suspended: bool) -> Event {
    Event::Reply(Reply::GithubDocument(Document::Installation(Installation {
        id,
        login: b"acme".as_slice().into(),
        account: 22,
        kind: Account::Organization,
        created: Wall::EPOCH,
        suspended,
    })))
}
fn begin(machine: &mut Accept) {
    assert_eq!(go(machine, Event::Begin), Need::Read(Item::AppId));
    assert_eq!(go(machine, Event::Read(Some(secret(&7u64.to_le_bytes())))), Need::Read(Item::AppKey));
    assert_eq!(go(machine, Event::Read(Some(secret(fixtures::DER)))), Need::Read(Item::Accepted));
}
fn pending(ids: Option<Secret>) -> Accept {
    let mut machine = machine();
    begin(&mut machine);
    assert_eq!(
        go(&mut machine, Event::Read(ids)),
        Need::Send {
            request: Call::GitHub(Request::Installation { id: ID }),
            auth: Auth::App { id: 7, key: secret(fixtures::DER) },
        }
    );
    machine
}
fn silent(machine: &mut Accept) {
    for event in [
        Event::Begin,
        Event::Read(Some(secret(fixtures::DER))),
        reply(ID, false),
        Event::Written,
        Event::Drawn(secret(fixtures::DER)),
    ] {
        let mut out = Queue::uncapped();
        accept::step(machine, event, &mut out);
        assert!(out.is_empty());
    }
}
#[test]
fn accept_checks_the_installation_before_writing_the_sorted_set() {
    for (ids, expected, suspended) in [
        (None, item::accepted_item(&[3]), false),
        (Some(item::accepted_item(&[])), item::accepted_item(&[3]), false),
        (Some(item::accepted_item(&[1, 5])), item::accepted_item(&[1, 3, 5]), true),
        (Some(item::accepted_item(&[5, 7])), item::accepted_item(&[3, 5, 7]), false),
        (Some(item::accepted_item(&[1, 2])), item::accepted_item(&[1, 2, 3]), false),
    ] {
        let mut machine = pending(ids);
        assert_eq!(go(&mut machine, reply(ID, suspended)), Need::Write { item: Item::Accepted, bytes: expected });
        assert_eq!(go(&mut machine, Event::Written), Need::Done(Outcome::Nothing));
        silent(&mut machine);
    }
}
#[test]
fn accept_waits_for_the_set_to_be_written_before_done() {
    let mut machine = pending(None);
    assert_eq!(
        go(&mut machine, reply(ID, false)),
        Need::Write { item: Item::Accepted, bytes: item::accepted_item(&[3]) }
    );
    assert_eq!(go(&mut machine, Event::Written), Need::Done(Outcome::Nothing));
    silent(&mut machine);
}
#[test]
fn an_already_accepted_id_needs_no_write_even_at_capacity() {
    let mut machine = pending(Some(item::accepted_item(&[1, 2, 3, 4, 5, 6, 7, 8])));
    assert_eq!(go(&mut machine, reply(ID, false)), Need::Done(Outcome::Nothing));
    silent(&mut machine);
}
#[test]
fn an_extra_accepted_id_past_capacity_changes_nothing() {
    let mut machine = pending(Some(item::accepted_item(&[1, 2, 4, 5, 6, 7, 8, 9])));
    assert_eq!(go(&mut machine, reply(ID, false)), stop(Stage::Installation, b"3", Why::Unexpected));
    silent(&mut machine);
}
#[test]
fn accept_names_a_missing_refused_unsigned_or_wrong_installation() {
    for (event, step, subject, why) in [
        (Event::Reply(Reply::Status(404)), Stage::Installation, b"3".as_slice(), Why::Missing),
        (Event::Reply(Reply::Status(403)), Stage::Installation, b"3", Why::Refused(403)),
        (Event::Reply(Reply::Unsigned), Stage::App, b"key", Why::KeyForm),
        (Event::Reply(Reply::Down), Stage::Installation, b"3", Why::Down),
        (reply(4, false), Stage::Installation, b"3", Why::Unexpected),
        (Event::Reply(Reply::Empty), Stage::Installation, b"3", Why::Unexpected),
        (Event::Reply(Reply::GithubDocument(Document::BotUser(3))), Stage::Installation, b"3", Why::Unexpected),
        (Event::Reply(Reply::GithubRows(Box::new([]))), Stage::Installation, b"3", Why::Unexpected),
    ] {
        let mut machine = pending(None);
        assert_eq!(go(&mut machine, event), stop(step, subject, why));
        silent(&mut machine);
    }
    for (phase, value, step, subject, why) in [
        (0_u8, None, Stage::App, b"id".as_slice(), Why::Missing),
        (0, Some(secret(b"bad")), Stage::App, b"id", Why::Missing),
        (1, None, Stage::App, b"key", Why::Missing),
        (1, Some(secret(b"bad")), Stage::App, b"key", Why::Missing),
        (2, Some(secret(b"bad")), Stage::Installation, b"3", Why::Unexpected),
    ] {
        let mut machine = machine();
        go(&mut machine, Event::Begin);
        if phase > 0 {
            go(&mut machine, Event::Read(Some(secret(&7u64.to_le_bytes()))));
        }
        if phase > 1 {
            go(&mut machine, Event::Read(Some(secret(fixtures::DER))));
        }
        assert_eq!(go(&mut machine, Event::Read(value)), stop(step, subject, why));
        silent(&mut machine);
    }
}
#[test]
fn install_and_accept_reject_unasked_events_and_ignore_ended_events() {
    super::github_install::rejects_unasked_events_and_ignores_ended_events();
    for phase in 0_u8..6 {
        for event in [
            Event::Begin,
            Event::Read(None),
            Event::Reply(Reply::Down),
            Event::Written,
            Event::Drawn(secret(fixtures::DER)),
        ] {
            if (phase == 0 && event == Event::Begin)
                || ((1..=3).contains(&phase)
                    && match event {
                        Event::Read(_) => true,
                        Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => false,
                    })
                || (phase == 4
                    && match event {
                        Event::Reply(_) => true,
                        Event::Begin | Event::Read(_) | Event::Written | Event::Drawn(_) => false,
                    })
                || (phase == 5 && event == Event::Written)
            {
                continue;
            }
            let mut machine = machine();
            if phase > 0 {
                go(&mut machine, Event::Begin);
            }
            if phase > 1 {
                go(&mut machine, Event::Read(Some(secret(&7u64.to_le_bytes()))));
            }
            if phase > 2 {
                go(&mut machine, Event::Read(Some(secret(fixtures::DER))));
            }
            if phase > 3 {
                go(&mut machine, Event::Read(None));
            }
            if phase > 4 {
                go(&mut machine, reply(ID, false));
            }
            let (step, subject) = match phase {
                0 | 1 => (Stage::App, b"id".as_slice()),
                2 => (Stage::App, b"key".as_slice()),
                _ => (Stage::Installation, b"3".as_slice()),
            };
            assert_eq!(go(&mut machine, event), stop(step, subject, Why::Unexpected));
            silent(&mut machine);
        }
    }
}
