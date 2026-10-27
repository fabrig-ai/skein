//! Admission and answer fit for Actions and trees (forge-client.md, 2.3).
use super::fixtures::{Harness, outbox as writing};
use super::{LIMITS, REPO, at, terminal};
use crate::{Condition, Event, Outcome};
use skein_forge::api::{self, Answer, Error, Read, Write};
use skein_lib::Token;
fn dispatch() -> Write {
    Write::Dispatch { workflow: Box::from(*b"ci"), branch: Box::from(*b"work"), inputs: Box::new([]) }
}
pub(super) fn run() -> api::Run {
    api::Run {
        id: 7,
        number: 3,
        workflow: Box::from(*b"ci"),
        commit: [1; 32],
        event: api::Event::Dispatch,
        actor: 1,
        state: api::RunState::Waiting,
        conclusion: None,
        url: Box::new([]),
        created: at(100),
        jobs: Box::new([]),
    }
}
#[test]
fn turn_writes_refuse_invalid_fields_at_admission() {
    let mut excess = skein_lib::List::with_capacity(11);
    for _ in 0..=api::INPUTS {
        excess.push(api::Input { name: Box::from(*b"k"), value: Box::new([]) }).expect("fixture input bound");
    }
    for write in [
        Write::Dispatch { workflow: Box::from(*b"ci"), branch: Box::from(*b"work"), inputs: excess.into_boxed() },
        Write::Dispatch {
            workflow: Box::from(*b"ci"),
            branch: Box::from(*b"work"),
            inputs: Box::new([api::Input { name: Box::new([]), value: Box::new([]) }]),
        },
        Write::Dispatch { workflow: Box::new([]), branch: Box::from(*b"work"), inputs: Box::new([]) },
        Write::Dispatch { workflow: Box::from(*b"ci"), branch: Box::new([]), inputs: Box::new([]) },
        Write::Status {
            commit: [1; 32],
            context: Box::from(*b"ci"),
            check: api::Check::Passed,
            description: skein_lib::bytes::zeroed(api::DESCRIPTION_BYTES + 1),
            link: Box::new([]),
        },
    ] {
        let mut h = writing::Harness::new();
        h.make(1, write, Condition::None);
        assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Failed(Error::TooLarge))]);
        assert!(h.send().is_none());
    }
    assert!(crate::bounds::op(&api::Op::Write(dispatch()), &LIMITS));
    let status = Write::Status {
        commit: [1; 32],
        context: Box::new([]),
        check: api::Check::Passed,
        description: skein_lib::bytes::zeroed(api::DESCRIPTION_BYTES),
        link: Box::new([]),
    };
    assert!(crate::bounds::op(&api::Op::Write(status), &LIMITS));
    let mut ten = skein_lib::List::with_capacity(10);
    for _ in 0..api::INPUTS {
        ten.push(api::Input { name: Box::from(*b"k"), value: Box::new([]) }).expect("ten inputs");
    }
    let write = Write::Dispatch { workflow: Box::from(*b"ci"), branch: Box::from(*b"work"), inputs: ten.into_boxed() };
    assert!(crate::bounds::op(&api::Op::Write(write), &crate::Limits { op_bytes: 1024, ..LIMITS }));
}
#[test]
fn turn_pages_start_at_one() {
    for read in
        [Read::Runs { workflow: Box::from(*b"ci"), commit: [1; 32], page: 0 }, Read::Tree { object: [1; 32], page: 0 }]
    {
        super::fixtures::recovery::refused_read(read);
    }
}
#[test]
fn turn_answers_must_fit_the_call() {
    let listing = Read::Runs { workflow: Box::from(*b"ci"), commit: [1; 32], page: 1 };
    let mut wrong_commit = run();
    wrong_commit.commit = [2; 32];
    let mut wrong_workflow = run();
    wrong_workflow.workflow = Box::from(*b"other");
    let entry = api::Entry { name: Box::from(*b"a"), mode: 0o100_644, kind: api::EntryKind::Blob, object: [1; 32] };
    let mut later = entry.clone();
    later.name = Box::from(*b"b");
    for (read, answer) in [
        (listing.clone(), Answer::Runs { runs: Box::new([wrong_commit]), more: false }),
        (listing.clone(), Answer::Runs { runs: Box::new([wrong_workflow]), more: false }),
        (listing.clone(), Answer::Runs { runs: Box::new([run(), run()]), more: false }),
        (Read::Run { run: 8 }, Answer::Run(run())),
        (
            Read::Tree { object: [1; 32], page: 1 },
            Answer::Tree { entries: Box::new([later, entry.clone()]), more: false },
        ),
        (
            Read::Tree { object: [1; 32], page: 1 },
            Answer::Tree { entries: Box::new([entry.clone(), entry]), more: false },
        ),
    ] {
        let mut h = Harness::new(LIMITS);
        assert!(h.step(Event::Read { owner: Token::new(1), repository: REPO, read }).is_empty());
        let sent = h.one();
        terminal(&h.answer(&sent, 1, Ok(answer)), 1, Error::InvalidAnswer);
    }
    assert!(crate::api::accepts(&api::Op::Read(listing), &Answer::Runs { runs: Box::new([run()]), more: false }));
}

fn git_entries() -> [api::Entry; 2] {
    [
        api::Entry { name: Box::from(*b"tests.rs"), mode: 0o100_644, kind: api::EntryKind::Blob, object: [1; 32] },
        api::Entry { name: Box::from(*b"tests"), mode: 0o040_000, kind: api::EntryKind::Tree, object: [2; 32] },
    ]
}
#[test]
fn a_tree_in_git_order_fits() {
    let op = api::Op::Read(Read::Tree { object: [1; 32], page: 1 });
    assert!(crate::api::accepts(&op, &Answer::Tree { entries: Box::new(git_entries()), more: false }));
}
#[test]
fn a_tree_out_of_git_order_does_not_fit() {
    let mut entries = git_entries();
    entries.reverse();
    let mut h = Harness::new(LIMITS);
    h.step(Event::Read { owner: Token::new(1), repository: REPO, read: Read::Tree { object: [1; 32], page: 1 } });
    let sent = h.one();
    terminal(
        &h.answer(&sent, 1, Ok(Answer::Tree { entries: Box::new(entries), more: false })),
        1,
        Error::InvalidAnswer,
    );
}
