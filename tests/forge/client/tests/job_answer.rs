use skein_fake_forge_domain::api as forge;
use skein_forge::api;
use skein_forge_client_world::{LIMITS, translate};
#[test]
fn a_job_with_no_failed_step_translates_to_its_whole_logs_end() {
    let attempt = api::JobAttempt { head: translate::commit(3), run: 3, job: 7, attempt: 2 };
    let asked = api::Op::Read(api::Read::Job { attempt, max_bytes: 4 });
    let steps = Box::new([
        forge::Step {
            number: 0,
            name: Box::from(&b"Set up job"[..]),
            status: forge::StepStatus::Success,
            log: Box::from(&b"ab"[..]),
        },
        forge::Step {
            number: 1,
            name: Box::from(&b"Run build"[..]),
            status: forge::StepStatus::Cancelled,
            log: Box::from(&b"cd"[..]),
        },
        forge::Step {
            number: 2,
            name: Box::from(&b"Complete job"[..]),
            status: forge::StepStatus::Cancelled,
            log: Box::from(&b"ef"[..]),
        },
    ]);
    let result = translate::answer(
        &asked,
        forge::Answer::Job {
            job: forge::Job {
                id: 7,
                run: 3,
                index: 0,
                attempt: 2,
                name: Box::new([]),
                state: forge::RunState::Completed,
                conclusion: Some(forge::Conclusion::Failure),
                steps: Box::new([]),
            },
            steps,
        },
        &LIMITS,
    );
    assert_eq!(result, api::Answer::Job { attempt, step: None, log: Box::from(&b"cdef"[..]), truncated: true });
}
