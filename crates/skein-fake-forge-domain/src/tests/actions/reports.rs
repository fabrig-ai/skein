use super::*;
use crate::api::{Artifact, Cue, Report, ReportFormat};
const REPORT: &[u8] = b"tests";
fn configured_report(config: Config, format: ReportFormat, fails: &[&[u8]], cue: Option<Cue>) -> Harness {
    configured_report_jobs(config, format, fails, &[], cue, &[b"tests"])
}
fn configured_report_jobs(
    config: Config,
    format: ReportFormat,
    fails: &[&[u8]],
    flaky: &[&[u8]],
    cue: Option<Cue>,
    jobs: &[&[u8]],
) -> Harness {
    let mut configured = setup();
    configured.checks.contexts = Box::new([]);
    let mut workflow = workflow(jobs);
    workflow.fails = names(fails);
    workflow.flaky = names(flaky);
    workflow.report = Some(Report { artifact: copy_of(REPORT), format, deflate: true });
    workflow.select = Some(copy_of(b"select"));
    workflow.cue = cue;
    configured.workflows = Box::new([workflow]);
    // The three-failure JUnit document is this fixture's largest report.
    let content_bytes = u32::try_from(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"a\"><failure/></testcase><testcase name=\"b\"><failure/></testcase><testcase name=\"c\"><failure/></testcase></testsuite>".len()).unwrap();
    Harness::with(Config { limits: Limits { content_bytes, ..config.limits }, ..config }, configured)
}
fn run_report(h: &mut Harness, selected: Option<&[u8]>) -> Result<Answer, Error> {
    let inputs: Box<[crate::api::WorkflowInput]> = match selected {
        Some(ids) => Box::new([(copy_of(b"select"), copy_of(ids))]),
        None => Box::new([]),
    };
    h.ok(ENGINE, write(Write::Dispatch { workflow: copy_of(b"gate.yml"), branch: copy_of(MAIN), inputs }));
    h.settle();
    let run = workflow_runs(h)[0].id;
    h.call(ENGINE, read(Read::Artifact { run, name: copy_of(REPORT) }))
}
fn junit(bytes: &[u8]) -> Answer {
    Answer::Artifact {
        commit: FIRST,
        artifact: Artifact {
            name: copy_of(REPORT),
            path: copy_of(b"report.xml"),
            bytes: copy_of(bytes),
            deflate: true,
        },
    }
}
#[test]
fn failed_ids_upload_a_junit_report() {
    let mut h = configured_report(CALM, ReportFormat::Junit, &[b"b", b"a", b"a"], None);
    let report = run_report(&mut h, None);
    assert_eq!(report, Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"a\"><failure/></testcase><testcase name=\"b\"><failure/></testcase></testsuite>")));
    assert_eq!(run_jobs(&h, 1)[0].conclusion, Some(crate::api::Conclusion::Failure));
}
#[test]
fn selection_runs_only_its_failed_id() {
    let mut h = configured_report(CALM, ReportFormat::Junit, &[b"a", b"b"], None);
    assert_eq!(run_report(&mut h, Some(b"b")), Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"b\"><failure/></testcase></testsuite>")));
}
#[test]
fn selection_reports_passing_ids_beside_failed_ids() {
    let mut h = configured_report(CALM, ReportFormat::Junit, &[b"b"], None);
    assert_eq!(run_report(&mut h, Some(b"b\na\nb\n")), Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"a\"/><testcase name=\"b\"><failure/></testcase></testsuite>")));
}
#[test]
fn test_cues_read_ids_from_the_run_commit() {
    let mut h = configured_report(CALM, ReportFormat::Lines, &[], Some(Cue::Tests { path: copy_of(b"ids") }));
    let commit = h.commit(FIRST, &[(b"ids", b"b\na\nb\n")]);
    h.push(ENGINE, MAIN, commit).unwrap();
    assert_eq!(
        run_report(&mut h, None),
        Ok(Answer::Artifact {
            commit,
            artifact: Artifact {
                name: copy_of(REPORT),
                path: copy_of(b"failed.txt"),
                bytes: copy_of(b"a\nb\n"),
                deflate: true
            }
        })
    );
}
#[test]
fn a_faulted_run_uploads_no_report() {
    for config in [Config { lost: 1000, ..CALM }, Config { setup_faults: 1000, ..CALM }] {
        let mut h = configured_report(config, ReportFormat::Junit, &[b"a", b"b"], None);
        assert_eq!(run_report(&mut h, None), Err(Error::Missing(What::File)));
    }
}
#[test]
fn a_passing_run_reports_no_failed_ids() {
    let mut h = configured_report(CALM, ReportFormat::Junit, &[], None);
    assert_eq!(
        run_report(&mut h, None),
        Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite></testsuite>"))
    );
    assert_eq!(run_jobs(&h, 1)[0].conclusion, Some(crate::api::Conclusion::Success));
}

#[test]
fn faulted_jobs_suppress_reports_until_each_fault_is_rerun() {
    let mut h = configured_report_jobs(
        Config { setup_faults: 1000, ..CALM },
        ReportFormat::Lines,
        &[b"a"],
        &[],
        None,
        &[b"a", b"b"],
    );
    drop(run_report(&mut h, None));
    h.env.limits.setup_faults = 0;
    let jobs = run_jobs(&h, 1);
    h.ok(ENGINE, write(Write::Rerun { job: jobs[0].id }));
    h.settle();
    let pending = h.call(ENGINE, read(Read::Artifact { run: 1, name: copy_of(REPORT) }));
    h.ok(ENGINE, write(Write::Rerun { job: jobs[1].id }));
    h.settle();
    let completed = h.call(ENGINE, read(Read::Artifact { run: 1, name: copy_of(REPORT) }));
    assert_eq!(
        (pending, completed),
        (
            Err(Error::Missing(What::File)),
            Ok(Answer::Artifact {
                commit: FIRST,
                artifact: Artifact {
                    name: copy_of(REPORT),
                    path: copy_of(b"failed.txt"),
                    bytes: copy_of(b"a\n"),
                    deflate: true
                }
            })
        )
    );
}

fn flaky_report(fails: &[&[u8]], flaky: &[&[u8]]) -> Harness {
    configured_report_jobs(CALM, ReportFormat::Junit, fails, flaky, None, &[b"tests"])
}
#[test]
fn a_flaky_test_fails_in_a_whole_run_and_its_report_names_it() {
    let mut h = flaky_report(&[b"b"], &[b"c", b"a", b"b", b"a"]);
    let report = run_report(&mut h, None);
    assert_eq!(report, Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"a\"><failure/></testcase><testcase name=\"b\"><failure/></testcase><testcase name=\"c\"><failure/></testcase></testsuite>")));
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Failure));
    assert_eq!(run_jobs(&h, 1)[0].conclusion, Some(crate::api::Conclusion::Failure));
}
#[test]
fn a_flaky_test_passes_when_its_selection_lists_it() {
    let mut h = flaky_report(&[], &[b"a"]);
    drop(run_report(&mut h, None));
    let report = run_report(&mut h, Some(b"a"));
    assert_eq!(
        report,
        Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"a\"/></testsuite>"))
    );
    let runs = workflow_runs(&h);
    assert_eq!((runs[0].commit, runs[1].commit), (FIRST, FIRST));
    assert_eq!(
        (runs[1].conclusion, runs[0].conclusion),
        (Some(crate::api::Conclusion::Failure), Some(crate::api::Conclusion::Success))
    );
    assert_eq!(run_jobs(&h, runs[0].id)[0].conclusion, Some(crate::api::Conclusion::Success));
}
#[test]
fn a_selection_of_a_failing_and_a_flaky_test_fails_only_the_failing_one() {
    let mut h = flaky_report(&[b"b"], &[b"a", b"b"]);
    let report = run_report(&mut h, Some(b"b\na\nb\n"));
    assert_eq!(report, Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite><testcase name=\"a\"/><testcase name=\"b\"><failure/></testcase></testsuite>")));
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Failure));
}
#[test]
fn a_whole_run_merges_flaky_ids_with_its_commit_cue() {
    let mut h = configured_report_jobs(
        CALM,
        ReportFormat::Lines,
        &[b"unused"],
        &[b"c", b"a", b"b"],
        Some(Cue::Tests { path: copy_of(b"ids") }),
        &[b"tests"],
    );
    let commit = h.commit(FIRST, &[(b"ids", b"b\n")]);
    h.push(ENGINE, MAIN, commit).unwrap();
    assert_eq!(
        run_report(&mut h, None),
        Ok(Answer::Artifact {
            commit,
            artifact: Artifact {
                name: copy_of(REPORT),
                path: copy_of(b"failed.txt"),
                bytes: copy_of(b"a\nb\nc\n"),
                deflate: true,
            },
        })
    );
}
#[test]
fn an_empty_selection_runs_no_flaky_ids() {
    let mut h = flaky_report(&[], &[b"a"]);
    assert_eq!(
        run_report(&mut h, Some(b"")),
        Ok(junit(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite></testsuite>"))
    );
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Success));
}

fn bounded_flaky_workflow(flaky: &[u8]) -> Harness {
    // Each id uses the fixture's content_bytes bound of 32, independently.
    let configured = Setup {
        workflows: Box::new([crate::api::Workflow {
            fails: names(&[b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]),
            flaky: names(&[flaky]),
            ..workflow(&[b"tests"])
        }]),
        ..setup()
    };
    Harness::with(CALM, configured)
}
#[test]
fn a_whole_run_can_merge_two_id_sets_at_their_bound() {
    let mut h = bounded_flaky_workflow(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let dispatched = h.call(ENGINE, dispatch());
    h.settle();
    assert_eq!(dispatched, Ok(Answer::Done));
    assert_eq!(workflow_runs(&h)[0].conclusion, Some(crate::api::Conclusion::Failure));
}
#[test]
#[should_panic(expected = "flaky ids fit: TooLarge")]
fn a_flaky_id_past_its_existing_bound_is_refused() {
    drop(bounded_flaky_workflow(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
}
