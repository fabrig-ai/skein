use super::{Body, Failure, Heard, plan::Exchange, start};
use crate::forgejo::{
    credential,
    decode::Malformed,
    document::{Document, Row},
    request::Request,
    tests::fixtures,
};
use alloc::boxed::Box;
use skein_lib::Duration;

fn begun(read: start::Read) -> start::Plan {
    let mut plan = start::Plan::new(read);
    let _sent = plan.begin();
    plan
}

#[test]
fn identity_start_sends_user_and_returns_id() {
    let mut plan = start::Plan::new(start::Read::User);
    let sent = plan.begin();
    let head = plan.heard(Heard::Head { status: 200, fields: fixtures::fields() });
    let row = plan.heard(Heard::Row(Row::User(fixtures::user(fixtures::START_USER, b"fixture"))));
    let ended =
        plan.heard(Heard::Done(Body::Document(Document::User(fixtures::user(fixtures::START_USER, b"fixture")))));
    assert_eq!(
        (sent, head, row, ended),
        (
            start::Step::Send(Exchange { request: Request::User, range: None }),
            None,
            None,
            Some(start::Step::Done(start::Heard::User { id: fixtures::START_USER }))
        )
    );
}

#[test]
fn github_rate_and_validator_are_none_in_forgejo_plans() {
    let fields = super::Fields { date: None, retry_after: None, has_more: None, content_range: None, close: false };
    let mut plan = begun(start::Read::User);
    assert_eq!(plan.heard(Heard::Head { status: 200, fields: fields.clone() }), None);
    let user = fixtures::user(fixtures::START_USER, b"alice");
    let terminal = plan.heard(Heard::Done(Body::Document(Document::User(user))));
    assert_eq!(terminal, Some(start::Step::Done(start::Heard::User { id: fixtures::START_USER })));

    let mut conditional = begun(start::Read::User);
    assert_eq!(
        conditional.heard(Heard::Head { status: 304, fields }),
        Some(start::Step::Done(start::Heard::Status { status: 304, retry: None }))
    );
}

#[test]
fn page_cap_start_returns_positive_cap() {
    let mut plan = start::Plan::new(start::Read::ApiSettings);
    let sent = plan.begin();
    let ended = plan.heard(Heard::Done(Body::Document(fixtures::api_settings(fixtures::PAGE_CAP))));
    assert_eq!(
        (sent, ended),
        (
            start::Step::Send(Exchange { request: Request::ApiSettings, range: None }),
            Some(start::Step::Done(start::Heard::ApiSettings { max_response_items: fixtures::PAGE_CAP }))
        )
    );
}

#[test]
fn zero_page_cap_is_unanswered() {
    let mut plan = begun(start::Read::ApiSettings);
    let ended = plan.heard(Heard::Done(Body::Document(fixtures::api_settings(0))));
    assert_eq!(ended, Some(start::Step::Done(start::Heard::Unanswered)));
}

#[test]
fn start_status_preserves_status_and_retry() {
    const RETRY: Duration = Duration::from_secs(5);
    for read in [start::Read::User, start::Read::ApiSettings] {
        for (status, retry) in [(429, Some(RETRY)), (401, None), (204, None)] {
            let mut plan = begun(read);
            let ended =
                plan.heard(Heard::Head { status, fields: super::Fields { retry_after: retry, ..fixtures::fields() } });
            let again = plan.heard(Heard::Failed(Failure::Seen));
            assert_eq!(
                (ended, again),
                (Some(start::Step::Done(start::Heard::Status { status, retry })), None),
                "{read:?} {status}"
            );
        }
    }
}

#[test]
fn bad_start_answer_is_unanswered() {
    for read in [start::Read::User, start::Read::ApiSettings] {
        let wrong_document = match read {
            start::Read::User => fixtures::api_settings(fixtures::PAGE_CAP),
            start::Read::ApiSettings => Document::User(fixtures::user(fixtures::START_USER, b"fixture")),
        };
        for heard in [
            Heard::Done(Body::Document(wrong_document)),
            Heard::Done(Body::Document(Document::Search { ok: true })),
            Heard::Done(Body::Malformed(Malformed::Shape { field: b"id" })),
            Heard::Done(Body::Empty),
            Heard::Done(Body::Raw { bytes: Box::new([]), more: false }),
            Heard::Failed(Failure::Unseen),
            Heard::Failed(Failure::Seen),
        ] {
            let mut plan = begun(read);
            let ended = plan.heard(heard.clone());
            let again = plan.heard(heard.clone());
            assert_eq!((ended, again), (Some(start::Step::Done(start::Heard::Unanswered)), None), "{read:?} {heard:?}");
        }
    }
}

#[test]
fn token_authorization_has_the_dialect_prefix() {
    let rendered = credential::authorization(&fixtures::credential()).expect("fixture fits");
    assert_eq!(rendered.expose(), b"token secret");
}

#[test]
fn credential_debug_contains_no_secret() {
    let credential = fixtures::credential();
    let rendered = credential::authorization(&credential).expect("fixture fits");
    for debug in [fixtures::debug(&credential), fixtures::debug(&rendered)] {
        assert_eq!(skein_lib::bytes::find(&debug, fixtures::TOKEN), None);
    }
}

mod calls;

mod edges;

mod job_resolution;
