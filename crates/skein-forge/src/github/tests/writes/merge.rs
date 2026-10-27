use super::*;
use crate::github::{failure, plans::tables::Method as MergeMethod, server};
use skein_lib::{Overflow, Writer};

const QUEUE_RULE: &[u8] = b"Repository rule violations found\nChanges must be made through the merge queue";
const NODE: &[u8] = b"opaque+/=node";
#[expect(clippy::disallowed_methods, reason = "the request-body fixture mutates the encoder's UTF-8 output")]
fn text(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap()
}
fn merge() -> api::Write {
    api::Write::Merge { number: 5, head: commit(), how: api::MergeHow::Style }
}
fn limits() -> crate::github::limits::Limits {
    let mut limits = super::limits();
    limits.body_bytes = 4096;
    limits.json.string = 4096;
    limits
}
fn rest(method: MergeMethod) -> Request {
    Request::Merge { owner: Box::from(*b"acme"), name: Box::from(*b"widgets"), number: 5, head: commit(), method }
}
fn query() -> Request {
    Request::PullGraphql { owner: Box::from(*b"acme"), name: Box::from(*b"widgets"), number: 5 }
}
fn enqueue() -> Request {
    Request::Enqueue { id: Box::from(NODE), head: commit() }
}
fn queue_facts(queued: Option<u32>) -> document::PullGraphql {
    document::PullGraphql { id: Box::from(NODE), merge_commit: None, queued, dequeued: None }
}
fn sent(result: Result<Option<Step>, api::Error>) -> Request {
    let Some(Step::Send(exchange)) = result.unwrap() else { panic!("request") };
    exchange.request
}
fn start() -> (Plan, Tables) {
    let mut tables = cached_tables();
    let mut plan = write_plan(merge());
    assert_eq!(begin(&mut plan, &mut tables), rest(MergeMethod::Merge));
    (plan, tables)
}
fn queue_query() -> (Plan, Tables) {
    let (mut plan, mut tables) = start();
    head(&mut plan, 405, &mut tables);
    assert_eq!(sent(finish(&mut plan, refusal(QUEUE_RULE), &mut tables)), query());
    (plan, tables)
}
fn queue_mutation() -> (Plan, Tables) {
    let (mut plan, mut tables) = queue_query();
    head(&mut plan, 200, &mut tables);
    assert_eq!(
        sent(finish(&mut plan, Body::Document(Document::PullGraphql(queue_facts(None))), &mut tables)),
        enqueue()
    );
    (plan, tables)
}
fn wire(request: &Request, method: Method, target: &[u8], body: &[u8], kind: Kind) {
    let encoded = request::encode(request, &limits()).unwrap();
    assert_eq!(encoded.method, method);
    assert_eq!(encoded.target.as_ref(), target);
    assert_eq!(encoded.body.as_deref(), Some(body));
    assert_eq!(encoded.auth, Auth::Installation);
    assert_eq!(
        request::fields(&encoded).as_ref(),
        [
            (b"Accept".as_slice(), b"application/vnd.github+json".as_slice()),
            (b"X-GitHub-Api-Version", crate::github::pin::API_VERSION.as_bytes())
        ]
    );
    assert_eq!(decoding::kind(request), Some(kind));
}
#[test]
fn github_merge_rest_commit_and_405s() {
    for (method, spelling) in
        [(MergeMethod::Merge, "merge"), (MergeMethod::Squash, "squash"), (MergeMethod::Rebase, "rebase")]
    {
        let mut tables = cached_tables();
        let mut row = tables.repository(KEY).unwrap().clone();
        row.merge = Some(method);
        tables.keep_repository(KEY, row);
        let mut plan = write_plan(merge());
        let request = begin(&mut plan, &mut tables);
        assert_eq!(request, rest(method));
        let body = alloc::fmt::format(format_args!(
            r#"{{"sha":"0123456789abcdef0123456789abcdef01234567","merge_method":"{spelling}"}}"#
        ));
        wire(&request, Method::Put, b"/repos/acme/widgets/pulls/5/merge", body.as_bytes(), Kind::Merge);
        assert_eq!(server::write_body(request.clone(), body.as_bytes(), &limits()), Some(request.clone()));
        encoded_body_bounds(&request);
        head(&mut plan, 200, &mut tables);
        let landed = update_pull().base_commit;
        assert_eq!(
            finish(&mut plan, Body::Document(Document::Merge(landed)), &mut tables),
            ended(Ok(api::Answer::Merged(landed)))
        );
        assert_eq!(finish(&mut plan, Body::Empty, &mut tables), Err(api::Error::InvalidAnswer));
    }
    for (status, message, error, drop) in [
        (409, failure::MERGE_STALE, api::Error::Stale, false),
        (409, b"unknown", api::Error::Refused, false),
        (405, failure::NOT_MERGEABLE, api::Error::Conflict, false),
        (
            405,
            b"Repository rule violations found\nAt least one approving review is required",
            api::Error::Protected,
            false,
        ),
        (405, b"Squash merges are not allowed on this repository.", api::Error::Refused, true),
        (405, b"Changes must be made through the merge queue", api::Error::Refused, false),
        (405, b"unknown", api::Error::Refused, false),
        (422, b"unknown", api::Error::Refused, false),
    ] {
        let (mut plan, mut tables) = start();
        head(&mut plan, status, &mut tables);
        assert_eq!(finish(&mut plan, refusal(message), &mut tables), ended(Err(error)));
        assert_eq!(tables.repository(KEY).is_none(), drop);
        assert_eq!(finish(&mut plan, Body::Empty, &mut tables), Err(api::Error::InvalidAnswer));
    }
    let mut tables = cached_tables();
    let mut row = tables.repository(KEY).unwrap().clone();
    row.merge = None;
    tables.keep_repository(KEY, row);
    assert_eq!(write_plan(merge()).begin(bounds(), WORD, &mut tables), Err(api::Error::Refused));
}
#[test]
fn github_merge_queue_and_stale() {
    let (mut plan, mut tables) = queue_mutation();
    let envelope = alloc::fmt::format(format_args!(
        r#"{{"query":"{}","variables":{{"id":"opaque+/=node","head":"0123456789abcdef0123456789abcdef01234567"}}}}"#,
        "mutation($id: ID!, $head: GitObjectID!) { enqueuePullRequest(input: {pullRequestId: $id, expectedHeadOid: $head}) { mergeQueueEntry { position } } }"
    ));
    wire(&enqueue(), Method::Post, b"/graphql", envelope.as_bytes(), Kind::Enqueue);
    assert_eq!(server::write_body(enqueue(), envelope.as_bytes(), &limits()), Some(enqueue()));
    let encoded = request::encode(&query(), &limits()).unwrap();
    wire(&query(), Method::Post, b"/graphql", encoded.body.as_deref().unwrap(), Kind::PullGraphql);
    head(&mut plan, 200, &mut tables);
    assert_eq!(finish(&mut plan, Body::Document(Document::Enqueued(3)), &mut tables), ended(Ok(api::Answer::Queued)));
    assert_eq!(finish(&mut plan, Body::Document(Document::Enqueued(3)), &mut tables), Err(api::Error::InvalidAnswer));
    for chunk in [1, 7] {
        for (errors, expected) in [
            (
                r#"{"type":"UNPROCESSABLE","message":"Pull request expected head oid does not match the current head oid"}"#,
                api::Error::Stale,
            ),
            (
                r#"{"type":"OTHER","message":"expected head oid does not match the current head oid"}"#,
                api::Error::Refused,
            ),
            (r#"{"type":"UNPROCESSABLE","message":"different"}"#, api::Error::Refused),
            (
                r#"{"type":"UNPROCESSABLE","message":"expected head oid does not match the current head oid"},{"type":"OTHER","message":"different"}"#,
                api::Error::Refused,
            ),
            (
                r#"{"type":"OTHER","message":"different"},{"type":"UNPROCESSABLE","message":"expected head oid does not match the current head oid"}"#,
                api::Error::Refused,
            ),
            (
                r#"{"type":"UNPROCESSABLE","message":"expected head oid does not match the current head oid"},{"type":"UNPROCESSABLE","message":"expected head oid does not match the current head oid"}"#,
                api::Error::Stale,
            ),
        ] {
            for data in ["null", r#"{"enqueuePullRequest":{"mergeQueueEntry":{"position":3}}}"#] {
                let body = alloc::fmt::format(format_args!(r#"{{"data":{data},"errors":[{errors}]}}"#));
                let document = decode(Kind::Enqueue, body.as_bytes(), chunk, &limits()).1.unwrap();
                let (mut plan, mut tables) = queue_mutation();
                head(&mut plan, 200, &mut tables);
                assert_eq!(finish(&mut plan, Body::Document(document), &mut tables), ended(Err(expected)));
            }
        }
    }
}
#[test]
fn github_server_enqueue_refusal_round_trips() {
    for (kind, message, expected) in [
        (
            b"UNPROCESSABLE".as_slice(),
            b"Pull request At least 1 approving review is required by reviewers with write access.".as_slice(),
            br#"{"data":{"enqueuePullRequest":null},"errors":[{"type":"UNPROCESSABLE","path":["enqueuePullRequest"],"message":"Pull request At least 1 approving review is required by reviewers with write access."}]}"#.as_slice(),
        ),
        (
            b"OTHER",
            b"refused \"review\"\\\n",
            br#"{"data":{"enqueuePullRequest":null},"errors":[{"type":"OTHER","path":["enqueuePullRequest"],"message":"refused \"review\"\\\n"}]}"#,
        ),
    ] {
        let mut out = Writer::new(expected.len());
        server::enqueue_refused(kind, message, &mut out).unwrap();
        let body = super::super::fixtures::bytes(out);
        assert_eq!(body.as_ref(), expected);
        for chunk in [1, 7] {
            let (rows, document) = decode(Kind::Enqueue, &body, chunk, &limits());
            assert_eq!(rows.len(), 0);
            assert_eq!(
                document,
                Ok(Document::Graphql(document::GraphqlRefusal { kind: Box::from(kind), message: Box::from(message) }))
            );
        }
        let mut short = Writer::new(body.len() - 1);
        assert_eq!(server::enqueue_refused(kind, message, &mut short), Err(Overflow));
    }
}
#[test]
fn github_enqueue_reads_the_recorded_refusal_whole() {
    // live session 1917's body, verbatim, with the fields the fake leaves out.
    let body = br#"{"data": {"enqueuePullRequest": null}, "errors": [{"type": "UNPROCESSABLE", "path": ["enqueuePullRequest"], "locations": [{"line": 1, "column": 39}], "message": "Pull request At least 1 approving review is required by reviewers with write access."}]}"#;
    for chunk in [1, 7, 4096] {
        let (rows, document) = decode(Kind::Enqueue, body, chunk, &limits());
        assert_eq!(rows.len(), 0);
        assert_eq!(
            document,
            Ok(Document::Graphql(document::GraphqlRefusal {
                kind: Box::from(*b"UNPROCESSABLE"),
                message: Box::from(
                    *b"Pull request At least 1 approving review is required by reviewers with write access."
                ),
            }))
        );
    }
}
#[test]
fn github_merge_graphql_bounds_and_errors() {
    for request in [enqueue(), Request::Enqueue { id: Box::from(*b"opaque\"\\node"), head: commit() }, query()] {
        let encoded = request::encode(&request, &limits()).unwrap();
        let size = u32::try_from(encoded.body.as_ref().unwrap().len()).unwrap();
        for (body_bytes, ok) in [(size, true), (size - 1, false)] {
            assert_eq!(
                request::encode(&request, &crate::github::limits::Limits { body_bytes, ..limits() }).is_ok(),
                ok
            );
        }
        for (target_bytes, ok) in [(8, true), (7, false)] {
            assert_eq!(
                request::encode(&request, &crate::github::limits::Limits { target_bytes, ..limits() }).is_ok(),
                ok
            );
        }
        if let Request::Enqueue { .. } = request {
            assert_eq!(server::write_body(request.clone(), encoded.body.as_deref().unwrap(), &limits()), Some(request));
        }
    }
    for (string_bytes, ok) in [(13, true), (12, false)] {
        let l = crate::github::limits::Limits { string_bytes, ..limits() };
        assert_eq!(request::encode(&enqueue(), &l).is_ok(), ok);
        let encoded = request::encode(&enqueue(), &limits()).unwrap();
        assert_eq!(server::write_body(enqueue(), encoded.body.as_deref().unwrap(), &l).is_some(), ok);
    }
    for chunk in [1, 7] {
        for position in [0, 3, u32::MAX] {
            let mut out = Writer::new(4096);
            server::enqueued(position, &mut out).unwrap();
            let body = super::super::fixtures::bytes(out);
            assert_eq!(decode(Kind::Enqueue, &body, chunk, &limits()).1, Ok(Document::Enqueued(position)));
            let mut exact = Writer::new(body.len());
            server::enqueued(position, &mut exact).unwrap();
            let mut short = Writer::new(body.len() - 1);
            assert_eq!(server::enqueued(position, &mut short), Err(Overflow));
        }
        for body in [
            b"{}".as_slice(),
            br#"{"data":null}"#,
            br#"{"data":{"enqueuePullRequest":null}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":null}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{}}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":4294967296}}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":-1}}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":"3"}}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":null}}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":3,"position":4}}}}"#,
            br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":3}}},"data":null}"#,
            br#"{"errors":[]}"#,
            br#"{"errors":[{"type":"OTHER"}]}"#,
            br#"{"errors":[{"type":"OTHER","message":"no","message":"yes"}]}"#,
        ] {
            let error = decode(Kind::Enqueue, body, chunk, &limits()).1.unwrap_err();
            let (mut plan, mut tables) = queue_mutation();
            head(&mut plan, 200, &mut tables);
            assert_eq!(finish(&mut plan, Body::Malformed(error), &mut tables), ended(Err(api::Error::Timeout)));
        }
        let body = br#"{"unknown":{"nested":[1,true]},"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":3,"other":[]}}}}"#;
        assert_eq!(decode(Kind::Enqueue, body, chunk, &limits()).1, Ok(Document::Enqueued(3)));
    }
    let (mut plan, mut tables) = queue_query();
    head(&mut plan, 200, &mut tables);
    assert_eq!(
        finish(
            &mut plan,
            Body::Document(Document::Graphql(document::GraphqlRefusal {
                kind: Box::from(*b"OTHER"),
                message: Box::from(*b"no")
            })),
            &mut tables
        ),
        ended(Err(api::Error::Unavailable))
    );
    for body in [
        Body::Empty,
        Body::Document(Document::Done),
        Body::Malformed(decoding::Malformed::Missing { field: b"id" }),
        Body::Document(Document::PullGraphql(document::PullGraphql { id: Box::from([]), ..queue_facts(None) })),
    ] {
        let (mut plan, mut tables) = queue_query();
        head(&mut plan, 200, &mut tables);
        assert_eq!(finish(&mut plan, body, &mut tables), ended(Err(api::Error::Unavailable)));
    }
}
#[test]
fn github_merge_uncertain_and_rules_changed() {
    for stage in [0u8, 1, 2] {
        for failed in [Failure::Unseen, Failure::Seen] {
            let (mut plan, mut tables) = match stage {
                0 => start(),
                1 => queue_query(),
                _ => queue_mutation(),
            };
            let expected =
                if stage == 1 || failed == Failure::Unseen { api::Error::Unavailable } else { api::Error::Timeout };
            assert_eq!(plan.heard(Heard::Failed(failed), bounds(), WORD, &mut tables), ended(Err(expected)));
            assert_eq!(plan.heard(Heard::Failed(failed), bounds(), WORD, &mut tables), Err(api::Error::InvalidAnswer));
        }
        for status in [201, 500, 401, 404, 301] {
            let (mut plan, mut tables) = match stage {
                0 => start(),
                1 => queue_query(),
                _ => queue_mutation(),
            };
            let result = plan.heard(Heard::Head { status, head: Head::new() }, bounds(), WORD, &mut tables);
            let expected = match status {
                401 => api::Error::Forbidden,
                404 => api::Error::Missing,
                301 => api::Error::Unavailable,
                _ => {
                    if stage == 1 {
                        api::Error::Unavailable
                    } else {
                        api::Error::Timeout
                    }
                }
            };
            if status == 201 {
                assert_eq!(result, ended(Err(expected)));
            } else {
                assert_eq!(result, Ok(None));
                assert_eq!(finish(&mut plan, refusal(b"unknown"), &mut tables), ended(Err(expected)));
            }
        }
    }
    // Queue and additional rule lines still follow the classifier's queue route.
    let (mut first, mut tables) = start();
    head(&mut first, 405, &mut tables);
    assert_eq!(
        sent(finish(
            &mut first,
            refusal(b"Repository rule violations found\nAnother rule\nChanges must be made through the merge queue"),
            &mut tables
        )),
        query()
    );
    head(&mut first, 200, &mut tables);
    assert_eq!(
        sent(finish(&mut first, Body::Document(Document::PullGraphql(queue_facts(None))), &mut tables)),
        enqueue()
    );
    head(&mut first, 200, &mut tables);
    assert_eq!(finish(&mut first, Body::Document(Document::Enqueued(2)), &mut tables), ended(Ok(api::Answer::Queued)));
    let ejected = observed_pull(&mut tables, false, None, Some(b"rules changed"), 7);
    assert_eq!(ejected.queued, None);
    // A subsequent effect asks REST again after ejection and a rules change.
    let mut later = write_plan(merge());
    assert_eq!(begin(&mut later, &mut tables), rest(MergeMethod::Merge));
    head(&mut later, 200, &mut tables);
    assert_eq!(
        finish(&mut later, Body::Document(Document::Merge(commit())), &mut tables),
        ended(Ok(api::Answer::Merged(commit())))
    );
}

#[test]
fn github_merge_rest_documents_and_request_bounds() {
    for chunk in [1, 7] {
        let landed = update_pull().base_commit;
        let mut out = Writer::new(4096);
        server::merged(&landed, &mut out).unwrap();
        let body = super::super::fixtures::bytes(out);
        assert_eq!(decode(Kind::Merge, &body, chunk, &limits()).1, Ok(Document::Merge(landed)));
        let mut exact = Writer::new(body.len());
        server::merged(&landed, &mut exact).unwrap();
        let mut short = Writer::new(body.len() - 1);
        assert_eq!(server::merged(&landed, &mut short), Err(Overflow));
        for body in [
            b"{}".as_slice(),
            b"null",
            br#"{"sha":null}"#,
            br#"{"sha":3}"#,
            br#"{"sha":"0123456789abcdef0123456789abcdef01234567","sha":"0123456789abcdef0123456789abcdef01234567"}"#,
            br#"{"sha":"0123456789ABCDEF0123456789abcdef01234567"}"#,
            br#"{"sha":"0123456789abcdef0123456789abcdef0123456"}"#,
        ] {
            let error = decode(Kind::Merge, body, chunk, &limits()).1.unwrap_err();
            let (mut plan, mut tables) = start();
            head(&mut plan, 200, &mut tables);
            assert_eq!(finish(&mut plan, Body::Malformed(error), &mut tables), ended(Err(api::Error::Timeout)));
        }
        let good = br#"{"sha":"0123456789abcdef0123456789abcdef01234567","unknown":{"nested":[true,3]}}"#;
        assert_eq!(decode(Kind::Merge, good, chunk, &limits()).1, Ok(Document::Merge(commit())));
    }
    let request = rest(MergeMethod::Merge);
    let encoded = request::encode(&request, &limits()).unwrap();
    let target_len = u32::try_from(encoded.target.len()).unwrap();
    for (target_bytes, ok) in [(target_len, true), (target_len - 1, false)] {
        assert_eq!(request::encode(&request, &crate::github::limits::Limits { target_bytes, ..limits() }).is_ok(), ok);
    }
    let path = Request::Merge {
        owner: Box::from(*b"acme x"),
        name: Box::from(*b"widgets/y"),
        number: NUMBER,
        head: commit(),
        method: MergeMethod::Merge,
    };
    assert_eq!(
        request::encode(&path, &limits()).unwrap().target.as_ref(),
        b"/repos/acme%20x/widgets%2Fy/pulls/9007199254740993/merge"
    );
    let mut bad = request.clone();
    if let Request::Merge { head, .. } = &mut bad {
        *head = [1; 32];
    }
    assert_eq!(request::encode(&bad, &limits()), Err(request::Refusal::Text));
    for body in [
        br#"{"sha":"0123456789abcdef0123456789abcdef01234567"}"#.as_slice(),
        br#"{"sha":"0123456789abcdef0123456789abcdef01234567","merge_method":"unknown"}"#,
        br#"{"sha":"0123456789abcdef0123456789abcdef01234567","merge_method":"merge","merge_method":"squash"}"#,
    ] {
        assert_eq!(server::write_body(request.clone(), body, &limits()), None);
    }
    let mut tables = Tables::new(0, 0);
    let mut plan = write_plan(merge());
    assert_eq!(begin(&mut plan, &mut tables), Request::Repository { id: 7 });
    head(&mut plan, 200, &mut tables);
    assert_eq!(
        sent(finish(&mut plan, Body::Document(Document::Repository(repository())), &mut tables)),
        rest(MergeMethod::Merge)
    );
    assert_eq!(tables.repository(KEY), None);
}

#[test]
fn github_merge_enqueue_body_malformed_and_limits() {
    let envelope = request::encode(&enqueue(), &limits()).unwrap().body.unwrap();
    let size = u32::try_from(envelope.len()).unwrap();
    for (body_bytes, ok) in [(size, true), (size - 1, false)] {
        assert_eq!(
            server::write_body(enqueue(), &envelope, &crate::github::limits::Limits { body_bytes, ..limits() })
                .is_some(),
            ok
        );
    }
    for body in [
        b"{}".as_slice(),
        br#"{"query":"other","variables":{"id":"opaque","head":"0123456789abcdef0123456789abcdef01234567"}}"#,
    ] {
        assert_eq!(server::write_body(enqueue(), body, &limits()), None);
    }
    let text = text(&envelope);
    for malformed in [
        text.replace(r#""id":"opaque+/=node""#, r#""id":null"#),
        text.replace(r#""id":"opaque+/=node""#, r#""id":"opaque","id":"other""#),
        text.replace(r#""id":"opaque+/=node","#, ""),
        text.replace(r#""head":"0123456789abcdef0123456789abcdef01234567""#, r#""head":true"#),
        text.replace(
            r#""variables":{"id":"opaque+/=node","head":"0123456789abcdef0123456789abcdef01234567"}"#,
            r#""variables":{},"id":"opaque+/=node","head":"0123456789abcdef0123456789abcdef01234567""#,
        ),
    ] {
        if malformed.as_bytes() != envelope.as_ref() {
            assert_eq!(server::write_body(enqueue(), malformed.as_bytes(), &limits()), None);
        }
    }
    let extended = text.replace(r#""variables":{"#, r#""unknown":{"nested":[true]},"variables":{"unused":[],"#);
    assert_eq!(server::write_body(enqueue(), extended.as_bytes(), &limits()), Some(enqueue()));
    let errors = br#"{"errors":[{"type":"OTHER","message":"no"},{"type":"OTHER","message":"no"}]}"#;
    for (inner, ok) in [(2, true), (1, false)] {
        assert_eq!(
            decode(Kind::Enqueue, errors, 1, &crate::github::limits::Limits { inner, ..limits() }).1.is_ok(),
            ok
        );
    }
    for (string_bytes, ok) in [(5, true), (4, false)] {
        assert_eq!(
            decode(Kind::Enqueue, errors, 7, &crate::github::limits::Limits { string_bytes, ..limits() }).1.is_ok(),
            ok
        );
    }
}

#[expect(clippy::manual_map, reason = "the fixture spells out optional copies without closure combinators")]
fn observed_pull(
    tables: &mut Tables,
    merged: bool,
    queued: Option<u32>,
    reason: Option<&[u8]>,
    chunk: u32,
) -> api::Pull {
    let mut plan = Plan::new(KEY, api::Op::Read(api::Read::Pull { number: 5 }));
    assert_eq!(
        begin(&mut plan, tables),
        Request::Pull { owner: Box::from(*b"acme"), name: Box::from(*b"widgets"), number: 5 }
    );
    let mut pull = update_pull();
    pull.number = 5;
    pull.closed = merged;
    pull.merged = merged;
    pull.merge_commit = None;
    let mut out = Writer::new(4096);
    server::pull(&pull, &mut out).unwrap();
    head(&mut plan, 200, tables);
    let doc = decode(Kind::Pull, &super::super::fixtures::bytes(out), chunk, &limits()).1.unwrap();
    let Request::CombinedStatus { commit: observed, .. } = sent(finish(&mut plan, Body::Document(doc), tables)) else {
        panic!("combined")
    };
    assert_eq!(observed, commit());
    head(&mut plan, 200, tables);
    let doc = decode(Kind::CombinedStatus, br#"{"state":"pending","total_count":0,"statuses":[]}"#, chunk, &limits())
        .1
        .unwrap();
    let Request::CheckRuns { commit: observed, .. } = sent(finish(&mut plan, Body::Document(doc), tables)) else {
        panic!("runs")
    };
    assert_eq!(observed, commit());
    head(&mut plan, 200, tables);
    assert_eq!(sent(finish(&mut plan, Body::Document(Document::Rows), tables)), query());
    let facts = document::PullGraphql {
        id: Box::from(NODE),
        merge_commit: if merged { Some(update_pull().base_commit) } else { None },
        queued,
        dequeued: match reason {
            Some(reason) => Some(Box::from(reason)),
            None => None,
        },
    };
    let mut out = Writer::new(4096);
    server::pull_graphql(&facts, &mut out).unwrap();
    head(&mut plan, 200, tables);
    let doc = decode(Kind::PullGraphql, &super::super::fixtures::bytes(out), chunk, &limits()).1.unwrap();
    let Some(Step::Done(Ok(api::Answer::Pull(pull)))) = finish(&mut plan, Body::Document(doc), tables).unwrap() else {
        panic!("fresh pull")
    };
    pull
}
#[test]
fn github_merge_lost_enqueue_adopts_and_polls() {
    for chunk in [1, 7] {
        let (mut plan, mut tables) = queue_mutation();
        assert_eq!(
            plan.heard(Heard::Failed(Failure::Seen), bounds(), WORD, &mut tables),
            ended(Err(api::Error::Timeout))
        );
        let pull = observed_pull(&mut tables, false, Some(3), None, chunk);
        assert_eq!(pull.queued, Some(3));
        assert_eq!(pull.commit, commit());
        assert_eq!(pull.state, api::State::Open);
        // The client adopts the observed entry and uses separate Pull calls until it lands or is ejected.
        let pull = observed_pull(&mut tables, false, None, Some(b"checks failed"), chunk);
        assert_eq!(pull.queued, None);
        assert_eq!(pull.dequeued.as_deref(), Some(b"checks failed".as_slice()));
        let pull = observed_pull(&mut tables, true, None, None, chunk);
        assert_eq!(pull.state, api::State::Closed);
        assert_eq!(pull.merged, Some(update_pull().base_commit));
        assert_eq!(pull.commit, commit());
        assert_eq!(finish(&mut plan, Body::Empty, &mut tables), Err(api::Error::InvalidAnswer));
    }
}
#[test]
fn github_merge_rest_lost_and_heard_proof() {
    for lost in [false, true] {
        let (mut plan, mut tables) = start();
        if lost {
            assert_eq!(
                plan.heard(Heard::Failed(Failure::Seen), bounds(), WORD, &mut tables),
                ended(Err(api::Error::Timeout))
            );
        } else {
            head(&mut plan, 200, &mut tables);
            assert_eq!(
                finish(&mut plan, Body::Document(Document::Merge(update_pull().base_commit)), &mut tables),
                ended(Ok(api::Answer::Merged(update_pull().base_commit)))
            );
        }
        let pull = observed_pull(&mut tables, true, None, None, 7);
        assert_eq!(pull.state, api::State::Closed);
        assert_eq!(pull.commit, commit());
        assert_eq!(pull.merged, Some(update_pull().base_commit));
        assert_eq!(pull.base_commit, Some(update_pull().base_commit));
    }
}

#[test]
fn github_merge_order_and_mismatched_documents() {
    for stage in [0u8, 1, 2] {
        let (mut plan, mut tables) = match stage {
            0 => start(),
            1 => queue_query(),
            _ => queue_mutation(),
        };
        assert_eq!(finish(&mut plan, Body::Empty, &mut tables), Err(api::Error::InvalidAnswer));
        head(&mut plan, 200, &mut tables);
        assert_eq!(
            plan.heard(Heard::Head { status: 200, head: Head::new() }, bounds(), WORD, &mut tables),
            Err(api::Error::InvalidAnswer)
        );
        assert_eq!(
            finish(&mut plan, Body::Document(Document::Number(99)), &mut tables),
            ended(Err(if stage == 1 { api::Error::Unavailable } else { api::Error::Timeout }))
        );
    }
    let (mut plan, mut tables) = start();
    let Step::Send(exchange) = Step::send(rest(MergeMethod::Merge)) else { panic!("exchange") };
    plan.set_pending(exchange.clone());
    assert_eq!(plan.pending(), Some(exchange));
    assert_eq!(plan.pending(), None);
    assert_eq!(plan.begin(bounds(), WORD, &mut tables), Err(api::Error::InvalidAnswer));
}
