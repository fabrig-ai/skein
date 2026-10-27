//! Write prerequisites and pending requests fit the GitHub call ledger.
use super::{fixtures as f, github_files::CallHeap};
use alloc::boxed::Box;
use skein_forge::api::{Answer, Error, Op, Read, Write};
use skein_lib::{Decimal, Duration, List, Writer};

fn warm() -> f::Harness {
    let mut h = f::github_calls();
    f::github_repository(&mut h, f::REPO, 22);
    h.call(1, f::REPO, Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) }));
    h.run(Duration::ZERO);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &f::ok(f::GITHUB_BRANCH));
    h.run(Duration::ZERO);
    assert_eq!(h.answers.remove(&1).expect("warm connection").result, Ok(Answer::Commit([0; 32])));
    h
}

fn begin(h: &mut f::Harness, op: Op, heap: &mut CallHeap) {
    let span = skein_heap::Span::start();
    h.call(1, f::REPO, op);
    h.run(Duration::ZERO);
    heap.count(span.end());
}

fn hear(h: &mut f::Harness, body: &[u8], heap: &mut CallHeap) {
    let span = skein_heap::Span::start();
    h.answer(f::FORGE, body);
    h.run(Duration::ZERO);
    heap.count(span.end());
}

#[expect(clippy::print_stdout, clippy::disallowed_macros, reason = "report measured call peaks for the heap ledger")]
fn fits(heap: &CallHeap, label: &str) {
    let slot = crate::limits::github_call_heap(&f::limits(), &f::shared()).expect("GitHub call heap");
    assert!(u64::try_from(heap.peak).expect("positive peak") <= slot, "{label}: {} > {slot}", heap.peak);
    println!("{label}: peak {} bytes <= GitHub call heap {slot}", heap.peak);
}

#[test]
fn largest_github_set_reviewers_fits_one_slots_heap() {
    let shared = f::shared();
    let l = f::limits();
    let bound = shared.rows.min(shared.op_bytes / 8).min(shared.answer_bytes / 8);
    assert_eq!(bound, l.inner, "both reviewer lists reach their fixture bound");
    // Both full lists can differ by at most rows in total. Also measure a
    // full addition or removal alone: all login slots/text exist before the
    // body check refuses eight string-bound logins in the fixture's body cap.
    for (label, desired_count, current_count, first) in [
        ("SetReviewers both full", bound, bound, bound / 2),
        ("SetReviewers full addition", bound, 0, 0),
        ("SetReviewers full removal", 0, bound, 0),
    ] {
        let mut h = warm();
        let mut desired = List::with_capacity(desired_count);
        for id in 0..desired_count {
            desired.push(u64::from(id)).expect("desired bound");
        }
        let login = f::filled(usize::try_from(l.string_bytes).expect("string bound"), b'l');
        for id in 0..bound.checked_mul(2).expect("reviewer ids") {
            let wanted = id < desired_count;
            let current = id >= first && id < first.checked_add(current_count).expect("current bound");
            if wanted != current {
                h.component.github_tables().keep_user(f::FORGE, u64::from(id), login.clone());
            }
        }
        let mut length = usize::try_from(current_count.saturating_sub(1)).expect("commas");
        for id in first..first.checked_add(current_count).expect("current bound") {
            length = length
                .checked_add(7)
                .expect("id object")
                .checked_add(Decimal::of(u64::from(id)).as_bytes().len())
                .expect("id length");
        }
        let mut reviewers = Writer::new(length);
        for id in first..first.checked_add(current_count).expect("current bound") {
            if id != first {
                reviewers.put(b",").expect("comma");
            }
            reviewers.put(b"{\"id\":").expect("id key");
            reviewers.put(Decimal::of(u64::from(id)).as_bytes()).expect("id");
            reviewers.put(b"}").expect("reviewer end");
        }
        let pull = joined(&[
            br#"{"number":7,"state":"open","merged":false,"mergeable":true,"head":{"ref":""#,
            &login,
            br#"","sha":"0000000000000000000000000000000000000000"},"base":{"ref":""#,
            &login,
            br#"","sha":"1111111111111111111111111111111111111111"},"requested_reviewers":["#,
            &reviewers.finish(),
            b"]}",
        ]);
        let response = f::ok(&pull);
        let added = f::response(201, b"{}");
        let removed = f::ok(b"{}");
        let op = Op::Write(Write::SetReviewers { number: 7, reviewers: desired.into_boxed() });
        let mut heap = CallHeap { peak: 0, held: 0 };
        begin(&mut h, op, &mut heap);
        hear(&mut h, &response, &mut heap);
        if desired_count == bound && current_count == bound {
            for response in [&added, &removed] {
                let wire = h.request_bytes(f::FORGE);
                assert!(skein_lib::bytes::find(&wire, &login).is_some(), "string-bound login is sent");
                hear(&mut h, response, &mut heap);
            }
            let answer = h.answered(1).expect("reviewers terminal");
            assert_eq!((answer.cost, &answer.result), (3, &Ok(Answer::Done)));
        } else {
            let answer = h.answered(1).expect("body cap checked before mutation");
            assert_eq!((answer.cost, &answer.result), (1, &Err(Error::TooLarge)));
        }
        fits(&heap, label);
    }
}

#[test]
fn github_queue_merge_fits_one_slots_heap() {
    let mut h = warm();
    let text = f::filled(usize::try_from(f::limits().string_bytes).expect("string bound"), b'n');
    let pull = joined(&[
        br#"{"data":{"repository":{"pullRequest":{"id":""#,
        &text,
        br#"","mergeCommit":null,"mergeQueueEntry":null,"timelineItems":{"nodes":[{"reason":""#,
        &text,
        br#""}]}}}}}"#,
    ]);
    let refusal = f::response(
        405,
        br#"{"message":"Repository rule violations found\n\nChanges must be made through the merge queue\n\n"}"#,
    );
    let pull = f::ok(&pull);
    let enqueued = f::ok(br#"{"data":{"enqueuePullRequest":{"mergeQueueEntry":{"position":1}}}}"#);
    let mut heap = CallHeap { peak: 0, held: 0 };
    begin(
        &mut h,
        Op::Write(Write::Merge { number: 7, head: [0; 32], how: skein_forge::api::MergeHow::Style }),
        &mut heap,
    );
    hear(&mut h, &refusal, &mut heap);
    hear(&mut h, &pull, &mut heap);
    assert!(skein_lib::bytes::find(&h.request_bytes(f::FORGE), &text).is_some());
    hear(&mut h, &enqueued, &mut heap);
    let answer = h.answered(1).expect("queue terminal");
    assert_eq!((answer.cost, &answer.result), (3, &Ok(Answer::Queued)));
    fits(&heap, "queue Merge");
}

// Scripted peer allocations stay outside the component's measurement spans.
fn joined(parts: &[&[u8]]) -> Box<[u8]> {
    let mut size = 0_usize;
    for part in parts {
        size = size.checked_add(part.len()).expect("fixture size");
    }
    let mut out = Writer::new(size);
    for part in parts {
        out.put(part).expect("fixture bytes");
    }
    out.finish()
}
