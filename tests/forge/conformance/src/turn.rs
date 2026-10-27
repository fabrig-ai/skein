//! Recorded turn prerequisites on the pinned Forgejo (forge.md, 9).
use crate::{
    Result,
    cases::Recorder,
    facts::{Fact, add},
    fixture::Fixture,
    json::{At, Json},
    mask::Mask,
    write::Secrets,
};
use skein_throwaway_forgejo::Exchange;
use std::{path::Path, time::Duration};

mod actions;
mod artifacts;
mod git;
const REPO: &str = "/api/v1/repos/fixture/turn";

pub(crate) struct Probe<'a> {
    recorder: Recorder<'a>,
    facts: Vec<Fact>,
}
impl Probe<'_> {
    fn send(&mut self, name: &str, method: &str, target: &str, body: Option<&str>) -> Result<Exchange> {
        self.recorder.send(name, method, target, body, Some(&self.recorder.f.owner.token), None)
    }
    fn get(&mut self, name: &str, target: &str) -> Result<Json> {
        let response = self.send(name, "GET", target, None)?;
        if response.status != 200 {
            return Err(format!("{name}: HTTP {}", response.status).into());
        }
        crate::json(&response)
    }
    fn fact(&mut self, name: &'static str, holds: bool, response: &Exchange) {
        // The recorder has scanned these bytes before any diagnostic is retained.
        let body = self.recorder.mask.bytes(&response.body);
        add(
            &mut self.facts,
            name,
            holds,
            &format!("HTTP {}: {}", response.status, crate::facts::diagnostic(&body).trim_end()),
        );
    }
    fn check(&mut self, name: &'static str, holds: bool, detail: &str) {
        add(&mut self.facts, name, holds, detail);
    }
    fn head(&mut self, name: &str, branch: &str) -> Result<String> {
        text(self.get(name, &format!("{REPO}/branches/{branch}"))?.root().get("commit").get("id"))
    }
}
fn text(at: At<'_>) -> Result<String> {
    Ok(at.str().ok_or("expected string in turn fixture")?.to_owned())
}
fn number(at: At<'_>) -> Result<u64> {
    Ok(at.u64().ok_or("expected number in turn fixture")?)
}
fn rows<'a>(json: &'a Json, field: &str) -> Result<Vec<At<'a>>> {
    let at = json.root().get(field);
    if (field == "tree" || field == "statuses") && at.kind() == Some(skein_json::Kind::Null) {
        return Ok(Vec::new());
    }
    json.root().array().or_else(|| at.array()).ok_or_else(|| format!("expected {field} rows in turn fixture").into())
}
fn pause() {
    std::thread::sleep(Duration::from_millis(100));
}
pub(crate) fn record(f: &Fixture, out: &Path, mask: &mut Mask, secrets: &mut Secrets) -> Result<Vec<Fact>> {
    f.api("POST", "/api/v1/user/repos", br#"{"name":"turn","auto_init":true,"default_branch":"main"}"#)?;
    let mut p = Probe { recorder: Recorder { f, out, mask, secrets }, facts: Vec::new() };
    git::record(&mut p)?;
    actions::record(&mut p)?;
    artifacts::record(&mut p)?;
    Ok(p.facts)
}

pub(crate) fn unchecked() -> Vec<Fact> {
    NAMES
        .iter()
        .map(|name| Fact { name, outcome: crate::facts::Outcome::Unchecked("--runner not supplied".into()) })
        .collect()
}
const NAMES: &[&str] = &[
    "tree-commit-id-answers-root-tree",
    "tree-entries-use-git-order",
    "tree-directory-id-answers-directory",
    "tree-pages-and-truncated",
    "tree-past-last-page-is-an-empty-array",
    "tree-truncated-stays-true-on-short-last-page",
    "status-combined-keeps-description-and-target-url",
    "status-list-keeps-description-and-target-url",
    "merge-fast-forward-only-moves-base-to-head",
    "merge-fast-forward-only-marks-pull-merged-at-head",
    "merge-fast-forward-only-refuses-diverged-head",
    "merge-fast-forward-only-diverged-is-405",
    "update-rebase-rewrites-onto-base-and-keeps-messages",
    "update-rebase-containing-base-changes-nothing",
    "update-rebase-conflict-is-409-and-keeps-head",
    "actions-dispatch-missing-workflow-is-404",
    "actions-dispatch-missing-branch-is-404",
    "actions-dispatch-missing-branch-is-400",
    "actions-dispatch-is-204-with-no-body",
    "actions-rerun-running-is-400",
    "actions-dispatch-run-keeps-head-at-dispatch",
    "actions-dispatch-statuses-land-on-dispatch-head",
    "actions-dispatch-only-workflow-push-starts-no-run",
    "actions-rerun-completed-is-204",
    "actions-rerun-next-attempt-visible-on-job",
    "actions-dispatch-run-names-event-and-trigger-user",
    "actions-dispatch-run-names-trigger-event-and-trigger-user",
    "actions-dispatch-run-jobs-list-both-outcomes",
    "actions-dispatch-job-steps-setup-first-completion-last",
    "actions-dispatch-input-reaches-job-log",
    "actions-runs-filter-workflow-and-head-newest-first",
    "actions-artifact-upload-completes",
    "actions-artifact-run-read-keeps-dispatch-head",
    "actions-artifacts-name-selects-only-that-artifact",
    "actions-artifacts-absent-name-is-empty",
    "actions-artifacts-unknown-run-is-404",
    "actions-artifact-zip-is-direct",
    "actions-artifact-zip-keeps-one-failed-id-file",
];
