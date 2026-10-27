#![cfg(test)]
#![expect(clippy::needless_raw_string_hashes, reason = "literal malformed fields use consistent raw-byte notation")]
use skein_forge::forgejo::{
    client,
    decode::Malformed,
    document::{Document, Row, StatusState},
    server,
};
pub mod common;
use common::cases::files::{cases, expected};
use common::forgejo;
#[test]
fn file_transcripts_decode_to_their_documents_and_rows() {
    for (case, request, answer, status) in cases() {
        let transcript = forgejo::transcript(case);
        assert_eq!(transcript.status, status, "{case}");
        if let Some(answer) = answer {
            let (document, rows) = expected(answer);
            let decoded = forgejo::decode(&request, &transcript.body);
            assert_eq!(decoded.document, Ok(document), "{case}");
            assert_eq!(decoded.rows, rows, "{case}");
        } else {
            assert!(client::decoder(&request, &forgejo::limits()).is_none(), "{case}: bytes");
        }
    }
}
#[test]
fn file_requests_encode_as_the_tool_sent_them() {
    for (case, request, _, _) in cases() {
        let transcript = forgejo::transcript(case);
        let encoded = client::encode(&request, &forgejo::limits()).expect("request");
        assert_eq!(encoded.method, transcript.method, "{case}");
        assert_eq!(encoded.target.as_ref(), transcript.target, "{case}");
        assert_eq!(encoded.body.as_deref().unwrap_or(b""), transcript.request_body, "{case}");
    }
}
#[test]
fn file_requests_route_on_the_servers_side_to_the_same_request() {
    for (case, request, _, _) in cases() {
        assert_eq!(forgejo::route(&forgejo::transcript(case)), request, "{case}");
    }
}
#[test]
fn file_answers_the_server_encodes_decode_unchanged_with_the_transcripts_shape() {
    for (case, request, answer, _) in cases() {
        if let Some(answer) = answer {
            let transcript = forgejo::transcript(case);
            let bytes = server::encode(&answer, &forgejo::limits()).expect("answer");
            assert!(forgejo::shape(&bytes).is_subset(&forgejo::shape(&transcript.body)), "{case}");
            let (document, rows) = expected(answer);
            let decoded = forgejo::decode(&request, &bytes);
            assert_eq!(decoded.document, Ok(document), "{case}");
            assert_eq!(decoded.rows, rows, "{case}");
        } else {
            assert!(client::decoder(&request, &forgejo::limits()).is_none(), "{case}: bytes");
        }
    }
}
#[test]
fn file_transcript_mutations_are_malformed_naming_the_field() {
    for (case, field, before, after, expected) in [
        (
            "pull-files-page-1",
            "status",
            br#""status":"added""#.as_slice(),
            br#""status":"unknown""#.as_slice(),
            Malformed::Value { field: b"status" },
        ),
        (
            "compare",
            "sha",
            br#""sha":"0000000000000000000000000000000000000004""#.as_slice(),
            br#""sha":"000000000000000000000000000000000000000""#.as_slice(),
            Malformed::Id { field: b"sha" },
        ),
        (
            "actions-run",
            "commit_sha",
            br#""commit_sha":"0000000000000000000000000000000000000010","#.as_slice(),
            br#""#.as_slice(),
            Malformed::Missing { field: b"commit_sha" },
        ),
        (
            "actions-jobs",
            "attempt",
            br#""attempt":1"#.as_slice(),
            br#""attempt":4294967296"#.as_slice(),
            Malformed::Range { field: b"attempt" },
        ),
        (
            "actions-combined-status",
            "creator",
            br#""creator":null,"#.as_slice(),
            br#""#.as_slice(),
            Malformed::Missing { field: b"creator" },
        ),
        (
            "actions-job",
            "status",
            br#""status":"success""#.as_slice(),
            br#""status":"finished""#.as_slice(),
            Malformed::Value { field: b"status" },
        ),
        (
            "actions-job",
            "number",
            br#""number":1"#.as_slice(),
            br#""number":4294967296"#.as_slice(),
            Malformed::Range { field: b"number" },
        ),
    ] {
        let transcript = forgejo::transcript(case);
        let bytes = forgejo::replace(&transcript.body, before, after);
        assert_eq!(forgejo::decode(&forgejo::route(&transcript), &bytes).document, Err(expected), "{case}/{field}");
    }
}
#[test]
fn a_failing_actions_status_links_its_run_and_job() {
    let transcript = forgejo::transcript("actions-combined-status");
    let decoded = forgejo::decode(&forgejo::route(&transcript), &transcript.body);
    let Row::Status(failed) = &decoded.rows[1] else {
        panic!("status row");
    };
    assert_eq!(failed.status, StatusState::Failure);
    let link = skein_forge::forgejo::header::job_link(&failed.target_url, b"http://127.0.0.1:3000/fixture/specimen");
    assert_eq!(link, Some((1, 1)));
    let (run_number, job_index) = link.expect("this repository's job link");
    let transcript = forgejo::transcript("actions-runs");
    let runs = forgejo::decode(&forgejo::route(&transcript), &transcript.body);
    let Row::Run(ref run) = runs.rows[0] else {
        panic!("run row");
    };
    assert_eq!(run.index_in_repo, run_number);
    assert_eq!(run.commit_sha, forgejo::sha(16));
    let transcript = forgejo::transcript("actions-jobs");
    let jobs = forgejo::decode(&forgejo::route(&transcript), &transcript.body);
    let Row::Job(ref job) = jobs.rows[usize::try_from(job_index).expect("job index fits")] else {
        panic!("job row");
    };
    assert_eq!(job.run_id, run.id);
    assert_eq!(job.id, 2);
    assert_eq!(job.attempt, 1);
}

#[test]
fn the_recorded_forgejo_zip_inflates_its_single_file() {
    // Forgejo 17's four-byte report in a 144-byte ZIP with a signed descriptor.
    const ZIP_BYTES: u32 = 144;
    const FILE_BYTES: u32 = 4;
    let transcript = forgejo::transcript("actions-artifact-zip");
    assert_eq!(transcript.status, 200);
    assert_eq!(transcript.body.len(), usize::try_from(ZIP_BYTES).expect("fixture length"));
    assert_eq!(
        skein_forge::forgejo::zip::read(&transcript.body, FILE_BYTES, ZIP_BYTES),
        Ok(skein_forge::forgejo::zip::File {
            path: Box::from(*b"failed.txt"),
            bytes: Box::from(*b"a\nb\n"),
            truncated: false
        })
    );
}

#[test]
fn an_unknown_runs_artifact_listing_is_404() {
    let transcript = forgejo::transcript("actions-artifacts-unknown-run");
    assert_eq!(transcript.status, 404);
    let request = skein_forge::forgejo::request::Request::Artifacts {
        at: skein_forge::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"turn") },
        run: 9999,
        name: Some(Box::from(*b"tests")),
    };
    assert_eq!(forgejo::route(&transcript), request);
    let encoded = client::encode(&request, &forgejo::limits()).expect("request");
    assert_eq!(&*encoded.target, &*transcript.target);
    assert_eq!(
        forgejo::decode_error(&transcript.body).document,
        Ok(Document::Error(skein_forge::forgejo::document::ErrorBody {
            message: skein_forge::forgejo::document::Body::Kept(Box::from(
                *b"run with id 9999: resource does not exist"
            ))
        }))
    );
}
