use super::{fixtures as f, read_support as r, write_support as w};
use skein_forge::{
    api,
    github::request::{Method, Request},
};

#[test]
fn github_fake_issue_writes_drive_plans_and_read_back() {
    let mut h = w::harness();
    let token = r::token(&mut h);
    let api::Answer::Created(number) = w::write(
        &mut h,
        &token,
        api::Write::CreateIssue { key: f::boxed(b"issue-key"), title: f::boxed(b"first"), body: f::boxed(b"body") },
    )
    .unwrap() else {
        panic!("created issue")
    };
    assert_eq!(
        w::write(
            &mut h,
            &token,
            api::Write::Edit { number, title: Some(f::boxed(b"edited")), body: Some(f::boxed(b"new body")) }
        ),
        Ok(api::Answer::Done)
    );
    assert_eq!(w::write(&mut h, &token, api::Write::Close { number }), Ok(api::Answer::Done));
    let api::Answer::Item { item, .. } = r::read(&mut h, &token, api::Read::Item { number, after: 0 }).unwrap() else {
        panic!("closed item")
    };
    assert_eq!(item.state, api::State::Closed);
    assert_eq!(w::write(&mut h, &token, api::Write::Reopen { number }), Ok(api::Answer::Done));
    let api::Answer::Commented(id) = w::write(
        &mut h,
        &token,
        api::Write::Post { number, key: f::boxed(b"comment-key"), body: f::boxed(b"comment") },
    )
    .unwrap() else {
        panic!("posted comment")
    };
    let api::Answer::Item { item, comments, .. } =
        r::read(&mut h, &token, api::Read::Item { number, after: 0 }).unwrap()
    else {
        panic!("item readback")
    };
    assert_eq!(item.state, api::State::Open);
    assert_eq!(item.title.as_ref(), b"edited");
    assert_eq!(item.body.as_ref(), b"new body");
    let comment = comments.first().unwrap();
    assert_eq!(comment.id, id);
    assert!(comment.body.ends_with(b"\ncomment"));
    assert_eq!(comment.key.as_deref(), Some(b"comment-key".as_slice()));
    assert_eq!(w::write(&mut h, &token, api::Write::Close { number: 999 }), Err(api::Error::Missing));
    assert_eq!(
        w::write(
            &mut h,
            &token,
            api::Write::CreateIssue { key: f::boxed(b"empty"), title: Box::from([]), body: Box::from([]) }
        ),
        Err(api::Error::Empty)
    );
}
#[test]
fn github_fake_issue_write_scope_precedes_body_decoding() {
    let mut h = w::harness();
    let token = r::token(&mut h);
    for request in [
        Request::CreateIssue {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"absent"),
            title: Box::from([]),
            body: Box::from([]),
        },
        Request::Post { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), number: 1, body: Box::from([]) },
        Request::Edit { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), number: 1, title: None, body: None },
        Request::Close { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), number: 1 },
        Request::Reopen { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), number: 1 },
    ] {
        let encoded = skein_forge::github::request::encode(&request, &h.limits.github).unwrap();
        let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), b"invalid");
        f::refusal(&h, &response, 404, b"Not Found");
    }
    assert_eq!(h.calls, 0);
    for body in [br#"{"state":"invalid"}"#.as_slice(), br#"{"state":"closed","title":"mixed"}"#, b"invalid"] {
        let response = h.ask(Method::Patch, b"/repos/acme/widgets/issues/1", Some(token.expose()), body);
        f::refusal(&h, &response, 422, b"Validation Failed");
    }
}
