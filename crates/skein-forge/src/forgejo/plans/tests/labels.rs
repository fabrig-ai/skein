use crate::api::{Answer, Error, Op, Write};
use crate::forgejo::{
    plans::plan::Step,
    tests::fixtures::{self, BOUNDS, Run, fields},
};

const CATALOG: &[u8] = br#"[{"id":7,"name":"finding"},{"id":9,"name":"severity:high"}]"#;
const ISSUE: &[u8] = br#"{"number":1,"state":"open","user":{"id":2,"login":"writer"},"labels":[{"id":7,"name":"finding"},{"id":9,"name":"severity:high"}],"updated_at":"1970-01-01T00:00:00Z","title":"t","body":"b"}"#;
fn names() -> Box<[Box<[u8]>]> {
    Box::new([Box::from(b"finding".as_slice()), Box::from(b"severity:high".as_slice())])
}
fn creation() -> Write {
    Write::CreateLabel { name: Box::from(b"finding".as_slice()), color: [0x12, 0xab, 0xef] }
}
fn begun(write: Write) -> Run {
    // The created issue carries the two labels named by this fixture.
    const LABELS: u32 = 2;
    let mut run = Run::with_limits(
        crate::forgejo::limits::Limits { inner: LABELS, ..fixtures::limits() },
        BOUNDS,
        Op::Write(write),
    );
    run.tables().keep_repository(fixtures::REPO, fixtures::specimen());
    run.begin();
    run
}
#[test]
fn an_issue_is_created_with_its_labels() {
    let mut run = begun(Write::CreateIssue {
        key: Box::from(b"k".as_slice()),
        title: Box::from(b"t".as_slice()),
        body: Box::from(b"b".as_slice()),
        labels: names(),
    });
    run.answer(200, CATALOG);
    let result = run.answer(201, ISSUE);
    assert_eq!(result, Step::Done(Ok(Answer::Created(1))));
    run.assert_sent(&[
        "GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8",
        "POST /api/v1/repos/fixture/specimen/issues",
    ]);
    run.assert_bodies(&[None, Some(br#"{"title":"t","body":"<!-- kit:key 6b -->\nb","labels":[7,9]}"#)]);
}
#[test]
fn an_issues_labels_are_replaced_by_name() {
    let mut run = begun(Write::Labels { number: 1, labels: names() });
    run.answer(200, CATALOG);
    assert_eq!(run.answer(200, CATALOG), Step::Done(Ok(Answer::Done)));
    run.assert_sent(&[
        "GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8",
        "PUT /api/v1/repos/fixture/specimen/issues/1/labels",
    ]);
    run.assert_bodies(&[None, Some(br#"{"labels":[7,9]}"#)]);
}
#[test]
fn an_empty_label_set_clears_the_issue() {
    let mut run = begun(Write::Labels { number: 1, labels: Box::new([]) });
    run.answer(200, CATALOG);
    assert_eq!(run.answer(200, b"[]"), Step::Done(Ok(Answer::Done)));
    run.assert_bodies(&[None, Some(br#"{"labels":[]}"#)]);
}
#[test]
fn a_label_that_exists_is_found_not_made_again() {
    let mut run = begun(creation());
    assert_eq!(run.answer(200, CATALOG), Step::Done(Err(Error::Exists)));
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8"]);
}
#[test]
fn a_missing_label_is_created_with_its_color() {
    let mut run = begun(creation());
    run.answer(200, b"[]");
    assert_eq!(run.answer(201, br#"{"id":7,"name":"finding","color":"12abef"}"#), Step::Done(Ok(Answer::Done)));
    run.assert_sent(&[
        "GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8",
        "POST /api/v1/repos/fixture/specimen/labels",
    ]);
    run.assert_bodies(&[None, Some(br##"{"name":"finding","color":"#12abef"}"##)]);
}
#[test]
fn an_unknown_label_is_missing_before_any_write() {
    for write in [
        Write::Labels { number: 1, labels: names() },
        Write::CreateIssue {
            key: Box::from(b"k".as_slice()),
            title: Box::from(b"t".as_slice()),
            body: Box::from(b"b".as_slice()),
            labels: names(),
        },
    ] {
        let mut run = begun(write);
        assert_eq!(run.answer(200, b"[]"), Step::Done(Err(Error::Missing)));
        run.assert_sent(&["GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8"]);
    }
}
#[test]
fn a_catalog_beyond_one_page_refuses_before_writing() {
    let mut run = begun(creation());
    let response =
        crate::forgejo::plans::Fields { total_count: Some(u64::from(BOUNDS.rows).checked_add(1).unwrap()), ..fields() };
    assert_eq!(run.answer_with(200, CATALOG, response), Step::Done(Err(Error::TooLarge)));
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8"]);
}
#[test]
fn a_label_conflict_is_exists() {
    let mut run = begun(creation());
    run.answer(200, b"[]");
    assert_eq!(run.answer(409, br#"{"message":"exists"}"#), Step::Done(Err(Error::Exists)));
}

#[test]
fn a_catalog_at_the_row_bound_is_complete() {
    // The canned catalog repeats the same label for each of the fixture's eight rows.
    let mut rows = skein_lib::List::with_capacity(BOUNDS.rows);
    for _ in 0..BOUNDS.rows {
        rows.push(crate::forgejo::document::Label { id: 7, name: Box::from(b"finding".as_slice()) }).unwrap();
    }
    let catalog =
        crate::forgejo::server::encode(&crate::forgejo::server::Answer::Labels(rows.into_boxed()), &fixtures::limits())
            .unwrap();
    let mut run = begun(creation());
    let response = crate::forgejo::plans::Fields { total_count: Some(u64::from(BOUNDS.rows)), ..fields() };
    assert_eq!(run.answer_with(200, &catalog, response), Step::Done(Err(Error::Exists)));
}

#[test]
fn a_catalog_conflict_does_not_claim_a_label_exists() {
    let mut run = begun(creation());
    assert_eq!(run.answer(409, br#"{"message":"conflict"}"#), Step::Done(Err(Error::Unavailable)));
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8"]);
}
#[test]
fn a_catalog_without_ids_fails_before_any_write() {
    let mut run = begun(creation());
    assert_eq!(run.answer(200, br#"[{"name":"finding"}]"#), Step::Done(Err(Error::Unavailable)));
    run.assert_sent(&["GET /api/v1/repos/fixture/specimen/labels?page=1&limit=8"]);
}
