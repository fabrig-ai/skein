mod accounts;
mod collaborators;
mod delivery;
pub(crate) mod fixtures;
mod hooks;
mod orgs;
mod pin;
mod protection;
mod repositories;
use crate::forgejo::{
    client,
    decode::Malformed,
    document::{Body, Document},
    id, path,
    request::Request,
    time,
};
use alloc::boxed::Box;
use skein_lib::{Wall, Writer};
#[test]
fn path_segments_percent_encode_slashes_spaces_and_utf8() {
    for (name, expected) in [
        (b"main".as_slice(), b"main".as_slice()),
        (b"feature/slash", b"feature%2Fslash"),
        (b"a b", b"a%20b"),
        ("é".as_bytes(), b"%C3%A9"),
    ] {
        let encoded_bytes = expected.len();
        let mut out = Writer::new(encoded_bytes);
        path::segment(name, &mut out).unwrap();
        assert_eq!(&*out.finish(), expected, "{name:?}");
    }
}
#[test]
fn file_paths_keep_slashes_and_percent_encode_spaces() {
    const FILE_TARGET_BYTES: usize = b"docs/a%20b.md".len();
    let mut out = Writer::new(FILE_TARGET_BYTES);
    path::file(b"docs/a b.md", &mut out).unwrap();
    assert_eq!(&*out.finish(), b"docs/a%20b.md");
}
fn encoded_target() -> path::Target {
    path::read(b"/api/v1/repos/a%2Fb/%C3%A9?q=a+b", &fixtures::limits()).unwrap()
}
#[test]
fn target_segments_decode_percent_encoded_slashes() {
    assert_eq!(encoded_target().segments[3].as_ref(), b"a/b");
}
#[test]
fn target_query_values_decode_plus_as_space() {
    assert_eq!(encoded_target().query[0].value.as_ref(), b"a b");
}
const RFC3339_TIMES: [(&[u8], u64); 4] = [
    (b"2020-01-01T00:00:00Z", 1_577_836_800),
    (b"2020-01-01T05:30:00+05:30", 1_577_836_800),
    (b"2019-12-31T17:00:00-07:00", 1_577_836_800),
    (b"2020-01-01T00:00:00.999Z", 1_577_836_800),
];
#[test]
fn rfc3339_reads_offsets_and_discards_fractional_seconds() {
    for (text, seconds) in RFC3339_TIMES {
        assert_eq!(time::read(text).unwrap().as_secs(), seconds, "{text:?}");
    }
}
#[test]
fn rfc3339_writes_offset_times_in_utc() {
    for (text, _) in RFC3339_TIMES {
        assert_eq!(time::write(time::read(text).unwrap()), *b"2020-01-01T00:00:00Z", "{text:?}");
    }
}
#[test]
fn rfc3339_refuses_invalid_dates_and_times_outside_wall() {
    for text in
        [b"1969-12-31T23:59:59Z".as_slice(), b"2020-02-30T00:00:00Z", b"2020-01-01T00:00:00", b"2554-07-21T23:34:34Z"]
    {
        assert_eq!(time::read(text), None, "{text:?}");
    }
}
#[test]
fn rfc3339_writes_the_last_wall_second_in_utc() {
    assert_eq!(time::write(Wall::from_nanos(u64::MAX)), *b"2554-07-21T23:34:33Z");
}
#[test]
fn rfc3339_reads_the_epoch_at_walls_lower_bound() {
    assert_eq!(time::read(b"1970-01-01T00:00:00Z"), Some(Wall::EPOCH));
}
#[test]
fn object_ids_read_40_or_64_lowercase_digits_only() {
    for (length, valid) in [(40, true), (64, true), (39, false), (41, false)] {
        let digits = Box::from([b'a'; 64].get(..length).unwrap());
        let result = id::read(&digits);
        assert_eq!(result.is_some(), valid, "{length}");
        if let Some(value) = result {
            assert_eq!(id::write(&value), digits);
        }
    }
    assert_eq!(id::read(&[b'A'; 40]), None);
    assert_eq!(id::unsigned(b"18446744073709551615"), Some(u64::MAX));
    for text in [b"18446744073709551616".as_slice(), b"-1", b"1.0", b"1e0", b""] {
        assert_eq!(id::unsigned(text), None, "{text:?}");
    }
}
#[test]
fn a_start_document_missing_a_field_is_malformed_naming_it() {
    let limits = fixtures::limits();
    let rows:[(&str,Request,&[u8],Malformed);2]=[
        ("user without login",Request::User,br#"{"id":2,"is_admin":false}"#,Malformed::Missing{field:b"login"}),
        ("repository without its format",Request::RepositoryById{id:1},br#"{"id":1,"owner":{"id":1,"login":"fixture"},"name":"specimen","default_branch":"main","default_merge_style":"merge","allow_merge_commits":true,"allow_squash_merge":true,"allow_rebase":true,"html_url":"http://127.0.0.1:3000/fixture/specimen"}"#,Malformed::Missing{field:b"object_format_name"})];
    for (name, request, body, expected) in rows {
        let decoder = client::decoder(&request, &limits).unwrap();
        assert_eq!(fixtures::decode(decoder, body, &limits).document, Err(expected), "{name}");
    }
}
#[test]
fn a_login_at_its_string_cap_is_kept() {
    let limits = fixtures::small();
    let decoder = client::decoder(&Request::User, &limits).unwrap();
    let result = fixtures::decode(decoder, br#"{"id":2,"login":"12345678"}"#, &limits);
    assert_eq!(result.document, Ok(Document::User(fixtures::user(2, b"12345678"))));
}
#[test]
fn a_login_one_past_its_string_cap_is_malformed() {
    let limits = fixtures::small();
    let decoder = client::decoder(&Request::User, &limits).unwrap();
    let result = fixtures::decode(decoder, br#"{"id":2,"login":"123456789"}"#, &limits);
    assert_eq!(result.document, Err(Malformed::TooLong { field: b"login" }));
}
#[test]
fn a_body_one_past_its_cap_keeps_its_length() {
    // The body fixture has the sixteen retained bytes and one more.
    const LONG_BODY_BYTES: u32 = fixtures::SMALL_BODY_BYTES + 1;
    let limits = fixtures::small();
    let long = fixtures::capped(br#""12345678901234567""#, limits.body_bytes, &limits);
    assert_eq!(Body::read(&long, b"body"), Ok(Body::Long(u64::from(LONG_BODY_BYTES))));
}
#[test]
fn a_body_at_its_cap_keeps_its_bytes() {
    let limits = fixtures::small();
    assert_eq!(
        Body::read(&fixtures::capped(br#""1234567890123456""#, limits.body_bytes, &limits), b"body"),
        Ok(Body::Kept(Box::from(*b"1234567890123456")))
    );
}
#[test]
fn start_documents_refuse_duplicate_null_and_wrongly_typed_fields() {
    let limits = fixtures::limits();
    for (body, expected) in [
        (br#"{"id":2,"id":3,"login":"writer"}"#.as_slice(), Malformed::Shape { field: b"id" }),
        (br#"{"id":null,"login":"writer"}"#, Malformed::Missing { field: b"id" }),
        (br#"{"id":"2","login":"writer"}"#, Malformed::Shape { field: b"id" }),
        (br#"{"id":-1,"login":"writer"}"#, Malformed::Range { field: b"id" }),
    ] {
        let decoder = client::decoder(&Request::User, &limits).unwrap();
        assert_eq!(fixtures::decode(decoder, body, &limits).document, Err(expected), "{body:?}");
    }
}
#[test]
fn unknown_nested_fields_are_skipped_without_retaining_their_strings() {
    let limits = fixtures::small();
    let decoder = client::decoder(&Request::User, &limits).unwrap();
    let result = fixtures::decode(
        decoder,
        br#"{"extra":{"login":"long unknown string","nested":[{}]},"id":1,"login":"fixture"}"#,
        &limits,
    );
    assert_eq!(
        result.document,
        Ok(Document::User(crate::forgejo::document::User {
            id: 1,
            login: Box::from(*b"fixture"),
            is_admin: false,
            full_name: Box::from([])
        }))
    );
    assert!(result.rows.is_empty());
}
// The user target is twelve bytes, so eleven is one byte short.
const USER_TARGET_BYTES: u32 = 12;
fn user_target_limits() -> crate::forgejo::limits::Limits {
    crate::forgejo::limits::Limits { target_bytes: USER_TARGET_BYTES - 1, ..fixtures::limits() }
}
#[test]
fn a_user_target_encodes_at_its_byte_bound() {
    let limits = crate::forgejo::limits::Limits { target_bytes: USER_TARGET_BYTES, ..fixtures::limits() };
    assert_eq!(client::encode(&Request::User, &limits).unwrap().target.as_ref(), b"/api/v1/user");
}
#[test]
fn a_user_target_one_past_its_cap_cannot_encode() {
    assert_eq!(client::encode(&Request::User, &user_target_limits()), Err(client::Refusal::TooLong));
}
#[test]
fn a_user_target_one_past_its_cap_cannot_decode() {
    assert_eq!(path::read(b"/api/v1/user", &user_target_limits()), Err(path::Unread::TooLong));
}
#[test]
fn malformed_percent_escapes_are_refused_before_routing() {
    for target in [b"/bad%".as_slice(), b"/bad%2", b"/bad%GG"] {
        assert_eq!(path::read(target, &user_target_limits()), Err(path::Unread::Escape), "{target:?}");
    }
}
#[test]
fn a_user_route_refuses_the_wrong_method() {
    assert_eq!(
        crate::forgejo::server::route(crate::forgejo::request::Method::Post, b"/api/v1/user", &fixtures::limits())
            .unwrap_err(),
        crate::forgejo::server::Unrouted::Method
    );
}
#[test]
fn an_unknown_target_has_no_route() {
    assert_eq!(
        crate::forgejo::server::route(crate::forgejo::request::Method::Get, b"/api/v1/no-route", &fixtures::limits())
            .unwrap_err(),
        crate::forgejo::server::Unrouted::NotFound
    );
}
#[test]
fn a_comment_at_or_below_after_is_skipped_body_unread() {
    let limits = fixtures::small();
    let request = fixtures::comments(fixtures::COMMENT_THREAD);
    let mut machine = client::decoder(&request, &limits).unwrap();
    client::keep_after(&mut machine, 4);
    let result=fixtures::decode(machine,br#"[{"id":4,"user":{"id":2,"login":"writer"},"body":"12345678901234567"},{"id":5,"user":{"id":2,"login":"writer"},"body":"new"}]"#,&limits);
    assert_eq!(result.document, Ok(Document::Listed { skipped: 0 }));
    assert_eq!(
        result.rows.as_slice(),
        &[crate::forgejo::document::Row::Comment(crate::forgejo::document::Comment {
            id: 5,
            user: crate::forgejo::document::User {
                id: 2,
                login: Box::from(*b"writer"),
                is_admin: false,
                full_name: Box::from([])
            },
            created_at: None,
            updated_at: None,
            body: Body::Kept(Box::from(*b"new"))
        })]
    );
}
#[test]
fn a_cut_thread_counts_the_comments_it_skipped() {
    let limits = fixtures::limits();
    let request = fixtures::comments(fixtures::COMMENT_THREAD);
    let machine = client::decoder(&request, &limits).unwrap();
    let result=fixtures::decode_cut(machine,br#"[{"id":1,"user":{"id":2,"login":"writer"},"body":"first"},{"id":2,"body":"second"},{"id":3,"body":"third"}]"#,&limits,1);
    assert_eq!(result.document, Ok(Document::Listed { skipped: 2 }));
    assert_eq!(result.rows.len(), 1);
}
#[test]
fn a_pull_request_object_makes_an_issue_a_pull_and_null_does_not() {
    let limits = fixtures::limits();
    for (suffix, pull) in [(b",\"pull_request\":{}".as_slice(), true), (b",\"pull_request\":null", false), (b"", false)]
    {
        let prefix=br#"{"number":1,"state":"open","user":{"id":1,"login":"fixture"},"labels":[],"updated_at":"2020-01-01T00:00:00Z","title":"title","body":"body""#;
        let mut out = Writer::new(prefix.len().saturating_add(suffix.len()).saturating_add(1));
        out.put(prefix).unwrap();
        out.put(suffix).unwrap();
        out.put(b"}").unwrap();
        let bytes = out.finish();
        let request = Request::Issue {
            at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
            number: 1,
        };
        let machine = client::decoder(&request, &limits).unwrap();
        let result = fixtures::decode(machine, &bytes, &limits);
        let Document::Issue(value) = result.document.unwrap() else { panic!("an issue") };
        assert_eq!(value.pull, pull, "{suffix:?}");
    }
}
#[test]
fn http_dates_read_imf_fixdate_and_nothing_else() {
    for (text, expected) in [
        (b"Wed, 01 Jan 2020 00:00:00 GMT".as_slice(), Some(Wall::from_nanos(1_577_836_800_000_000_000))),
        (b"Sunday, 06-Nov-94 08:49:37 GMT", None),
        (b"Sun Nov  6 08:49:37 1994", None),
        (b"Wed, 32 Jan 2020 00:00:00 GMT", None),
        (b"Wed, 01 Jan 2020 00:00:00 UTC", None),
    ] {
        assert_eq!(time::http_date(text), expected, "{text:?}");
    }
}
#[test]
fn each_review_state_reads_from_forgejos_word() {
    use crate::forgejo::document::{ReviewState, Row};
    let limits = fixtures::limits();
    let request = Request::Reviews {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        number: 1,
        page: crate::forgejo::request::Page { page: Some(1), limit: Some(2) },
    };
    for (word, state) in [
        (b"APPROVED".as_slice(), ReviewState::Approved),
        (b"REQUEST_CHANGES", ReviewState::RequestChanges),
        (b"COMMENT", ReviewState::Comment),
        (b"PENDING", ReviewState::Pending),
        (b"REQUEST_REVIEW", ReviewState::RequestReview),
    ] {
        let prefix = br#"[{"id":1,"user":{"id":2,"login":"writer"},"state":""#;
        let suffix=br#"","commit_id":"0000000000000000000000000000000000000000","body":"","official":false,"dismissed":false}]"#;
        let mut out = Writer::new(prefix.len() + word.len() + suffix.len());
        out.put(prefix).unwrap();
        out.put(word).unwrap();
        out.put(suffix).unwrap();
        let decoded = fixtures::decode(client::decoder(&request, &limits).unwrap(), &out.finish(), &limits);
        let Row::Review(value) = &decoded.rows.as_slice()[0] else {
            panic!("review row");
        };
        assert_eq!(value.state, state, "{word:?}");
        assert_eq!(decoded.document, Ok(Document::Listed { skipped: 0 }));
    }
}

#[test]
fn a_branch_naming_no_rule_has_no_protection_name() {
    let limits = fixtures::limits();
    let request = Request::Branch {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        name: Box::from(*b"main"),
    };
    let decoded=fixtures::decode(client::decoder(&request,&limits).unwrap(),br#"{"name":"main","commit":{"id":"0000000000000000000000000000000000000003"},"effective_branch_protection_name":""}"#,&limits);
    let Document::Branch(value) = decoded.document.unwrap() else {
        panic!("branch");
    };
    assert_eq!(value.effective_branch_protection_name, None);
}
#[test]
fn a_protection_with_inner_plus_one_contexts_is_past_its_bound() {
    let mut limits = fixtures::limits();
    limits.inner = 1;
    let request = Request::Protection {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        name: Box::from(*b"main"),
    };
    for (body,valid) in [(br#"{"rule_name":"main","enable_status_check":true,"status_check_contexts":["test"],"required_approvals":1,"dismiss_stale_approvals":true,"require_signed_commits":false}"#.as_slice(),true),(br#"{"rule_name":"main","enable_status_check":true,"status_check_contexts":["test","other"],"required_approvals":1,"dismiss_stale_approvals":true,"require_signed_commits":false}"#,false)] {
        let decoded=fixtures::decode(client::decoder(&request,&limits).unwrap(),body,&limits);
        if valid { let Document::Protection(value)=decoded.document.unwrap() else { panic!("protection"); }; assert_eq!(value.status_check_contexts.as_ref(),&[Box::from(b"test".as_slice())]); } else { assert_eq!(decoded.document,Err(Malformed::Bound { field:b"status_check_contexts" })); }
    }
}
#[test]
fn permission_owner_reads_without_mapping_it_to_admin() {
    let limits = fixtures::limits();
    let request = Request::Permission {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        login: Box::from(*b"fixture"),
    };
    let decoded = fixtures::decode(
        client::decoder(&request, &limits).unwrap(),
        br#"{"permission":"owner","user":{"id":1,"login":"fixture"}}"#,
        &limits,
    );
    assert_eq!(
        decoded.document,
        Ok(Document::Permission(crate::forgejo::document::CollaboratorPermission {
            permission: crate::forgejo::document::Permission::Owner,
            user: crate::forgejo::document::User {
                id: 1,
                login: Box::from(*b"fixture"),
                is_admin: false,
                full_name: Box::from([])
            }
        }))
    );
}
#[test]
fn a_cut_review_thread_counts_remaining_comments() {
    let limits = fixtures::limits();
    let request = Request::ReviewComments {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        number: 1,
        review: 5,
    };
    let decoded=fixtures::decode_cut(client::decoder(&request,&limits).unwrap(),br#"[{"id":1,"user":{"id":1,"login":"fixture"},"path":"file.txt","position":1,"body":"first"},{"id":2},{"id":3}]"#,&limits,1);
    assert_eq!(decoded.rows.len(), 1);
    assert_eq!(decoded.document, Ok(Document::Listed { skipped: 2 }));
}
#[test]
fn x_hasmore_reads_true_or_false_only() {
    for (text, expected) in [
        (b"true".as_slice(), Some(true)),
        (b"false", Some(false)),
        (b"True", None),
        (b"1", None),
        (b"", None),
        (b"false ", None),
    ] {
        assert_eq!(crate::forgejo::header::has_more(text), expected, "{text:?}");
    }
}
#[test]
fn a_content_range_reads_its_first_end_and_total() {
    use crate::forgejo::header::{ContentRange, content_range};
    for (text, expected) in [
        (b"bytes 0-3/7".as_slice(), Some(ContentRange { first: 0, end: 4, total: Some(7) })),
        (b"bytes 4-6/*", Some(ContentRange { first: 4, end: 7, total: None })),
        (b"bytes 3-0/7", None),
        (b"bytes 0-7/7", None),
        (b"bytes */7", None),
        (b"bytes 0-1/2 ", None),
        (b"items 0-1/2", None),
        (b"bytes 0--1/2", None),
    ] {
        assert_eq!(content_range(text), expected, "{text:?}");
    }
}
#[test]
fn retry_after_reads_seconds_or_a_date_from_the_answers_date() {
    use skein_lib::Duration;
    let date = Wall::from_nanos(1_577_836_800_000_000_000);
    for (text, expected) in [
        (b"10".as_slice(), Some(Duration::from_secs(10))),
        (b"0", Some(Duration::ZERO)),
        (b"Wed, 01 Jan 2020 00:00:05 GMT", Some(Duration::from_secs(5))),
        (b"Tue, 31 Dec 2019 23:59:59 GMT", Some(Duration::ZERO)),
        (b"-1", None),
        (b"18446744073709551615", None),
        (b"1.5", None),
    ] {
        assert_eq!(crate::forgejo::header::retry_after(text, date), expected, "{text:?}");
    }
}
#[test]
fn raw_file_targets_keep_slashes_and_encode_each_other_byte() {
    let request = Request::Raw {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        path: Box::from(*b"docs/a b.md"),
        reference: id::read(&[b'0'; 40]).unwrap(),
    };
    let limits = fixtures::limits();
    let encoded = client::encode(&request, &limits).unwrap();
    assert_eq!(
        &*encoded.target,
        b"/api/v1/repos/fixture/specimen/raw/docs/a%20b.md?ref=0000000000000000000000000000000000000000"
    );
    let crate::forgejo::server::Route::Ready(routed) =
        crate::forgejo::server::route(encoded.method, &encoded.target, &limits).unwrap()
    else {
        panic!("read route has no body")
    };
    assert_eq!(routed, request);
    assert!(client::decoder(&request, &limits).is_none());
}
#[test]
fn a_job_link_of_another_shape_names_no_job() {
    let repository = b"http://127.0.0.1:3000/fixture/specimen";
    for (link, expected) in [
        (b"/fixture/specimen/actions/runs/1/jobs/0".as_slice(), Some((1, 0))),
        (b"http://127.0.0.1:3000/fixture/specimen/actions/runs/1/jobs/0", Some((1, 0))),
        (b"//127.0.0.1:3000/fixture/specimen/actions/runs/1/jobs/0", None),
        (b"http://other:3000/fixture/specimen/actions/runs/1/jobs/0", None),
        (b"/other/specimen/actions/runs/1/jobs/0", None),
        (b"/fixture/specimen/actions/runs/1/jobs/0/x", None),
        (b"https://127.0.0.1:3000/fixture/specimen/actions/runs/1/jobs/0", None),
        (b"/fixture/specimen/actions/runs/1/jobs/4294967296", None),
        (b"/fixture/specimen/actions/runs/18446744073709551616/jobs/0", None),
        (b"/fixture/specimen/actions/runs/1/jobs/0?other=1", None),
        (b"/fixture/specimen/actions/runs/1/jobs/-1", None),
    ] {
        assert_eq!(crate::forgejo::header::job_link(link, repository), expected, "{link:?}");
    }
}
#[test]
fn a_write_whose_text_is_not_utf8_is_refused_before_encoding() {
    let limits = fixtures::limits();
    let at = crate::forgejo::request::Repo { owner: Box::from(*b"a"), name: Box::from(*b"b") };
    let rows = [
        Request::CreateIssue { at: at.clone(), title: Box::from([255]), body: Box::from([]) },
        Request::PostComment { at: at.clone(), number: 1, body: Box::from([255]) },
        Request::RequestReviewers { at, number: 1, reviewers: Box::from([Box::<[u8]>::from([255u8])]) },
    ];
    for request in rows {
        assert_eq!(client::encode(&request, &limits), Err(client::Refusal::Text), "{request:?}");
    }
}
#[test]
fn an_edit_writes_only_the_fields_it_is_given() {
    type EditRow = (Option<Box<[u8]>>, Option<Box<[u8]>>, Option<crate::forgejo::document::IssueState>, &'static [u8]);
    let limits = fixtures::limits();
    let rows: [EditRow; 4] = [
        (None, None, None, b"{}"),
        (Some(Box::from(*b"title")), None, None, br#"{"title":"title"}"#),
        (None, Some(Box::from(*b"body")), None, br#"{"body":"body"}"#),
        (None, None, Some(crate::forgejo::document::IssueState::Closed), br#"{"state":"closed"}"#),
    ];
    for (title, body, state, expected) in rows {
        let request = Request::EditIssue {
            at: crate::forgejo::request::Repo { owner: Box::from(*b"a"), name: Box::from(*b"b") },
            number: 1,
            title,
            body,
            state,
        };
        assert_eq!(client::encode(&request, &limits).unwrap().body.as_deref(), Some(expected));
    }
}
#[test]
fn each_refusal_message_reads_back_from_what_the_server_answers() {
    use crate::forgejo::failure::{self, Refusal};
    for refusal in [
        Refusal::MergeClosed,
        Refusal::MergeConflict,
        Refusal::MergeProtected,
        Refusal::MergeStyle,
        Refusal::HeadOutOfDate,
        Refusal::UpdateConflict,
        Refusal::Empty,
    ] {
        let (status, message) = failure::answer(refusal);
        assert_eq!(failure::refusal(status, message), Some(refusal));
        assert_eq!(failure::refusal(status.saturating_add(1), message), None);
        let mut longer = Writer::new(message.len().saturating_add(1));
        longer.put(message).unwrap();
        longer.put(b"!").unwrap();
        let longer = longer.finish();
        assert_eq!(failure::refusal(status, &longer), None);
        let limits = fixtures::limits();
        let encoded = crate::forgejo::server::encode(
            &crate::forgejo::server::Answer::Error(crate::forgejo::document::ErrorBody {
                message: Body::Kept(Box::from(message)),
            }),
            &limits,
        )
        .unwrap();
        let decoded = fixtures::decode(client::error_decoder(&limits), &encoded, &limits);
        assert_eq!(
            decoded.document,
            Ok(Document::Error(crate::forgejo::document::ErrorBody { message: Body::Kept(Box::from(message)) }))
        );
    }
}
#[test]
fn an_error_message_keeps_its_cap_and_a_long_messages_length() {
    let limits = fixtures::small();
    let rows: [(&[u8], Body); 2] = [
        (br#"{"message":"12345678"}"#, Body::Kept(Box::from(*b"12345678"))),
        (br#"{"message":"123456789"}"#, Body::Long(9)),
    ];
    for (bytes, message) in rows {
        let decoded = fixtures::decode(client::error_decoder(&limits), bytes, &limits);
        assert_eq!(decoded.document, Ok(Document::Error(crate::forgejo::document::ErrorBody { message })));
    }
}

#[test]
fn an_empty_or_non_json_error_body_has_no_message_document() {
    let limits = fixtures::limits();
    for bytes in [b"".as_slice(), b"method not allowed"] {
        let decoded = fixtures::decode(client::error_decoder(&limits), bytes, &limits);
        assert!(decoded.document.is_err(), "no message document for {bytes:?}");
        assert!(decoded.rows.is_empty());
    }
}

#[test]
fn pull_files_decode_forgejos_changed_status_as_modified() {
    use crate::forgejo::document::{ChangedFile, FileStatus, Row};
    let limits = fixtures::limits();
    let request = Request::PullFiles {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        number: 7,
        page: crate::forgejo::request::Page { page: Some(1), limit: Some(1) },
    };
    let body = br#"[{"filename":"notes.md","status":"changed","additions":2,"deletions":1,"changes":3,"previous_filename":"","html_url":"http://127.0.0.1:3000/fixture/specimen/src/commit/head/notes.md","contents_url":"http://127.0.0.1:3000/api/v1/repos/fixture/specimen/contents/notes.md","raw_url":"http://127.0.0.1:3000/fixture/specimen/raw/commit/head/notes.md"}]"#;
    let decoded = fixtures::decode(client::decoder(&request, &limits).unwrap(), body, &limits);
    assert_eq!(decoded.document, Ok(Document::Listed { skipped: 0 }), "modified file list");
    assert_eq!(
        decoded.rows.as_slice(),
        &[Row::File(ChangedFile {
            filename: Box::from(*b"notes.md"),
            previous_filename: Some(Box::from([])),
            status: FileStatus::Modified,
        })],
        "Forgejo's changed file keeps both sides"
    );
}

fn job_request() -> Request {
    Request::Job {
        at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
        job: 2,
    }
}
fn job() -> crate::forgejo::document::Job {
    crate::forgejo::document::Job { id: 2, run_id: 1, attempt: 1, ..crate::forgejo::document::job() }
}
fn setup() -> crate::forgejo::document::Step {
    crate::forgejo::document::Step {
        number: 0,
        name: Box::from(*b"Set up job"),
        status: crate::forgejo::document::StepStatus::Success,
    }
}
#[test]
fn a_job_reads_its_steps_numbers_names_and_statuses() {
    use crate::forgejo::document::{Row, Step, StepStatus};
    let limits = fixtures::limits();
    let body = br#"{"id":2,"run_id":1,"attempt":1,"name":"fail","status":"failure","steps":[{"number":0,"name":"Set up job","status":"success","started":"2020-01-01T00:00:14+05:30"},{"number":1,"name":"exit 1\n","status":"failure"},{"number":2,"name":"Complete job","status":"failure"}]}"#;
    let decoded = fixtures::decode(client::decoder(&job_request(), &limits).unwrap(), body, &limits);
    assert_eq!(
        decoded.document,
        Ok(Document::Job(crate::forgejo::document::Job {
            name: Box::from(*b"fail"),
            status: StepStatus::Failure,
            ..job()
        }))
    );
    assert_eq!(
        decoded.rows.as_slice(),
        [
            Row::Step(setup()),
            Row::Step(Step { number: 1, name: Box::from(*b"exit 1\n"), status: StepStatus::Failure }),
            Row::Step(Step { number: 2, name: Box::from(*b"Complete job"), status: StepStatus::Failure }),
        ],
        "each step goes up as a row, in Forgejo's order"
    );
}
#[test]
fn a_job_without_a_steps_member_reads_no_steps() {
    let limits = fixtures::limits();
    let body = br#"{"id":4,"run_id":2,"attempt":0,"status":"waiting"}"#;
    let decoded = fixtures::decode(client::decoder(&job_request(), &limits).unwrap(), body, &limits);
    assert_eq!(
        decoded.document,
        Ok(Document::Job(crate::forgejo::document::Job {
            id: 4,
            run_id: 2,
            attempt: 0,
            status: crate::forgejo::document::StepStatus::Waiting,
            ..crate::forgejo::document::job()
        }))
    );
    assert_eq!(decoded.rows.as_slice(), []);
}
#[test]
fn a_job_encodes_its_steps_only_when_it_has_some() {
    use crate::forgejo::server::{self, Answer};
    let limits = fixtures::limits();
    for (steps, expected) in [
        (
            Box::from([setup()]),
            br#"{"id":2,"run_id":1,"attempt":1,"steps":[{"number":0,"name":"Set up job","status":"success"}]}"#
                .as_slice(),
        ),
        (Box::from([]), br#"{"id":2,"run_id":1,"attempt":1}"#),
    ] {
        assert_eq!(server::encode(&Answer::Job { job: job(), steps }, &limits).unwrap().as_ref(), expected);
    }
}
#[test]
fn each_step_status_reads_back_from_the_servers_word() {
    use crate::forgejo::document::{Row, StepStatus};
    let limits = fixtures::limits();
    for status in [
        StepStatus::Unknown,
        StepStatus::Waiting,
        StepStatus::Running,
        StepStatus::Success,
        StepStatus::Failure,
        StepStatus::Cancelled,
        StepStatus::Skipped,
        StepStatus::Blocked,
    ] {
        let step = crate::forgejo::document::Step { status, ..setup() };
        let answer = crate::forgejo::server::Answer::Job { job: job(), steps: Box::from([step.clone()]) };
        let body = crate::forgejo::server::encode(&answer, &limits).unwrap();
        let decoded = fixtures::decode(client::decoder(&job_request(), &limits).unwrap(), &body, &limits);
        assert_eq!(decoded.document, Ok(Document::Job(job())), "{status:?}");
        assert_eq!(decoded.rows.as_slice(), [Row::Step(step)], "{status:?}");
    }
}
#[test]
fn a_step_status_outside_forgejos_words_is_malformed() {
    let limits = fixtures::limits();
    let body = br#"{"id":2,"run_id":1,"attempt":1,"steps":[{"number":0,"name":"x","status":"done"}]}"#;
    let decoded = fixtures::decode(client::decoder(&job_request(), &limits).unwrap(), body, &limits);
    assert_eq!(decoded.document, Err(Malformed::Value { field: b"status" }));
}
#[test]
fn a_job_reads_more_steps_than_inner_as_rows() {
    let limits = crate::forgejo::limits::Limits { inner: 2, ..fixtures::limits() };
    let body = br#"{"id":2,"run_id":1,"attempt":1,"steps":[{"number":0,"name":"a","status":"success"},{"number":1,"name":"b","status":"failure"},{"number":2,"name":"c","status":"failure"}]}"#;
    let decoded = fixtures::decode(client::decoder(&job_request(), &limits).unwrap(), body, &limits);
    assert_eq!(decoded.document, Ok(Document::Job(job())));
    assert_eq!(decoded.rows.len(), 3);
}
#[test]
fn a_step_name_past_string_bytes_reads_empty() {
    use crate::forgejo::document::{Row, Step, StepStatus};
    let limits = crate::forgejo::limits::Limits { string_bytes: 8, ..fixtures::limits() };
    let body = br#"{"id":2,"run_id":1,"attempt":1,"steps":[{"number":0,"name":"12345678","status":"success"},{"number":1,"name":"123456789","status":"failure"}]}"#;
    let decoded = fixtures::decode(client::decoder(&job_request(), &limits).unwrap(), body, &limits);
    assert_eq!(decoded.document, Ok(Document::Job(job())));
    assert_eq!(
        decoded.rows.as_slice(),
        [
            Row::Step(Step { number: 0, name: Box::from(*b"12345678"), status: StepStatus::Success }),
            Row::Step(Step { number: 1, name: Box::from([]), status: StepStatus::Failure }),
        ]
    );
}

#[test]
fn a_comparison_sends_verification_false_and_files_false_when_asked() {
    let limits = fixtures::limits();
    let before = id::read(b"0100000000000000000000000000000000000000").unwrap();
    let after = id::read(b"0300000000000000000000000000000000000000").unwrap();
    let targets: [(bool, &[u8]); 2] = [
        (true, b"/api/v1/repos/fixture/specimen/compare/0100000000000000000000000000000000000000...0300000000000000000000000000000000000000?verification=false"),
        (false, b"/api/v1/repos/fixture/specimen/compare/0100000000000000000000000000000000000000...0300000000000000000000000000000000000000?files=false&verification=false"),
    ];
    for (files, target) in targets {
        let request = Request::Compare {
            at: crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") },
            before,
            after,
            files,
        };
        let encoded = client::encode(&request, &limits).unwrap();
        assert_eq!(encoded.target.as_ref(), target, "{files}");
        let crate::forgejo::server::Route::Ready(routed) =
            crate::forgejo::server::route(encoded.method, &encoded.target, &limits).unwrap()
        else {
            panic!("A comparison routes without a body");
        };
        assert_eq!(routed, request, "{files}");
    }
}

#[test]
fn an_empty_range_over_an_empty_log_reads_as_empty() {
    use crate::forgejo::header::{ContentRange, content_range};
    for (text, expected) in [
        (b"bytes 0--1/0".as_slice(), Some(ContentRange { first: 0, end: 0, total: Some(0) })),
        (b"bytes 0--1/2", None),
        (b"bytes 1--1/0", None),
    ] {
        assert_eq!(content_range(text), expected, "{text:?}");
    }
}

mod tokens;
