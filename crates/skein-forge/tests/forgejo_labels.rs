#![cfg(test)]
pub mod common;
use common::forgejo;
use skein_forge::forgejo::{
    client,
    document::{Document, Row},
    request::{Page, Repo, Request},
};
fn at() -> Repo {
    Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"labels") }
}
#[test]
fn label_write_routes_replay_the_pinned_requests() {
    for (case, request) in [
        ("label-created", Request::CreateLabel { at: at(), name: Box::from(*b"finding"), color: [0x12, 0xab, 0xef] }),
        (
            "label-created-again",
            Request::CreateLabel { at: at(), name: Box::from(*b"finding"), color: [0x12, 0xab, 0xef] },
        ),
        (
            "issue-created-with-labels",
            Request::CreateIssue {
                at: at(),
                title: Box::from(*b"finding"),
                body: Box::from(*b"labelled issue"),
                labels: Box::new([1, 3]),
            },
        ),
        ("issue-labels-replaced", Request::SetLabels { at: at(), number: 1, labels: Box::new([1]) }),
        ("issue-label-unknown", Request::SetLabels { at: at(), number: 1, labels: Box::new([5]) }),
    ] {
        let transcript = forgejo::label_transcript(case);
        let encoded = client::encode(&request, &forgejo::limits()).unwrap();
        assert_eq!(encoded.method, transcript.method);
        assert_eq!(encoded.target, transcript.target.into_boxed_slice());
        assert_eq!(encoded.body.as_deref(), Some(transcript.request_body.as_slice()));
        assert_eq!(
            forgejo::decode_route(transcript.method, &encoded.target, &transcript.request_body, &forgejo::limits())
                .unwrap()
                .0,
            request
        );
    }
}
#[test]
fn the_recorded_catalog_preserves_duplicate_names_and_ids() {
    let transcript = forgejo::label_transcript("labels-listed");
    let request = Request::Labels { at: at(), page: Page { page: Some(1), limit: Some(50) } };
    for chunk in [1, 7, transcript.body.len()] {
        let decoded = forgejo::decode_chunks(&request, &transcript.body, chunk);
        assert_eq!(decoded.document, Ok(Document::Listed { skipped: 0 }));
        assert_eq!(
            decoded.rows.as_slice(),
            &[Row::Label { id: 1, name: Box::from(*b"finding") }, Row::Label { id: 2, name: Box::from(*b"finding") }]
        );
    }
}
#[test]
fn recorded_unknown_ids_are_dropped_by_the_pin() {
    let transcript = forgejo::label_transcript("issue-label-unknown");
    let request = Request::SetLabels { at: at(), number: 1, labels: Box::new([5]) };
    let decoded = forgejo::decode(&request, &transcript.body);
    assert_eq!(transcript.status, 200);
    assert_eq!(decoded.document, Ok(Document::Listed { skipped: 0 }));
    assert!(decoded.rows.is_empty());
}
