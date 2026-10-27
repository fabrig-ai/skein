use super::{Probe, REPO, Result, number, pause, rows};
use crate::json::Json;

const WORKFLOW: &str = "name: turn-dispatch\non:\n  workflow_dispatch:\n    inputs:\n      word:\n        required: true\n        type: string\njobs:\n  pass:\n    runs-on: kit-host\n    steps:\n      - run: |\n          printf 'TURN_INPUT=%s\\n' '${{ inputs.word }}'\n          sleep 5\n  fail:\n    runs-on: kit-host\n    steps:\n      - run: |\n          printf 'TURN_FAILED_STEP\\n'\n          exit 1\n";
fn listing(head: &str, workflow: &str, page: u64) -> String {
    format!("{REPO}/actions/runs?workflow={workflow}&head_sha={head}&page={page}&limit=2")
}
fn dispatch(p: &mut Probe<'_>, name: &str, workflow: &str, branch: &str) -> Result<skein_throwaway_forgejo::Exchange> {
    p.send(
        name,
        "POST",
        &format!("{REPO}/actions/workflows/{workflow}/dispatches"),
        Some(&format!(r#"{{"ref":"{branch}","inputs":{{"word":"KIT_TURN_INPUT"}}}}"#)),
    )
}
fn wait_run(p: &mut Probe<'_>, head: &str, previous: u64) -> Result<u64> {
    for _ in 0..300 {
        let read = p.recorder.f.get(&listing(head, "turn.yaml", 1))?;
        if let Some(id) = rows(&read, "workflow_runs")?.first().and_then(|r| r.get("id").u64())
            && id > previous
        {
            return Ok(id);
        }
        pause();
    }
    Err("dispatch run not found in 30 seconds".into())
}
fn settled(p: &mut Probe<'_>, run: u64) -> Result<Json> {
    let target = format!("{REPO}/actions/runs/{run}");
    for _ in 0..900 {
        let read = p.recorder.f.get(&target)?;
        if read.root().get("status").is("failure") || read.root().get("status").is("success") {
            return Ok(read);
        }
        pause();
    }
    Err("dispatch run did not complete in 90 seconds".into())
}
pub(super) fn record(p: &mut Probe<'_>) -> Result<()> {
    let f = p.recorder.f;
    f.api("PATCH", REPO, br#"{"has_actions":true}"#)?;
    f.put(REPO, "main", ".forgejo/workflows/turn.yaml", WORKFLOW)?;
    f.put(REPO, "main", ".forgejo/workflows/other.yaml", &WORKFLOW.replace("turn-dispatch", "other-dispatch"))?;
    f.branch(REPO, "dispatch", "main")?;
    let before = p.head("actions-dispatch-head-before", "dispatch")?;
    for (name, workflow, branch) in [
        ("actions-dispatch-missing-workflow-is-404", "absent.yaml", "dispatch"),
        ("actions-dispatch-missing-branch-is-404", "turn.yaml", "absent"),
    ] {
        let answer = dispatch(p, name, workflow, branch)?;
        p.fact(name, answer.status == 404, &answer);
    }
    let name = "actions-dispatch-missing-branch-is-400";
    let answer = dispatch(p, name, "turn.yaml", "absent")?;
    p.fact(name, answer.status == 400, &answer);
    let name = "actions-dispatch-is-204-with-no-body";
    let answer = dispatch(p, name, "turn.yaml", "dispatch")?;
    p.fact(name, answer.status == 204 && answer.body.is_empty(), &answer);
    let run = wait_run(p, &before, 0)?;
    let jobs = p.get("actions-dispatch-jobs-before-rerun", &format!("{REPO}/actions/runs/{run}/jobs"))?;
    let job =
        rows(&jobs, "jobs")?.into_iter().find(|j| j.get("name").is("pass")).ok_or("dispatch passing job missing")?;
    let id = number(job.get("id"))?;
    let attempt = number(job.get("attempt"))?;
    let name = "actions-rerun-running-is-400";
    let answer = p.send(name, "POST", &format!("{REPO}/actions/jobs/{id}/rerun"), None)?;
    p.fact(name, answer.status == 400, &answer);
    // Move the branch while this run is in progress; its recorded commit must stay fixed.
    f.put(REPO, "dispatch", "after-dispatch.txt", "after dispatch\n")?;
    let after = p.head("actions-dispatch-head-after-push", "dispatch")?;
    settled(p, run)?;
    let name = "actions-dispatch-run-keeps-head-at-dispatch";
    let read = p.get(name, &format!("{REPO}/actions/runs/{run}"))?;
    p.check(name, before != after && read.root().get("commit_sha").is(&before), "run did not keep dispatch-time head");
    let name = "actions-dispatch-statuses-land-on-dispatch-head";
    let read = p.get(name, &format!("{REPO}/commits/{before}/status?page=1&limit=2"))?;
    let statuses = rows(&read, "statuses")?;
    p.check(
        name,
        read.root().get("sha").is(&before)
            && statuses.len() == 2
            && statuses.iter().all(|s| s.get("url").str().is_some_and(|url| url.ends_with(&before)))
            && statuses.iter().any(|s| s.get("status").is("success"))
            && statuses.iter().any(|s| s.get("status").is("failure")),
        "completed dispatch has total_count:0 and statuses:null; no commit statuses",
    );
    let name = "actions-dispatch-only-workflow-push-starts-no-run";
    let read = p.get(name, &listing(&after, "turn.yaml", 1))?;
    p.check(name, rows(&read, "workflow_runs")?.is_empty(), "push created a dispatch-only workflow run");
    detail(p, run, id, attempt)?;
    let name = "actions-rerun-completed-is-204";
    let answer = p.send(name, "POST", &format!("{REPO}/actions/jobs/{id}/rerun"), None)?;
    p.fact(name, answer.status == 204 && answer.body.is_empty(), &answer);
    let name = "actions-rerun-next-attempt-visible-on-job";
    let mut next = None;
    for _ in 0..300 {
        let read = p.recorder.f.get(&format!("{REPO}/actions/jobs/{id}"))?;
        if read.root().get("attempt").u64().is_some_and(|n| n > attempt) {
            next = Some(read);
            break;
        }
        pause();
    }
    let read = p.get(name, &format!("{REPO}/actions/jobs/{id}"))?;
    p.check(
        name,
        next.is_some() && read.root().get("attempt").u64() == attempt.checked_add(1),
        "job did not advance exactly one attempt",
    );
    settled(p, run)?;
    filters(p, &before, &after)
}
fn detail(p: &mut Probe<'_>, run: u64, job: u64, attempt: u64) -> Result<()> {
    let name = "actions-dispatch-run-names-event-and-trigger-user";
    let read = p.get(name, &format!("{REPO}/actions/runs/{run}"))?;
    p.check(
        name,
        read.root().get("id").u64() == Some(run)
            && read.root().get("event").is("workflow_dispatch")
            && read.root().get("trigger_user").get("id").u64() == Some(p.recorder.f.owner.id)
            && read.root().get("created").str().is_some(),
        "expected event workflow_dispatch, trigger_user.id and created",
    );
    let name = "actions-dispatch-run-names-trigger-event-and-trigger-user";
    let read = p.get(name, &format!("{REPO}/actions/runs/{run}"))?;
    p.check(
        name,
        read.root().get("trigger_event").is("workflow_dispatch")
            && read.root().get("trigger_user").get("id").u64() == Some(p.recorder.f.owner.id)
            && read.root().get("created").str().is_some(),
        "expected trigger_event, trigger_user.id and created",
    );
    let name = "actions-dispatch-run-jobs-list-both-outcomes";
    let read = p.get(name, &format!("{REPO}/actions/runs/{run}/jobs"))?;
    let jobs = rows(&read, "jobs")?;
    p.check(
        name,
        jobs.len() == 2
            && jobs.iter().all(|j| j.get("run_id").u64() == Some(run))
            && jobs.iter().any(|j| j.get("status").is("success"))
            && jobs.iter().any(|j| j.get("status").is("failure")),
        "expected both jobs and outcomes",
    );
    let failed = jobs.iter().find(|j| j.get("name").is("fail")).ok_or("dispatch failed job missing")?;
    let failed = number(failed.get("id"))?;
    let name = "actions-dispatch-job-steps-setup-first-completion-last";
    let read = p.get(name, &format!("{REPO}/actions/jobs/{failed}"))?;
    let steps = rows(&read, "steps")?;
    p.check(
        name,
        steps.len() == 3
            && steps.first().is_some_and(|s| s.get("number").u64() == Some(0) && s.get("name").is("Set up job"))
            && steps.get(1).is_some_and(|s| s.get("status").is("failure"))
            && steps.last().is_some_and(|s| s.get("name").is("Complete job") && s.get("status").is("failure")),
        "expected setup, failed workflow step, failed completion",
    );
    let name = "actions-dispatch-input-reaches-job-log";
    let log = p.send(name, "GET", &format!("{REPO}/actions/jobs/{job}/logs?attempt={attempt}"), None)?;
    p.fact(name, log.status == 200 && String::from_utf8_lossy(&log.body).contains("TURN_INPUT=KIT_TURN_INPUT"), &log);
    Ok(())
}
fn filters(p: &mut Probe<'_>, before: &str, after: &str) -> Result<()> {
    // Three runs at one head cross a page. Another workflow at that head and
    // this workflow at the old head make either ignored filter observable.
    let other = dispatch(p, "actions-filter-other-workflow-dispatch", "other.yaml", "dispatch")?;
    if other.status != 204 {
        return Err("other workflow dispatch failed".into());
    }
    let mut ids = Vec::new();
    for index in 0..3 {
        let answer = dispatch(p, &format!("actions-filter-dispatch-{index}"), "turn.yaml", "dispatch")?;
        if answer.status != 204 {
            return Err("filter fixture dispatch failed".into());
        }
        let previous = ids.last().copied().unwrap_or(0);
        ids.push(wait_run(p, after, previous)?);
    }
    ids.reverse();
    let name = "actions-runs-filter-workflow-and-head-newest-first";
    let read = p.get(name, &listing(after, "turn.yaml", 1))?;
    let page2 = p.get("actions-runs-filter-page-2", &listing(after, "turn.yaml", 2))?;
    let page3 = p.get("actions-runs-filter-page-3", &listing(after, "turn.yaml", 3))?;
    let old = p.get("actions-runs-filter-old-head", &listing(before, "turn.yaml", 1))?;
    let first = rows(&read, "workflow_runs")?;
    let second = rows(&page2, "workflow_runs")?;
    let all = first.iter().chain(&second).copied().collect::<Vec<_>>();
    let observed = all.iter().filter_map(|r| r.get("id").u64()).collect::<Vec<_>>();
    p.check(
        name,
        first.len() == 2
            && second.len() == 1
            && rows(&page3, "workflow_runs")?.is_empty()
            && observed == ids
            && all.iter().all(|r| {
                r.get("workflow_id").is("turn.yaml")
                    && r.get("commit_sha").is(after)
                    && r.get("trigger_event").is("workflow_dispatch")
                    && r.get("trigger_user").get("id").u64() == Some(p.recorder.f.owner.id)
            })
            && rows(&old, "workflow_runs")?.len() == 1,
        "head_sha and newest order hold; workflow is ignored: page 2 also contains other.yaml",
    );
    // Finish every queued job before the fixture's runner is stopped.
    for id in ids {
        settled(p, id)?;
    }
    let others = p.get("actions-filter-other-workflow-run", &listing(after, "other.yaml", 1))?;
    for row in rows(&others, "workflow_runs")? {
        settled(p, number(row.get("id"))?)?;
    }
    Ok(())
}
