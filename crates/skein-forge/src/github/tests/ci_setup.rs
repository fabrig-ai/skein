use super::*;

const PRESENT: &[u8] =
    br#"{"total_count":2,"workflows":[{"id":17,"name":"CI","path":".github/workflows/ci.yml","state":"active"}]}"#;
const EMPTY: &[u8] = br#"{"total_count":0,"workflows":[]}"#;

fn body(kind: Kind, wire: &[u8]) -> Body {
    match decode(kind, wire, 1, &limits()).1 {
        Ok(document) => Body::Document(document),
        Err(error) => Body::Malformed(error),
    }
}
fn setup_call(actions: Body, workflows: Body, next: bool) -> Result<Option<Step>, api::Error> {
    let mut call = Call::new(api::Read::CiSetup, bounds(), true);
    assert_eq!(call.begin(), Ok(Step::send(Request::ActionsPermissions { owner: owner(), name: name() })));
    call.head(false);
    assert_eq!(
        call.heard(Heard::Done(actions)),
        Ok(Some(Step::send(Request::Workflows { owner: owner(), name: name() })))
    );
    call.head(next);
    call.heard(Heard::Done(workflows))
}
fn setup_answer(actions: Option<bool>, workflows: Option<bool>) -> Result<Option<Step>, api::Error> {
    ended(Ok(api::Answer::CiSetup(api::CiSetup { actions, workflows })))
}
#[test]
fn github_ci_setup_enabled_and_proven_empty() {
    golden(
        &Request::ActionsPermissions { owner: owner(), name: name() },
        b"/repos/acme/widgets/actions/permissions",
        Some(Kind::ActionsPermissions),
        Accept::Json,
    );
    golden(
        &Request::Workflows { owner: owner(), name: name() },
        b"/repos/acme/widgets/actions/workflows?per_page=1",
        Some(Kind::Workflows),
        Accept::Json,
    );
    assert_eq!(
        setup_call(body(Kind::ActionsPermissions, br#"{"enabled":true}"#), body(Kind::Workflows, EMPTY), false),
        setup_answer(Some(true), Some(false))
    );
}
#[test]
fn github_ci_setup_disabled_is_independent_of_workflows() {
    assert_eq!(
        setup_call(body(Kind::ActionsPermissions, br#"{"enabled":false}"#), body(Kind::Workflows, PRESENT), false),
        setup_answer(Some(false), Some(true))
    );
}
#[test]
fn github_ci_setup_present_with_continuation_stops_after_page_one() {
    assert_eq!(
        setup_call(body(Kind::ActionsPermissions, br#"{"enabled":true}"#), body(Kind::Workflows, PRESENT), true),
        setup_answer(Some(true), Some(true))
    );
}
#[test]
fn github_ci_setup_partial_or_malformed_never_proves_absence() {
    for (wire, next) in [
        (EMPTY, true),
        (br#"{"total_count":1,"workflows":[]}"#, false),
        (br#"{"workflows":[]}"#, false),
        (br#"{"total_count":0}"#, false),
        (br#"{"total_count":0,"workflows":null}"#, false),
        (br#"{"total_count":0,"workflows":[{}]}"#, false),
        (br#"{"total_count":0,"workflows":[{"id":1,"name":"CI","path":"ci.yml","state":"active"}]}"#, false),
        (br#"{"total_count":0,"workflows":[]"#, false),
        (br#"{"total_count":-1,"workflows":[]}"#, false),
        (br#"{"total_count":0,"total_count":0,"workflows":[]}"#, false),
    ] {
        assert_eq!(
            setup_call(body(Kind::ActionsPermissions, br#"{"enabled":true}"#), body(Kind::Workflows, wire), next),
            setup_answer(Some(true), None),
            "{wire:?}, next={next}"
        );
    }
}
#[test]
fn github_ci_setup_bad_actions_still_reads_workflows() {
    for wire in
        [b"{}".as_slice(), br#"{"enabled":null}"#, br#"{"enabled":"false"}"#, br#"{"enabled":true,"enabled":false}"#]
    {
        assert_eq!(
            setup_call(body(Kind::ActionsPermissions, wire), body(Kind::Workflows, EMPTY), false),
            setup_answer(None, Some(false))
        );
    }
}
#[test]
fn github_ci_setup_optional_refusals_keep_facts_independent() {
    for status in [403, 404, 410, 500, 501, 503] {
        let mut call = Call::new(api::Read::CiSetup, bounds(), true);
        let _sent = call.begin().unwrap();
        assert_eq!(call.heard(Heard::Head { status, head: Head::new() }), Ok(None));
        assert_eq!(
            call.done(Document::Error(document::Refusal { message: Box::from(*b"Unavailable"), errors: Box::new([]) })),
            Ok(Some(Step::send(Request::Workflows { owner: owner(), name: name() })))
        );
        call.head(false);
        assert_eq!(call.heard(Heard::Done(body(Kind::Workflows, EMPTY))), setup_answer(None, Some(false)));
        assert!(call.tables.repository(KEY).is_some());

        let mut call = started(api::Read::CiSetup, bounds());
        let _sent = call.done(Document::ActionsPermissions(true)).unwrap();
        assert_eq!(call.heard(Heard::Head { status, head: Head::new() }), Ok(None));
        assert_eq!(call.heard(Heard::Done(Body::Empty)), setup_answer(Some(true), None));
    }
}
#[test]
fn github_ci_setup_preserves_transport_authentication_and_rate_errors() {
    for (heard, expected) in [
        (Heard::Failed(Failure::Unseen), api::Error::Unavailable),
        (Heard::Failed(Failure::Seen), api::Error::Timeout),
        (Heard::Head { status: 401, head: Head::new() }, api::Error::Forbidden),
        (Heard::Head { status: 301, head: Head::new() }, api::Error::Unavailable),
        (Heard::Head { status: 307, head: Head::new() }, api::Error::Unavailable),
    ] {
        let mut call = Call::new(api::Read::CiSetup, bounds(), true);
        let _sent = call.begin().unwrap();
        assert_eq!(call.heard(heard), ended(Err(expected)));
    }
    let mut call = Call::new(api::Read::CiSetup, bounds(), true);
    let _sent = call.begin().unwrap();
    let mut head = Head::new();
    head.retry_after = Some(skein_lib::Duration::from_secs(60));
    assert_eq!(call.heard(Heard::Head { status: 429, head }), Ok(None));
    assert_eq!(
        call.heard(Heard::Done(Body::Empty)),
        ended(Err(api::Error::RateLimited { after: skein_lib::Duration::from_secs(60) }))
    );
}
