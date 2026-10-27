//! Artifact requests, streamed listings and archives meet at the call plan.
use super::*;
use crate::forgejo::{client, document as d, request::Request, server, tests::zip::archive};
const RUN_ID: u64 = 7;
const ARTIFACT_ID: u64 = 11;
const FILE_BYTES: u32 = 4; // b"a\nb\n".
const NAME: &[u8] = b"tests /name";
fn read(max_bytes: u32) -> Read {
    Read::Artifact { run: RUN_ID, name: Box::from(NAME), max_bytes }
}
fn listing() -> server::Answer {
    server::Answer::Artifacts(Box::new([d::Artifact {
        id: ARTIFACT_ID,
        name: Box::from(NAME),
        run_id: RUN_ID,
        size_in_bytes: u64::from(FILE_BYTES),
    }]))
}
fn round(step: Step) -> Request {
    let Step::Send(exchange) = step else {
        panic!("request");
    };
    let limits = fixtures::limits();
    let encoded = client::encode(&exchange.request, &limits).expect("encoded request");
    let server::Route::Ready(request) = server::route(encoded.method, &encoded.target, &limits).expect("route") else {
        panic!("read route");
    };
    assert_eq!(request, exchange.request);
    assert_eq!(exchange.range, None);
    request
}
fn zip_plan(max: u32) -> Run {
    let mut plan = Run::known(Op::Read(read(max)));
    round(plan.begin());
    let listing = server::encode(&listing(), &fixtures::limits()).expect("artifact listing");
    round(plan.answer(200, &listing));
    round(plan.answer(200, RUN));
    plan
}
#[test]
fn artifact_files_keep_the_run_commit_and_cut_only_when_longer() {
    for (deflated, descriptor) in [(false, None), (true, Some(true))] {
        let zip = archive(&[b"failed.txt"], deflated, descriptor);
        for (max, bytes, truncated) in
            [(FILE_BYTES, b"a\nb\n".as_slice(), false), (FILE_BYTES.saturating_sub(1), b"a\nb".as_slice(), true)]
        {
            let mut plan = zip_plan(max);
            assert_eq!(
                plan.answer(200, &zip),
                Step::Done(Ok(Answer::File {
                    head: HEAD,
                    path: Box::from(*b"failed.txt"),
                    bytes: Box::from(bytes),
                    truncated
                }))
            );
            plan.assert_sent(&[
                "GET /api/v1/repos/fixture/specimen/actions/runs/7/artifacts?name=tests%20%2Fname",
                "GET /api/v1/repos/fixture/specimen/actions/runs/7",
                "GET /api/v1/repos/fixture/specimen/actions/artifacts/11/zip",
            ]);
        }
    }
}
#[test]
fn missing_artifacts_and_unknown_runs_keep_the_repository_path() {
    for (status, bytes) in [(200, b"[]".as_slice()), (404, b"".as_slice())] {
        let mut plan = Run::known(Op::Read(read(FILE_BYTES)));
        plan.begin();
        assert_eq!(plan.answer(status, bytes), Step::Done(Err(Error::Missing)));
        assert!(plan.tables().repository(REPO).is_some());
    }
}
#[test]
#[expect(clippy::disallowed_methods, reason = "fixed ZIP record offsets name the edited method and CRC")]
fn unsupported_or_corrupt_artifacts_fail_the_read() {
    let mut method = archive(&[b"failed.txt"], false, None);
    let central = 30usize + b"failed.txt".len() + usize::try_from(FILE_BYTES).expect("file length");
    method[8..10].copy_from_slice(&12u16.to_le_bytes());
    method[central + 10..central + 12].copy_from_slice(&12u16.to_le_bytes());
    let mut crc = archive(&[b"failed.txt"], false, None);
    crc[14] ^= 1;
    crc[central + 16] ^= 1;
    for (bytes, error) in
        [(archive(&[b"a", b"b"], false, None), Error::Refused), (method, Error::Refused), (crc, Error::InvalidAnswer)]
    {
        let mut plan = zip_plan(FILE_BYTES);
        assert_eq!(plan.answer(200, &bytes), Step::Done(Err(error)));
    }
}
#[test]
fn whole_archives_fit_exactly_at_the_answer_bound() {
    let zip = archive(&[b"failed.txt"], false, None);
    let bound = u32::try_from(zip.len()).expect("small ZIP");
    for (answer_bytes, expected) in [
        (
            bound,
            Ok(Answer::File {
                head: HEAD,
                path: Box::from(*b"failed.txt"),
                bytes: Box::from(*b"a\nb\n"),
                truncated: false,
            }),
        ),
        (bound.saturating_sub(1), Err(Error::TooLarge)),
    ] {
        let mut plan =
            Run::with(crate::forgejo::plans::plan::Bounds { answer_bytes, ..BOUNDS }, Op::Read(read(FILE_BYTES)));
        plan.begin();
        let bytes = server::encode(&listing(), &fixtures::limits()).expect("listing");
        plan.answer(200, &bytes);
        plan.answer(200, RUN);
        assert_eq!(plan.answer(200, &zip), Step::Done(expected));
    }
}
