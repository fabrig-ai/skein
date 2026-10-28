//! Context cues through the same CI and Actions reads as a client.
use super::*;
use crate::api::{CheckSummary, ContextCue, StepStatus};

fn configured() -> (Config, Setup) {
    let config = Config { limits: Limits { name_bytes: 64, content_bytes: 128, ..LIMITS }, ..CALM };
    let mut setup = setup();
    setup.checks.contexts = names(&[b"check", b"changes"]);
    setup.checks.cues = Box::new([ContextCue {
        context: copy_of(b"changes"),
        cue: Cue::PullFile {
            path: copy_of(b"changes/{pull}.md"),
            no_pull: Check::Skipped,
            step: copy_of(b"changes"),
            log: copy_of(b"Missing changes/{pull}.md"),
        },
    }]);
    (config, setup)
}

fn assert_contexts(h: &mut Harness, pull: u64, check: Check, changes: Check) {
    let pull = h.pull(pull);
    for (name, expected) in [(b"check".as_slice(), check), (b"changes".as_slice(), changes)] {
        let mut found = false;
        for status in &pull.statuses {
            if status.context.as_ref() == name {
                assert_eq!(status.state, expected, "{name:?}");
                found = true;
            }
        }
        assert!(found, "named context {name:?}");
    }
}

fn named<'a>(rows: &'a [CheckSummary], context: &[u8]) -> &'a CheckSummary {
    for row in rows {
        if row.status.context.as_ref() == context {
            return row;
        }
    }
    panic!("named context {context:?}");
}

#[test]
fn contexts_choose_their_own_cue_and_inherit_the_repository_cue() {
    let mut configured = setup();
    configured.checks.contexts = names(&[b"check", b"changes"]);
    configured.checks.cue = Some(Cue::File { path: copy_of(b"ci"), green: copy_of(b"green") });
    configured.checks.cues =
        Box::new([ContextCue { context: copy_of(b"check"), cue: Cue::Message { fail: copy_of(b"FAIL") } }]);
    let mut h = Harness::with(CALM, configured);
    let head = h.commit(FIRST, &[(b"ci", b"red")]);
    h.push(ENGINE, b"work", head).unwrap();
    h.open(b"work").unwrap();
    h.settle();
    assert_contexts(&mut h, 1, Check::Passed, Check::Failed);
}

#[test]
fn a_file_cue_skips_a_push_then_judges_the_pull_on_the_same_head() {
    let (config, setup) = configured();
    let mut h = Harness::with(config, setup);
    let head = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", head).unwrap();
    h.settle();
    let Answer::Checks(statuses) = h.ok(PERSON, read(Read::Checks { commit: head })) else { unreachable!("checks") };
    let check = named(&statuses, b"check");
    let changes = named(&statuses, b"changes");
    assert_eq!(check.status.state, Check::Passed, "an uncued context passes");
    assert_eq!(changes.status.state, Check::Skipped);
    assert_eq!(&*changes.description, b"CI skipped");
    assert_eq!(h.open(b"work"), Ok(Answer::Created(1)));
    h.settle();
    assert_contexts(&mut h, 1, Check::Passed, Check::Failed);
}

#[test]
fn a_file_cue_substitutes_the_pull_number_and_needs_only_file_presence() {
    let (config, setup) = configured();
    let mut h = Harness::with(config, setup);
    h.ok(ENGINE, create(b"issue", b"body", &[]));
    let wrong = h.commit(FIRST, &[(b"changes/1.md", b"not this pull")]);
    h.push(ENGINE, b"work", wrong).unwrap();
    assert_eq!(h.open(b"work"), Ok(Answer::Created(2)));
    h.settle();
    assert_contexts(&mut h, 2, Check::Passed, Check::Failed);
    let right = h.commit(wrong, &[(b"changes/2.md", b"")]);
    h.push(ENGINE, b"work", right).unwrap();
    h.settle();
    assert_contexts(&mut h, 2, Check::Passed, Check::Passed);
}

#[test]
fn a_cued_failure_is_read_back_through_actions_job_step_and_log() {
    let (config, setup) = configured();
    let mut h = Harness::with(config, setup);
    h.ok(ENGINE, create(b"issue", b"body", &[]));
    let head = h.commit(FIRST, &[(b"src", b"one")]);
    h.push(ENGINE, b"work", head).unwrap();
    h.open(b"work").unwrap();
    h.settle();
    let Answer::Checks(statuses) = h.ok(PERSON, read(Read::Checks { commit: head })) else { unreachable!("checks") };
    let reference = named(&statuses, b"changes").job.unwrap();
    let Answer::Jobs(jobs) = h.ok(PERSON, read(Read::Jobs { run: reference.run })) else { unreachable!("jobs") };
    assert_eq!(jobs[1].id, reference.job);
    let Answer::Job { job, steps } = h.ok(PERSON, read(Read::Job { job: reference.job })) else { unreachable!("job") };
    assert_eq!(job.attempt, reference.attempt);
    assert_eq!(&*steps[1].name, b"changes");
    assert_eq!(steps[1].status, StepStatus::Failure);
    assert!(steps[1].log.ends_with(b"Missing changes/2.md\n"));
    assert_eq!(
        h.ok(PERSON, read(Read::Log { job: job.id, attempt: job.attempt, step: Some(1) })),
        Answer::File(steps[1].log.clone())
    );
    assert_eq!(
        h.call(PERSON, read(Read::Log { job: job.id, attempt: 99, step: Some(1) })),
        Err(Error::Missing(What::Job))
    );
}

#[test]
fn templates_expand_every_occurrence_and_price_twenty_digit_numbers() {
    assert_eq!(&*crate::ci::template(b"{pull}/{pull}", Some(u64::MAX)), b"18446744073709551615/18446744073709551615");
    assert_eq!(crate::ci::template_bound(b"{pull}/{pull}"), 41);
    let (config, mut setup) = configured();
    setup.checks.cues = Box::new([ContextCue {
        context: copy_of(b"changes"),
        cue: Cue::PullFile {
            path: copy_of(b"changes/{pull}.md"),
            no_pull: Check::Skipped,
            step: copy_of(b"changes"),
            log: copy_of(b"Missing changes/{pull}.md"),
        },
    }]);
    let config = Config { limits: Limits { name_bytes: 31, ..config.limits }, ..config };
    let _h = Harness::with(config, setup);
}

#[test]
#[should_panic(expected = "expanded path within the limits")]
fn a_template_one_byte_past_its_expanded_path_limit_is_refused() {
    let (config, setup) = configured();
    let config = Config { limits: Limits { name_bytes: 30, ..config.limits }, ..config };
    let _h = Harness::with(config, setup);
}
