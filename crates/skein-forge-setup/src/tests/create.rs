use super::fixtures;
use crate::{
    create::{self, Create},
    item::Item,
    reconcile::{Event, Need, Outcome, Reply, Stage, Why},
};
use skein_forge::forgejo::{
    document::{Document, Org},
    request::{Repo, Request},
};
use skein_lib::Queue;
fn machine() -> Create {
    Create::new(&fixtures::given(), b"first".as_slice().into(), b"starter".as_slice().into(), Box::new([]))
        .expect("fixture")
}
fn go(machine: &mut Create, event: Event) -> Need {
    let mut out = Queue::uncapped();
    create::step(machine, event, &mut out);
    out.pop().expect("one need")
}
fn repository_read() -> Request {
    Request::Repository { at: Repo { owner: b"first".as_slice().into(), name: b"starter".as_slice().into() } }
}
fn at_org(machine: &mut Create) {
    go(machine, Event::Begin);
    go(machine, Event::Read(Some(fixtures::secret(fixtures::ADMIN))));
    let mut admin = fixtures::person(b"ada", b"Ada");
    admin.is_admin = true;
    go(machine, Event::Reply(Reply::Document(Document::User(admin))));
    go(machine, Event::Reply(Reply::Document(Document::Org(Org { id: 1, name: b"first".as_slice().into() }))));
}
#[test]
fn a_repository_creation_race_reads_the_winning_repository_again() {
    let mut machine = machine();
    at_org(&mut machine);
    go(&mut machine, Event::Reply(Reply::Status(404)));
    let need = go(&mut machine, Event::Reply(Reply::Status(409)));
    assert_eq!(
        need,
        Need::Send {
            request: repository_read().into(),
            auth: crate::call::Auth::Token(fixtures::secret(fixtures::ADMIN))
        }
    );
}
#[test]
fn an_empty_file_list_finishes_after_access_with_the_repository_id() {
    let mut machine = machine();
    at_org(&mut machine);
    go(&mut machine, fixtures::repository(1, b"first", b"starter"));
    let permission = skein_forge::forgejo::document::CollaboratorPermission {
        permission: skein_forge::forgejo::document::Permission::Write,
        user: fixtures::person(b"kit", b"kit:bot"),
    };
    assert_eq!(
        go(&mut machine, Event::Reply(Reply::Document(Document::Permission(permission)))),
        Need::Done(Outcome::Created { id: core::num::NonZeroU64::new(1).expect("id") })
    );
}
#[test]
fn creation_halt_reports_the_outstanding_org_and_then_ignores_events() {
    let mut machine = machine();
    go(&mut machine, Event::Begin);
    go(&mut machine, Event::Read(Some(fixtures::secret(fixtures::ADMIN))));
    let mut admin = fixtures::person(b"ada", b"Ada");
    admin.is_admin = true;
    go(&mut machine, Event::Reply(Reply::Document(Document::User(admin))));
    let mut out = Queue::uncapped();
    create::halt(&mut machine, Why::Down, &mut out);
    assert_eq!(
        out.pop(),
        Some(Need::Stop(crate::reconcile::Failure {
            forge: 1,
            step: Stage::Org,
            subject: b"first".as_slice().into(),
            why: Why::Down
        }))
    );
    create::step(&mut machine, Event::Read(None), &mut out);
    create::halt(&mut machine, Why::Missing, &mut out);
    assert!(out.is_empty());
}
#[test]
fn creation_first_asks_for_the_admin_token() {
    assert_eq!(go(&mut machine(), Event::Begin), Need::Read(Item::AdminToken));
}
