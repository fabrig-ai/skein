use super::fixtures::{at, boxed, config, limits};
use crate::{
    response::{self, Range, Select},
    routes,
};
use skein_fake_forge_domain::api as a;
use skein_forge::forgejo::{
    document as d,
    request::{Page, Request, ReviewEvent},
};
use skein_lib::{Time, bytes};

fn readback_rows() -> [(Request, a::Answer, a::Read, Select); 7] {
    [
        (
            Request::CreateIssue { at: at(), title: boxed(b"title"), body: boxed(b"body") },
            a::Answer::Created(9),
            a::Read::Item { number: 9, after: 0 },
            Select::Document,
        ),
        (
            Request::OpenPull {
                at: at(),
                title: boxed(b"title"),
                body: boxed(b"body"),
                head: boxed(b"work"),
                base: boxed(b"main"),
            },
            a::Answer::Created(9),
            a::Read::Pull { number: 9 },
            Select::Document,
        ),
        (
            Request::PostComment { at: at(), number: 1, body: boxed(b"body") },
            a::Answer::Commented(7),
            a::Read::Comment { id: 7 },
            Select::Document,
        ),
        (
            Request::PostReview { at: at(), number: 1, event: ReviewEvent::Approved, body: boxed(b"body") },
            a::Answer::Reviewed(8),
            a::Read::Pull { number: 1 },
            Select::Review(8),
        ),
        (
            Request::EditIssue { at: at(), number: 1, title: None, body: None, state: Some(d::IssueState::Closed) },
            a::Answer::Done,
            a::Read::Item { number: 1, after: 0 },
            Select::Document,
        ),
        (
            Request::PostStatus {
                at: at(),
                commit: routes::object(7),
                context: boxed(b"ci"),
                state: d::StatusState::Failure,
                description: Box::new([]),
                link: Box::new([]),
            },
            a::Answer::Done,
            a::Read::Checks { commit: 7 },
            Select::Status(boxed(b"ci")),
        ),
        (
            Request::CreateBranch { at: at(), branch: boxed(b"work"), from: routes::object(7) },
            a::Answer::Done,
            a::Read::Branch { branch: boxed(b"work") },
            Select::Branch(boxed(b"work")),
        ),
    ]
}
#[test]
fn document_returning_writes_select_the_actual_post_write_read() {
    for (request, reply, expected_read, expected_select) in readback_rows() {
        assert_eq!(response::follow(&request, &reply), Ok(Some((expected_read, expected_select))), "{request:?}");
    }
}
#[test]
fn a_merge_answers_200_with_no_body() {
    for (request, status, body) in [
        (
            Request::Merge { at: at(), number: 1, head: routes::object(7), style: d::MergeStyle::Squash },
            200,
            b"".as_slice(),
        ),
        (Request::Update { at: at(), number: 1, style: None }, 200, b""),
    ] {
        assert_eq!(response::follow(&request, &a::Answer::Done), Ok(None), "{request:?}");
        assert_eq!(
            response::answer(
                &config(),
                &crate::setup::Store::new(&limits()),
                &limits(),
                &request,
                &Select::Document,
                &a::Answer::Done
            ),
            Ok(response::Reply { status, headers: Box::from([]), body: boxed(body) }),
            "{request:?}"
        );
    }
}
#[test]
fn reviewer_removal_and_branch_deletion_answer_204_with_no_body() {
    for request in [
        Request::RemoveReviewers { at: at(), number: 1, reviewers: Box::new([boxed(b"person")]) },
        Request::DeleteBranch { at: at(), branch: boxed(b"work") },
    ] {
        assert_eq!(response::follow(&request, &a::Answer::Done), Ok(None), "{request:?}");
        assert_eq!(done_reply(&request), response::Reply { status: 204, headers: Box::from([]), body: Box::from([]) });
    }
}
#[test]
fn a_reviewer_request_answers_201_with_no_body() {
    let request = Request::RequestReviewers { at: at(), number: 1, reviewers: Box::new([boxed(b"person")]) };
    // Forgejo sends the pull's reviews, which the client does not read.
    assert_eq!(done_reply(&request), response::Reply { status: 201, headers: Box::from([]), body: Box::from([]) });
}
fn done_reply(request: &Request) -> response::Reply {
    response::answer(
        &config(),
        &crate::setup::Store::new(&limits()),
        &limits(),
        request,
        &Select::Document,
        &a::Answer::Done,
    )
    .expect("bodyless write")
}
#[test]
fn write_responses_keep_the_readbacks_author_text_and_time() {
    let readback = a::Answer::Item {
        item: a::Summary {
            number: 9,
            kind: a::Kind::Issue,
            state: a::State::Closed,
            merged: false,
            title: boxed(b"existing title"),
            body: boxed(b"existing body"),
            author: 2,
            labels: Box::from([]),
            created: Time::ZERO,
            updated: Time::from_nanos(2_000_000_000),
        },
        comments: Box::from([]),
        more: false,
    };
    let request =
        Request::EditIssue { at: at(), number: 9, title: None, body: None, state: Some(d::IssueState::Closed) };
    let reply = response::answer(
        &config(),
        &crate::setup::Store::new(&limits()),
        &limits(),
        &request,
        &Select::Document,
        &readback,
    )
    .expect("document");
    assert_eq!(reply.status, 201);
    for expected in [
        b"\"id\":2".as_slice(),
        b"\"title\":\"existing title\"",
        b"\"body\":\"existing body\"",
        b"\"updated_at\":\"1970-01-01T00:00:02Z\"",
        b"\"state\":\"closed\"",
    ] {
        assert!(bytes::find(&reply.body, expected).is_some(), "{expected:?}");
    }
}
#[test]
fn raw_ranges_use_206_and_literal_content_range() {
    let request = Request::Raw { at: at(), path: boxed(b"src"), reference: routes::object(7) };
    for (range, status, body, content_range) in [
        (None, 200, b"abcdef".as_slice(), None),
        (Some(Range::From { first: 1, last: Some(3) }), 206, b"bcd", Some(b"bytes 1-3/6".as_slice())),
        (Some(Range::From { first: 3, last: Some(99) }), 206, b"def", Some(b"bytes 3-5/6".as_slice())),
        (Some(Range::From { first: 9, last: None }), 416, b"", None),
    ] {
        let reply = response::answer(
            &config(),
            &crate::setup::Store::new(&limits()),
            &limits(),
            &request,
            &Select::Raw(range),
            &a::Answer::File(boxed(b"abcdef")),
        )
        .expect("raw");
        assert_eq!((reply.status, &*reply.body), (status, body), "{range:?}");
        let header = reply.headers.first();
        assert_eq!(header_value(&reply.headers), content_range, "{range:?}");
        if let Some(header) = header {
            assert_eq!(&*header.name, b"content-range", "{range:?}");
        }
    }
}
fn header_value(headers: &[skein_http::Header]) -> Option<&[u8]> {
    let header = headers.first()?;
    Some(&header.value)
}
#[test]
fn range_syntax_is_bounded_and_rejects_reversed_offsets() {
    for (input, expected) in [
        (None, Ok(None)),
        (Some(b"bytes=1-3".as_slice()), Ok(Some(Range::From { first: 1, last: Some(3) }))),
        (Some(b"bytes=1-".as_slice()), Ok(Some(Range::From { first: 1, last: None }))),
        (Some(b"bytes=-2".as_slice()), Ok(Some(Range::Suffix(2)))),
        (Some(b"bytes=-0".as_slice()), Err(a::Error::Refused)),
        (Some(b"bytes=-".as_slice()), Err(a::Error::Refused)),
        (Some(b"bytes=1-0".as_slice()), Err(a::Error::Refused)),
        (Some(b"items=1-3".as_slice()), Err(a::Error::Refused)),
        (Some(b"bytes=18446744073709551616-".as_slice()), Err(a::Error::Refused)),
    ] {
        assert_eq!(response::range(input), expected, "{input:?}");
    }
}
#[test]
fn pull_file_responses_carry_x_hasmore() {
    let request = Request::PullFiles { at: at(), number: 1, page: Page { page: Some(1), limit: Some(2) } };
    for (more, expected) in [(true, b"true".as_slice()), (false, b"false".as_slice())] {
        let input = a::Answer::PullFiles { head: 7, files: Box::from([]), more };
        let reply = response::answer(
            &config(),
            &crate::setup::Store::new(&limits()),
            &limits(),
            &request,
            &Select::Document,
            &input,
        )
        .expect("files");
        assert_eq!(&*reply.body, b"[]", "more {more}");
        assert_eq!(reply.headers.len(), 1, "more {more}");
        let header = reply.headers.first().expect("one");
        assert_eq!((&*header.name, &*header.value), (b"x-hasmore".as_slice(), expected), "more {more}");
    }
}

#[test]
fn a_suffix_range_answers_206_with_the_slices_last_bytes() {
    let request = Request::JobLog { at: at(), job: 1, attempt: 1, step: Some(3) };
    let mut slice = [b'x'; 112];
    for (place, byte) in slice[102..].iter_mut().zip(b"0123456789") {
        *place = *byte;
    }
    let reply = response::answer(
        &config(),
        &crate::setup::Store::new(&limits()),
        &limits(),
        &request,
        &Select::Raw(Some(Range::Suffix(10))),
        &a::Answer::File(boxed(&slice)),
    )
    .expect("Suffix");
    assert_eq!((reply.status, &*reply.body), (206, b"0123456789".as_slice()));
    assert_eq!(header_value(&reply.headers), Some(b"bytes 102-111/112".as_slice()));
}

#[test]
fn a_suffix_longer_than_the_slice_answers_it_all() {
    let request = Request::JobLog { at: at(), job: 1, attempt: 1, step: Some(3) };
    let slice = [b'x'; 112];
    let reply = response::answer(
        &config(),
        &crate::setup::Store::new(&limits()),
        &limits(),
        &request,
        &Select::Raw(Some(Range::Suffix(100_000))),
        &a::Answer::File(boxed(&slice)),
    )
    .expect("Suffix");
    assert_eq!((reply.status, &*reply.body), (206, slice.as_slice()));
    assert_eq!(header_value(&reply.headers), Some(b"bytes 0-111/112".as_slice()));
}

#[test]
fn a_suffix_range_over_an_empty_log_names_no_byte() {
    let request = Request::JobLog { at: at(), job: 1, attempt: 1, step: Some(3) };
    let reply = response::answer(
        &config(),
        &crate::setup::Store::new(&limits()),
        &limits(),
        &request,
        &Select::Raw(Some(Range::Suffix(10))),
        &a::Answer::File(boxed(b"")),
    )
    .expect("Suffix");
    assert_eq!((reply.status, &*reply.body), (206, b"".as_slice()));
    assert_eq!(header_value(&reply.headers), Some(b"bytes 0--1/0".as_slice()));
}

#[test]
fn a_head_range_over_an_empty_log_answers_200() {
    let request = Request::JobLog { at: at(), job: 1, attempt: 1, step: Some(3) };
    let reply = response::answer(
        &config(),
        &crate::setup::Store::new(&limits()),
        &limits(),
        &request,
        &Select::Raw(Some(Range::From { first: 0, last: Some(10) })),
        &a::Answer::File(boxed(b"")),
    )
    .expect("Head");
    assert_eq!((reply.status, &*reply.body), (200, b"".as_slice()));
    assert_eq!(header_value(&reply.headers), None);
}

#[test]
fn style_refusals_use_the_pinned_status_and_message() {
    let merge = Request::Merge { at: at(), number: 1, head: routes::object(7), style: d::MergeStyle::FastForwardOnly };
    let failure = crate::errors::for_request(a::Error::Conflict, &merge, Time::ZERO);
    assert_eq!(failure.status, 500);
    assert!(bytes::find(failure.message, b"DivergingFastForwardOnly").is_some());
    let rebase =
        Request::Update { at: at(), number: 1, style: Some(skein_forge::forgejo::request::UpdateStyle::Rebase) };
    let failure = crate::errors::for_request(a::Error::Conflict, &rebase, Time::ZERO);
    assert_eq!((failure.status, failure.message), (409, b"rebase failed because of conflict".as_slice()));
    let merge_update =
        Request::Update { at: at(), number: 1, style: Some(skein_forge::forgejo::request::UpdateStyle::Merge) };
    let failure = crate::errors::for_request(a::Error::Conflict, &merge_update, Time::ZERO);
    assert_eq!((failure.status, failure.message), (409, b"merge failed because of conflict".as_slice()));
}
