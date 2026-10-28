type Names = Box<[Box<[u8]>]>;
use super::{LIMITS, fixtures::outbox::Harness};
use crate::{Condition, Made, Outcome, Recovery, recovery};
use skein_forge::api::{Answer, Error, Op, Write};
fn creation() -> Write {
    Write::CreateLabel { name: Box::from(b"finding".as_slice()), color: [0x12, 0xab, 0xef] }
}
#[test]
fn an_uncertain_label_is_found_by_its_name() {
    let mut h = Harness::new();
    h.make(1, creation(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    let lookup = h.one();
    assert_eq!(lookup.op, Op::Write(creation()));
    h.answer(lookup, Err(Error::Exists));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain), (1, Outcome::Made { made: Made::Set, found: true })]);
}
#[test]
fn an_uncertain_absent_label_can_be_made_again() {
    let mut h = Harness::new();
    h.make(1, creation(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Timeout));
    let lookup = h.one();
    h.answer(lookup, Ok(Answer::Done));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Uncertain), (1, Outcome::Made { made: Made::Set, found: false })]);
}
#[test]
fn a_label_that_exists_is_found_not_made_again() {
    let mut h = Harness::new();
    h.make(1, creation(), Condition::None);
    h.clock();
    let sent = h.one();
    h.answer(sent, Err(Error::Exists));
    assert_eq!(h.outcomes.as_slice(), &[(1, Outcome::Made { made: Made::Set, found: true })]);
}
#[test]
fn labels_fit_the_existing_operation_and_row_bounds() {
    // A label is a 16-byte string-list element plus its name (forge.md, 3.7).
    const LABEL_BYTES: u32 = 7;
    const OP_BYTES: u32 = 16 + LABEL_BYTES;
    const ROWS: u32 = 1;
    let limits = crate::Limits { rows: ROWS, op_bytes: OP_BYTES, ..LIMITS };
    let cases: [(&[u8], bool); 2] = [(b"finding", true), (b"finding!", false)];
    for (name, expected) in cases {
        let write = Write::Labels { number: 1, labels: Box::new([Box::from(name)]) };
        assert_eq!(crate::bounds::op(&Op::Write(write), &limits), expected);
    }
}

#[test]
fn label_lists_at_the_row_limit_fit_and_one_past_does_not() {
    const ROWS: u32 = 1;
    // Two one-byte label elements fit in 64 bytes, isolating row admission.
    const OP_BYTES: u32 = 64;
    let limits = crate::Limits { rows: ROWS, op_bytes: OP_BYTES, ..LIMITS };
    let cases: [(Names, bool); 2] = [
        (Box::new([Box::from(b"x".as_slice())]), true),
        (Box::new([Box::from(b"x".as_slice()), Box::from(b"y".as_slice())]), false),
    ];
    for (labels, expected) in cases {
        for write in [
            Write::Labels { number: 1, labels: labels.clone() },
            Write::CreateIssue { key: Box::from(b"k".as_slice()), title: Box::new([]), body: Box::new([]), labels },
        ] {
            assert_eq!(crate::bounds::op(&Op::Write(write), &limits), expected);
        }
    }
}

#[test]
fn label_creation_and_replacement_declare_their_recovery() {
    assert_eq!(recovery(&creation()), Recovery::Keyed);
    assert_eq!(recovery(&Write::Labels { number: 1, labels: Box::new([]) }), Recovery::Idempotent);
}
#[test]
fn label_writes_accept_only_the_done_answer_shape() {
    for write in [creation(), Write::Labels { number: 1, labels: Box::new([]) }] {
        assert!(crate::api::accepts(&Op::Write(write.clone()), &Answer::Done));
        assert!(!crate::api::accepts(&Op::Write(write), &Answer::Created(1)));
    }
}
