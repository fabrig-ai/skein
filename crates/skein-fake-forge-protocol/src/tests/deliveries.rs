use super::fixtures::{self, Harness, boxed};
use crate::{Service, delivery, routes};
use skein_fake_forge_domain::{
    self as domain,
    api::{Change, Check, Ended, JobRef},
};
use skein_forge::forgejo::{
    delivery::{Delivery, JobEnd, RefType},
    document::Body,
    request::ReviewEvent,
};
use skein_lib::{Secret, Token};
const REPOSITORY: u64 = 2;
const ITEM: u64 = 3;
fn decoded(post: &delivery::Post) -> Result<Delivery, skein_forge::forgejo::decode::Malformed> {
    fixtures::deliver(fixtures::delivery_event(post), &post.body, &fixtures::limits().documents)
}

const PUSH_BODY: &[u8] = br#"{"repository":{"id":2},"ref":"refs/heads/main","after":"63cbe1e459320dd7000000000000000700000000","pusher":{"id":2}}"#;
// openssl dgst -sha256 -hmac fixture-hook-secret, over PUSH_BODY with no newline.
const PUSH_HEAD: &[u8] = b"POST /hooks/1 HTTP/1.1\r\nHost: 127.0.0.1:7200\r\nContent-Type: application/json\r\nContent-Length: 116\r\nX-Forgejo-Event: push\r\nX-Forgejo-Signature: e9688af42c3f662a0f3a6fb23533ee787529adec729a21cac826a3886e52e81d\r\n\r\n";

fn hook(change: Change) -> domain::Request {
    domain::Request::Hook {
        queue: None,
        repository: boxed(b"org/repo"),
        change,
        number: Some(3),
        branch: Some(boxed(b"main")),
        commit: Some(7),
        by: Some(2),
        body: match change {
            Change::Issue | Change::Comment | Change::Review => Some(boxed(b"fixture body")),
            Change::Pull | Change::Job | Change::Push | Change::Group { .. } => None,
        },
        ended: (change == Change::Job).then_some(Ended {
            job: JobRef { run: 4, job: 5, attempt: 1 },
            commit: 7,
            state: Check::Passed,
        }),
    }
}
fn delivered(change: Change) -> delivery::Post {
    Service::new(fixtures::hooked_config(), &fixtures::limits(), 3)
        .expect("service")
        .delivery(&hook(change), &fixtures::limits())
        .expect("configured delivery")
}
#[test]
fn a_push_hook_is_delivered_as_one_signed_post_to_the_forges_path() {
    let post = delivered(Change::Push);
    assert_eq!(&*post.body, PUSH_BODY);
    assert_eq!(&*post.head, PUSH_HEAD);
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Push {
            repository: REPOSITORY,
            reference: boxed(b"refs/heads/main"),
            after: routes::object(7),
            pusher: fixtures::PERSON
        })
    );
}
#[test]
fn an_issue_hook_is_delivered_under_the_issues_event_naming_its_number() {
    let post = delivered(Change::Issue);
    assert_eq!(&*post.body, br#"{"repository":{"id":2},"number":3,"issue":{"body":"fixture body"}}"#);
    assert_eq!(fixtures::delivery_event(&post), b"issues");
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Issue { repository: REPOSITORY, number: ITEM, body: Body::Kept(boxed(b"fixture body")) })
    );
}
#[test]
fn a_jobs_end_is_delivered_naming_its_commit() {
    let post = delivered(Change::Job);
    assert_eq!(
        &*post.body,
        br#"{"run":{"repository":{"id":2},"commit_sha":"63cbe1e459320dd7000000000000000700000000"}}"#
    );
    assert_eq!(fixtures::delivery_event(&post), b"workflow_job_success");
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Job { end: JobEnd::Success, repository: REPOSITORY, commit: routes::object(7) })
    );
}
#[test]
fn a_failed_jobs_end_is_delivered_under_its_failure_event() {
    let service = Service::new(fixtures::hooked_config(), &fixtures::limits(), 3).expect("service");
    let mut request = hook(Change::Job);
    if let domain::Request::Hook { ended, .. } = &mut request {
        ended.as_mut().expect("job end").state = Check::Failed;
    }
    let post = service.delivery(&request, &fixtures::limits()).expect("job end");
    assert_eq!(fixtures::delivery_event(&post), b"workflow_job_failure");
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Job { end: JobEnd::Failure, repository: REPOSITORY, commit: routes::object(7) })
    );
}
#[test]
fn a_comment_hook_delivers_its_body_with_the_items_number() {
    let post = delivered(Change::Comment);
    assert_eq!(&*post.body, br#"{"repository":{"id":2},"issue":{"number":3},"comment":{"body":"fixture body"}}"#);
    assert_eq!(fixtures::delivery_event(&post), b"issue_comment");
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Comment { repository: REPOSITORY, number: ITEM, body: Body::Kept(boxed(b"fixture body")) })
    );
}
#[test]
fn a_review_hook_delivers_its_content_with_the_items_number() {
    let post = delivered(Change::Review);
    assert_eq!(&*post.body, br#"{"repository":{"id":2},"number":3,"review":{"content":"fixture body"}}"#);
    assert_eq!(fixtures::delivery_event(&post), b"pull_request_approved");
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Review {
            event: ReviewEvent::Approved,
            repository: REPOSITORY,
            number: ITEM,
            body: Body::Kept(boxed(b"fixture body"))
        })
    );
}
#[test]
fn a_deleted_branch_is_delivered_as_a_delete_event() {
    let service = Service::new(fixtures::hooked_config(), &fixtures::limits(), 3).expect("service");
    let mut request = hook(Change::Push);
    if let domain::Request::Hook { commit, .. } = &mut request {
        *commit = None;
    }
    let post = service.delivery(&request, &fixtures::limits()).expect("deletion");
    assert_eq!(&*post.body, br#"{"repository":{"id":2},"ref":"refs/heads/main","ref_type":"branch"}"#);
    assert_eq!(fixtures::delivery_event(&post), b"delete");
    assert_eq!(
        decoded(&post),
        Ok(Delivery::Ref {
            created: false,
            repository: REPOSITORY,
            reference: boxed(b"refs/heads/main"),
            ref_type: RefType::Branch
        })
    );
}
#[test]
fn a_hook_with_no_endpoint_is_not_delivered() {
    let service = Service::new(fixtures::config(), &fixtures::limits(), 3).expect("service");
    assert_eq!(service.delivery(&hook(Change::Push), &fixtures::limits()), None);
}
#[test]
fn a_delivery_endpoint_with_an_empty_secret_is_refused() {
    let mut config = fixtures::hooked_config();
    config.hooks.as_mut().expect("endpoint").secret = Secret::new(boxed(b""));
    assert_eq!(Service::new(config, &fixtures::limits(), 3).err(), Some(crate::Error::Limits));
}
#[test]
fn a_delivery_endpoint_with_an_empty_host_is_refused() {
    let mut config = fixtures::hooked_config();
    config.hooks.as_mut().expect("endpoint").host = boxed(b"");
    assert_eq!(Service::new(config, &fixtures::limits(), 3).err(), Some(crate::Error::Limits));
}
#[test]
fn the_delivery_signature_is_rfc_4231s_for_its_key_and_data() {
    assert_eq!(
        &delivery::sign(&Secret::new(boxed(b"Jefe")), b"what do ya want for nothing?"),
        b"5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}
#[test]
fn a_domain_push_is_delivered_signed_to_the_world() {
    let mut harness = Harness::hooked();
    assert_eq!(harness.push(b"work"), 2);
    harness.run();
    let post = harness.delivered(b"push");
    assert_eq!(&*post.body, br#"{"repository":{"id":2},"ref":"refs/heads/work","after":"975835de1c9756ce000000000000000200000000","pusher":{"id":2}}"#);
    // openssl dgst -sha256 -hmac fixture-hook-secret, over the literal body above.
    assert_eq!(&*post.head, b"POST /hooks/1 HTTP/1.1\r\nHost: 127.0.0.1:7200\r\nContent-Type: application/json\r\nContent-Length: 116\r\nX-Forgejo-Event: push\r\nX-Forgejo-Signature: 06e31c2c9354abfe7f536887dcaba9d19dc45d35781513c32b90c31fa249c6b8\r\n\r\n");
    assert_eq!(
        decoded(post),
        Ok(Delivery::Push {
            repository: REPOSITORY,
            reference: boxed(b"refs/heads/work"),
            after: routes::object(2),
            pusher: fixtures::PERSON
        })
    );
}

#[test]
fn service_refuses_zero_call_capacity() {
    let config = fixtures::config();
    let limits = crate::Limits { calls: 0, ..fixtures::limits() };
    assert!(!crate::valid(&limits, &config));
    assert_eq!(Service::new(config, &limits, 3).err(), Some(crate::Error::Limits));
}

#[test]
fn a_signature_holds_for_its_body_only() {
    let post = delivered(Change::Push);
    let secret = Secret::new(boxed(fixtures::FIXTURE_SECRET));
    let signature = *b"e9688af42c3f662a0f3a6fb23533ee787529adec729a21cac826a3886e52e81d";
    assert_eq!(delivery::sign(&secret, &post.body), signature);
    assert_ne!(delivery::sign(&secret, b"{}"), signature);
}

#[test]
fn forgejo_never_delivers_a_merge_queue_group_hook() {
    let limits = fixtures::limits();
    let service = Service::new(fixtures::hooked_config(), &limits, 3).unwrap();
    assert!(service.delivery(&hook(Change::Group { ended: false }), &limits).is_none());
}

#[test]
fn constructors_refuse_unrepresentable_http_json_and_walk_limits() {
    for case in 0_u32..6 {
        let mut limits = fixtures::limits();
        let mut config = fixtures::config();
        match case {
            0 => limits.http.send = u32::MAX,
            1 => limits.documents.json.length = u32::MAX,
            2 => limits.documents.json.chunk = limits.http.read + 1,
            3 => limits.raw_bytes = 0,
            4 => limits.connections = 0,
            5 => config.max_response_items = u32::MAX,
            _ => unreachable!(),
        }
        assert!(!crate::valid(&limits, &config));
        assert_eq!(Service::new(config, &limits, 3).err(), Some(crate::Error::Limits));
        if case < 5 {
            assert_eq!(crate::Server::new(Token::new(1), &limits).err(), Some(crate::Error::Limits));
        }
    }
}
