use super::fixtures::{self, Harness, boxed};
use skein_fake_forge_domain::api::{self as a, Op, Read, Write};
use skein_forge::forgejo::{
    document::{Document, Row},
    request::{Page, Request},
};
// The two label definitions are the first and second ids in the isolated repository.
const FINDING: u64 = 1;
const SEVERITY: u64 = 2;
const ISSUE: u64 = 1;
fn labeled() -> Harness {
    let mut h = Harness::new(fixtures::config());
    for name in [b"finding".as_slice(), b"severity:high"] {
        h.call(fixtures::PERSON, Op::Write(Write::DefineLabel { name: boxed(name) })).expect("fixture label");
    }
    h
}
#[test]
fn label_catalog_http_retains_stable_ids() {
    let mut h = labeled();
    let request = Request::Labels { at: fixtures::at(), page: Page { page: Some(1), limit: Some(2) } };
    h.send(&request);
    assert_eq!(h.status(), 200);
    assert_eq!(
        &*h.decoded(&request).rows,
        &[
            Row::Label { id: FINDING, name: boxed(b"finding") },
            Row::Label { id: SEVERITY, name: boxed(b"severity:high") }
        ]
    );
}
#[test]
fn labeled_issue_http_translates_ids_to_domain_names() {
    let mut h = labeled();
    let request = Request::CreateIssue {
        at: fixtures::at(),
        title: boxed(b"t"),
        body: boxed(b"b"),
        labels: Box::new([FINDING, SEVERITY]),
    };
    h.send(&request);
    let Document::Issue(issue) = h.decoded(&request).document else { panic!("issue response") };
    assert_eq!(issue.labels.as_ref(), &[boxed(b"finding"), boxed(b"severity:high")]);
}
#[test]
fn issue_labels_http_replaces_the_whole_set() {
    let mut h = labeled();
    h.call(
        fixtures::PERSON,
        Op::Write(Write::CreateIssue {
            title: boxed(b"t"),
            body: boxed(b"b"),
            labels: Box::new([boxed(b"finding"), boxed(b"severity:high")]),
        }),
    )
    .expect("fixture issue");
    let request = Request::SetLabels { at: fixtures::at(), number: ISSUE, labels: Box::new([SEVERITY]) };
    h.send(&request);
    let result = h.call(fixtures::PERSON, Op::Read(Read::Item { number: ISSUE, after: 0 })).expect("item");
    let a::Answer::Item { item, .. } = result else { panic!("item") };
    assert_eq!(item.labels.as_ref(), &[boxed(b"severity:high")]);
}
#[test]
fn label_creation_http_defines_the_repository_label() {
    let mut h = Harness::new(fixtures::config());
    h.send(&Request::CreateLabel { at: fixtures::at(), name: boxed(b"finding"), color: [0x12, 0xab, 0xef] });
    assert_eq!(h.status(), 201);
    assert_eq!(h.call(fixtures::PERSON, Op::Read(Read::Labels)), Ok(a::Answer::Labels(Box::new([boxed(b"finding")]))));
}
