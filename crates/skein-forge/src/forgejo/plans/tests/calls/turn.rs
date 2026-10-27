//! Turn calls through request spelling, server documents and streamed decoding.
use super::*;
use crate::api::{MergeHow, UpdateStyle, Write};
use crate::forgejo::{client, document as d, request::Request, server};
use alloc::boxed::Box;
// Two full run documents and their listing envelope fit within this fixture's cap.
const TURN_DOCUMENT_BYTES: u32 = 1024;
fn limits() -> crate::forgejo::limits::Limits {
    let mut limits = fixtures::limits();
    limits.writer.length = TURN_DOCUMENT_BYTES;
    limits.json.length = TURN_DOCUMENT_BYTES;
    limits
}
fn known(op: Op) -> Run {
    Run::with_limits(limits(), BOUNDS, op)
}
fn wire_run() -> d::Run {
    let limits = limits();
    let decoded = fixtures::decode(
        client::decoder(&Request::Run { at: fixtures::repository(), run: 7 }, &limits).expect("run decoder"),
        RUN_DOC,
        &limits,
    );
    let Ok(d::Document::Run(run)) = decoded.document else {
        panic!("run document");
    };
    run
}
const RUN_DOC: &[u8] = br#"{"id":7,"index_in_repo":5,"workflow_id":"ci.yaml","commit_sha":"0100000000000000000000000000000000000000","event":"workflow_dispatch","trigger_actor":{"id":2},"status":"failure","html_url":"https://forge/fixture/specimen/actions/runs/5","created_at":"1970-01-01T00:00:03Z"}"#;
const JOBS_DOC: &[u8] = br#"[{"id":9,"run_id":7,"attempt":1,"name":"passed","status":"success"},{"id":10,"run_id":7,"attempt":3,"name":"failed","status":"failure"}]"#;
const TREE_DOC: &[u8] = br#"{"tree":[{"path":"a","mode":"100644","type":"blob","sha":"0100000000000000000000000000000000000000"},{"path":"b","mode":"040000","type":"tree","sha":"0300000000000000000000000000000000000000"}],"truncated":false}"#;
fn run_value() -> api::Run {
    api::Run {
        id: 7,
        number: 5,
        workflow: Box::from(*b"ci.yaml"),
        commit: HEAD,
        event: api::Event::Dispatch,
        actor: 2,
        state: api::RunState::Completed,
        conclusion: Some(api::Conclusion::Failure),
        url: Box::from(*b"https://forge/fixture/specimen/actions/runs/5"),
        created: Time::from_nanos(3_000_000_000),
        jobs: Box::new([]),
    }
}
fn round_request(step: Step) -> Request {
    let Step::Send(exchange) = step else {
        panic!("request expected");
    };
    let limits = limits();
    let encoded = client::encode(&exchange.request, &limits).expect("request fits");
    let decoded = match server::route(encoded.method, &encoded.target, &limits).expect("server route") {
        server::Route::Ready(request) => request,
        server::Route::Body(_) => {
            fixtures::request(encoded.method, &encoded.target, encoded.body.as_deref().expect("request body"), &limits)
                .expect("server body")
        }
    };
    assert_eq!(decoded, exchange.request);
    decoded
}
fn server_answer(answer: &server::Answer) -> Box<[u8]> {
    server::encode(answer, &limits()).expect("server answer fits")
}
fn dispatch() -> Write {
    Write::Dispatch {
        workflow: Box::from(*b"ci.yaml"),
        branch: Box::from(*b"work"),
        inputs: Box::new([
            api::Input { name: Box::from(*b"key"), value: Box::from(*b"receipt") },
            api::Input { name: Box::from(*b"gate"), value: Box::from(*b"focused") },
        ]),
    }
}
#[test]
fn tree_pages_are_full_or_truncated_and_objects_round_trip() {
    let expected = Box::new([
        api::Entry { name: Box::from(*b"a"), mode: 0o100_644, kind: api::EntryKind::Blob, object: HEAD },
        api::Entry { name: Box::from(*b"b"), mode: 0o040_000, kind: api::EntryKind::Tree, object: BASE },
    ]);
    let mut plan = known(Op::Read(Read::Tree { object: HEAD, page: 2 }));
    round_request(plan.begin());
    assert_eq!(plan.answer(200, TREE_DOC), Step::Done(Ok(Answer::Tree { entries: expected, more: true })));
    plan.assert_sent(&[
        "GET /api/v1/repos/fixture/specimen/git/trees/0100000000000000000000000000000000000000?page=2&per_page=2",
    ]);
    for (truncated, more) in [(false, false), (true, true)] {
        let mut plan = known(Op::Read(Read::Tree { object: BASE, page: 1 }));
        round_request(plan.begin());
        let answer = server::Answer::Tree {
            entries: Box::new([d::Entry {
                path: Box::from(*b"submodule"),
                mode: 0o160_000,
                kind: api::EntryKind::Commit,
                sha: crate::forgejo::id::ObjectId { bytes: HEAD, format: crate::forgejo::id::ObjectFormat::Sha1 },
            }]),
            truncated,
        };
        let bytes = server_answer(&answer);
        assert_eq!(
            plan.answer(200, &bytes),
            Step::Done(Ok(Answer::Tree {
                entries: Box::new([api::Entry {
                    name: Box::from(*b"submodule"),
                    mode: 0o160_000,
                    kind: api::EntryKind::Commit,
                    object: HEAD
                }]),
                more
            }))
        );
    }
}
#[test]
fn runs_keep_dispatch_and_other_events_newest_first() {
    let limits = limits();
    let decoded = fixtures::decode(
        client::decoder(&Request::Run { at: fixtures::repository(), run: 7 }, &limits).expect("run decoder"),
        RUN_DOC,
        &limits,
    );
    let Ok(d::Document::Run(newer)) = decoded.document else {
        panic!("run document");
    };
    let mut older = newer.clone();
    older.id = 6;
    older.event = Box::from(*b"push");
    older.created_at = skein_lib::Wall::from_nanos(2_000_000_000);
    let mut expected_older = run_value();
    expected_older.id = 6;
    expected_older.event = api::Event::Other;
    expected_older.created = Time::from_nanos(2_000_000_000);
    let bytes = server_answer(&server::Answer::Runs { total_count: 2, runs: Box::new([newer, older]) });
    let mut plan = known(Op::Read(Read::Runs { workflow: Box::from(*b"ci.yaml"), commit: HEAD, page: 1 }));
    round_request(plan.begin());
    assert_eq!(
        plan.answer(200, &bytes),
        Step::Done(Ok(Answer::Runs { runs: Box::new([run_value(), expected_older]), more: true }))
    );
    plan.assert_sent(&["GET /api/v1/repos/fixture/specimen/actions/runs?workflow=ci.yaml&head_sha=0100000000000000000000000000000000000000&page=1&limit=2"]);
    let mut empty = known(Op::Read(Read::Runs { workflow: Box::from(*b"ci.yaml"), commit: HEAD, page: 2 }));
    round_request(empty.begin());
    assert_eq!(
        empty.answer(200, br#"{"total_count":2,"workflow_runs":[]}"#),
        Step::Done(Ok(Answer::Runs { runs: Box::new([]), more: false }))
    );
}
#[test]
fn run_reads_jobs_and_the_first_failed_step() {
    let mut plan = known(Op::Read(Read::Run { run: 7 }));
    round_request(plan.begin());
    round_request(plan.answer(200, &server_answer(&server::Answer::Run(wire_run()))));
    round_request(plan.answer(200, JOBS_DOC));
    let mut expected = run_value();
    expected.jobs = Box::new([
        api::RunJob {
            steps: 0,
            id: 9,
            name: Box::from(*b"passed"),
            attempt: 1,
            state: api::RunState::Completed,
            conclusion: Some(api::Conclusion::Success),
            step: None,
        },
        api::RunJob {
            steps: 2,
            id: 10,
            name: Box::from(*b"failed"),
            attempt: 3,
            state: api::RunState::Completed,
            conclusion: Some(api::Conclusion::Failure),
            step: Some(api::Step { number: 1, name: Box::from(*b"x") }),
        },
    ]);
    let bytes = server_answer(&server::Answer::Job {
        job: d::Job { id: 10, run_id: 7, attempt: 3, name: Box::from(*b"failed"), status: d::StepStatus::Failure },
        steps: Box::new([
            d::Step { number: 1, name: Box::from(*b"x"), status: d::StepStatus::Failure },
            d::Step { number: 2, name: Box::from(*b"cleanup"), status: d::StepStatus::Failure },
        ]),
    });
    assert_eq!(plan.answer(200, &bytes), Step::Done(Ok(Answer::Run(expected))));
    plan.assert_sent(&[
        "GET /api/v1/repos/fixture/specimen/actions/runs/7",
        "GET /api/v1/repos/fixture/specimen/actions/runs/7/jobs",
        "GET /api/v1/repos/fixture/specimen/actions/jobs/10",
    ]);
}
#[test]
fn dispatch_and_rerun_have_typed_empty_answers_and_refusals() {
    for (write, status, expected) in [
        (dispatch(), 204, Ok(Answer::Done)),
        (dispatch(), 404, Err(Error::Missing)),
        (dispatch(), 403, Err(Error::Forbidden)),
        (Write::Rerun { attempt: ATTEMPT }, 204, Ok(Answer::Done)),
        (Write::Rerun { attempt: ATTEMPT }, 400, Err(Error::Refused)),
        (Write::Rerun { attempt: ATTEMPT }, 403, Err(Error::Forbidden)),
    ] {
        let mut plan = known(Op::Write(write));
        round_request(plan.begin());
        assert_eq!(plan.answer(status, b""), Step::Done(expected));
    }
    let mut plan = known(Op::Write(dispatch()));
    round_request(plan.begin());
    plan.assert_bodies(&[Some(br#"{"ref":"work","inputs":{"key":"receipt","gate":"focused"}}"#)]);
}
#[test]
fn merge_fast_forward_and_rebase_update_spell_their_style() {
    let mut plan = known(Op::Write(Write::Merge { number: 1, head: HEAD, how: MergeHow::FastForwardOnly }));
    round_request(plan.begin());
    plan.assert_bodies(&[Some(
        br#"{"Do":"fast-forward-only","head_commit_id":"0100000000000000000000000000000000000000"}"#,
    )]);
    for (status, body, expected) in [
        (200, b"".as_slice(), Ok(Answer::Done)),
        (409, br#"{"message":"merge failed because of conflict"}"#.as_slice(), Err(Error::Conflict)),
        (409, b"".as_slice(), Err(Error::Conflict)),
    ] {
        let mut plan = known(Op::Write(Write::Update { number: 1, style: UpdateStyle::Rebase }));
        round_request(plan.begin());
        plan.assert_sent(&["POST /api/v1/repos/fixture/specimen/pulls/1/update?style=rebase"]);
        assert_eq!(plan.answer(status, body), Step::Done(expected));
    }
}
#[test]
fn status_sends_description_and_link_only_when_present() {
    for (description, link, body) in [
        (
            b"gate passed".as_slice(),
            b"https://forge/run/7".as_slice(),
            br#"{"description":"gate passed","target_url":"https://forge/run/7","state":"success","context":"ci"}"#
                .as_slice(),
        ),
        (b"".as_slice(), b"".as_slice(), br#"{"state":"success","context":"ci"}"#.as_slice()),
    ] {
        let mut plan = known(Op::Write(Write::Status {
            commit: HEAD,
            context: Box::from(*b"ci"),
            check: api::Check::Passed,
            description: Box::from(description),
            link: Box::from(link),
        }));
        round_request(plan.begin());
        plan.assert_bodies(&[Some(body)]);
    }
}

#[test]
fn an_empty_tree_page_is_null_on_the_wire_and_an_empty_answer() {
    let answer = server::Answer::Tree { entries: Box::new([]), truncated: false };
    let bytes = server_answer(&answer);
    assert_eq!(&*bytes, br#"{"truncated":false,"tree":null}"#);
    for body in [bytes.as_ref(), br#"{"tree":[],"truncated":false}"#] {
        let mut plan = known(Op::Read(Read::Tree { object: HEAD, page: 3 }));
        round_request(plan.begin());
        assert_eq!(plan.answer(200, body), Step::Done(Ok(Answer::Tree { entries: Box::new([]), more: false })));
    }
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "step tests construct and inspect wire fixtures (testing-strategy.md, 4)"
)]
fn dispatch_trigger_event_takes_precedence_over_the_empty_payload_event() {
    for fields in [
        br#""event":"","trigger_event":"workflow_dispatch""#.as_slice(),
        br#""trigger_event":"workflow_dispatch","event":"""#.as_slice(),
    ] {
        let original = core::str::from_utf8(RUN_DOC).expect("ASCII fixture");
        let text =
            original.replace("\"event\":\"workflow_dispatch\"", core::str::from_utf8(fields).expect("ASCII fields"));

        let mut plan = known(Op::Read(Read::Runs { workflow: Box::from(*b"ci.yaml"), commit: HEAD, page: 1 }));
        round_request(plan.begin());
        let listing = [b"{\"total_count\":1,\"workflow_runs\":[".as_slice(), text.as_bytes(), b"]}"].concat();
        assert_eq!(
            plan.answer(200, &listing),
            Step::Done(Ok(Answer::Runs { runs: Box::new([run_value()]), more: false }))
        );
    }
    let bytes = server_answer(&server::Answer::Run(wire_run()));
    assert!(
        bytes
            .windows(b"\"event\":\"\",\"trigger_event\":\"workflow_dispatch\"".len())
            .any(|w| w == b"\"event\":\"\",\"trigger_event\":\"workflow_dispatch\"")
    );
}

#[test]
fn the_pinned_diverging_fast_forward_error_is_an_unknown_write_outcome() {
    let message = crate::forgejo::failure::DIVERGING_FAST_FORWARD;
    let body = server_answer(&server::Answer::Error(d::ErrorBody { message: d::Body::Kept(Box::from(message)) }));
    let mut plan = known(Op::Write(Write::Merge { number: 1, head: HEAD, how: MergeHow::FastForwardOnly }));
    round_request(plan.begin());
    assert_eq!(plan.answer(500, &body), Step::Done(Err(Error::Timeout)));
}

#[test]
fn runs_filter_workflows_locally_but_more_counts_the_providers_rows() {
    let matching = wire_run();
    let mut unrelated = matching.clone();
    unrelated.id = 8;
    unrelated.workflow = Box::from(*b"other.yaml");
    let body = server_answer(&server::Answer::Runs { total_count: 2, runs: Box::new([unrelated.clone(), matching]) });
    let mut plan = known(Op::Read(Read::Runs { workflow: Box::from(*b"ci.yaml"), commit: HEAD, page: 1 }));
    round_request(plan.begin());
    assert_eq!(plan.answer(200, &body), Step::Done(Ok(Answer::Runs { runs: Box::new([run_value()]), more: true })));
    let body = server_answer(&server::Answer::Runs { total_count: 2, runs: Box::new([unrelated.clone(), unrelated]) });
    let mut plan = known(Op::Read(Read::Runs { workflow: Box::from(*b"ci.yaml"), commit: HEAD, page: 2 }));
    round_request(plan.begin());
    assert_eq!(plan.answer(200, &body), Step::Done(Ok(Answer::Runs { runs: Box::new([]), more: true })));
}
