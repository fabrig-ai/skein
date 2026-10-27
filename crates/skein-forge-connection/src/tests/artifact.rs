//! A compressed artifact at the shared answer bound fits its component ledger.
use super::{fixtures as f, github_files::CallHeap};
use alloc::boxed::Box;
use skein_forge::{
    api::{Answer, Op, Read},
    forgejo::{document as d, server, zip},
};
use skein_lib::Duration;
const RUN: u64 = 7;
const ARTIFACT: u64 = 11;
const REQUESTS: u32 = 3; // Listing, run, complete ZIP.
const PATH: &[u8] = b"failed.txt";
const RUN_BODY: &[u8] = br#"{"id":7,"index_in_repo":7,"commit_sha":"0000000000000000000000000000000000000000"}"#;
fn warm() -> f::Harness {
    let mut h = f::known();
    h.call(1, f::REPO, Op::Read(Read::Branch { branch: Box::from(f::BRANCH) }));
    h.answer(f::FORGE, &f::ok(f::BRANCH_BODY));
    h.run(Duration::ZERO);
    drop(h.answers.remove(&1));
    h
}
#[test]
#[expect(clippy::print_stdout, clippy::disallowed_macros, reason = "report the artifact's measured component peak")]
fn an_answer_bound_inflated_artifact_fits_the_owned_heap() {
    let bound = f::shared().answer_bytes;
    let bytes = f::filled(usize::try_from(bound).expect("answer cap"), b'x');
    let archive = zip::write(PATH, &bytes, true, bound).expect("compressible file fits archive cap");
    let listing = server::encode(
        &server::Answer::Artifacts(Box::new([d::Artifact {
            id: ARTIFACT,
            name: Box::from(*b"tests"),
            run_id: RUN,
            size_in_bytes: u64::from(bound),
        }])),
        &crate::limits::documents(&f::limits()),
    )
    .expect("listing fits");
    let mut h = warm();
    h.component_heap = Some(CallHeap { peak: 0, held: 0 });
    h.call(1, f::REPO, Op::Read(Read::Artifact { run: RUN, name: Box::from(*b"tests"), max_bytes: bound }));
    for body in [&listing, RUN_BODY, &archive] {
        let response = f::ok(body);
        h.answer(f::FORGE, &response);
        h.run(Duration::ZERO);
    }
    let heap = h.component_heap.take().expect("component metering");
    let answer = h.answered(1).expect("artifact answer");
    let kept = usize::try_from(bound).expect("answer cap").checked_sub(PATH.len()).expect("path fits");
    assert_eq!(
        answer.result,
        Ok(Answer::File { head: f::BRANCH_HEAD, path: Box::from(PATH), bytes: f::filled(kept, b'x'), truncated: true })
    );
    // Two raw buffers and the retained answer pay archive, inflate and output.
    let ledger = crate::limits::forgejo_call_heap(&f::limits(), &f::shared())
        .expect("call ledger")
        .checked_add(u64::from(bound).checked_mul(2).expect("raw overlap"))
        .expect("artifact ledger");
    assert!(u64::try_from(heap.peak).expect("peak") <= ledger, "{} <= {ledger}", heap.peak);
    println!("artifact component peak={} ledger={ledger}", heap.peak);
    assert_eq!(answer.cost, REQUESTS);
}
