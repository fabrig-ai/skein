//! A dispatched upload and the raw artifact routes on the pinned fixture.
use super::{Probe, REPO, Result, actions, number, rows};
use skein_throwaway_forgejo::Exchange;

const WORKFLOW: &str = "name: artifact-dispatch\non:\n  workflow_dispatch:\njobs:\n  upload:\n    runs-on: kit-host\n    timeout-minutes: 1\n    steps:\n      - run: printf 'a\\nb\\n' > failed.txt\n      - uses: https://code.forgejo.org/actions/upload-artifact@v3\n        with:\n          name: tests\n          path: failed.txt\n      - uses: https://code.forgejo.org/actions/upload-artifact@v3\n        with:\n          name: other\n          path: failed.txt\n";

pub(super) fn record(p: &mut Probe<'_>) -> Result<()> {
    p.recorder.f.put(REPO, "main", ".forgejo/workflows/artifact.yaml", WORKFLOW)?;
    let head = p.head("actions-artifact-dispatch-head", "main")?;
    let dispatched = p.send(
        "actions-artifact-dispatch",
        "POST",
        &format!("{REPO}/actions/workflows/artifact.yaml/dispatches"),
        Some(r#"{"ref":"main"}"#),
    )?;
    if dispatched.status != 204 {
        return Err(format!("artifact dispatch: HTTP {}", dispatched.status).into());
    }
    let run = actions::wait_workflow(p, &head, "artifact.yaml", 0)?;
    let settled = actions::settled(p, run);
    let observed = p.get("actions-artifact-run", &format!("{REPO}/actions/runs/{run}"))?;
    p.check(
        "actions-artifact-run-read-keeps-dispatch-head",
        observed.root().get("commit_sha").is(&head),
        "run.commit_sha did not name the dispatch head",
    );
    let jobs = p.get("actions-artifact-jobs", &format!("{REPO}/actions/runs/{run}/jobs"))?;
    for job in rows(&jobs, "jobs")? {
        let id = number(job.get("id"))?;
        let attempt = number(job.get("attempt"))?;
        p.send(
            &format!("actions-artifact-job-{id}-log"),
            "GET",
            &format!("{REPO}/actions/jobs/{id}/logs?attempt={attempt}"),
            None,
        )?;
    }
    let all = p.get("actions-artifacts-all", &format!("{REPO}/actions/runs/{run}/artifacts"))?;
    let named = p.get("actions-artifacts-named", &format!("{REPO}/actions/runs/{run}/artifacts?name=tests"))?;
    let absent = p.get("actions-artifacts-absent", &format!("{REPO}/actions/runs/{run}/artifacts?name=absent"))?;
    let unknown = p.send(
        "actions-artifacts-unknown-run",
        "GET",
        &format!("{REPO}/actions/runs/9999/artifacts?name=tests"),
        None,
    )?;
    p.fact("actions-artifacts-unknown-run-is-404", unknown.status == 404, &unknown);
    settled?;
    p.check(
        "actions-artifact-upload-completes",
        observed.root().get("status").is("success") && rows(&all, "artifacts")?.len() == 2,
        "the upload job did not complete with both named artifacts; see actions-artifact job log",
    );
    if !observed.root().get("status").is("success") {
        return Err(
            "artifact upload failed: recorded actions-artifact-run, actions-artifact-jobs and its job log".into()
        );
    }
    let named = rows(&named, "artifacts")?;
    p.check(
        "actions-artifacts-name-selects-only-that-artifact",
        named.len() == 1 && named.iter().all(|row| row.get("name").is("tests")),
        "the name filter did not return only tests",
    );
    p.check(
        "actions-artifacts-absent-name-is-empty",
        rows(&absent, "artifacts")?.is_empty(),
        "the absent name returned artifacts",
    );
    let artifact = named.iter().find(|row| row.get("name").is("tests")).ok_or("named uploaded artifact missing")?;
    let archive = download(p, number(artifact.get("id"))?)?;
    let bound = u32::try_from(archive.body.len())?;
    let file = skein_forge::forgejo::zip::read(&archive.body, bound, bound);
    let correct = file
        .as_ref()
        .is_ok_and(|file| file.path.as_ref() == b"failed.txt" && file.bytes.as_ref() == b"a\nb\n" && !file.truncated);
    p.check(
        "actions-artifact-zip-keeps-one-failed-id-file",
        correct,
        "archive did not decode as one failed.txt containing a and b",
    );
    if !correct {
        return Err(format!("artifact ZIP did not decode as the uploaded file: {file:?}").into());
    }
    Ok(())
}

fn download(p: &mut Probe<'_>, id: u64) -> Result<Exchange> {
    let response = p.send("actions-artifact-zip", "GET", &format!("{REPO}/actions/artifacts/{id}/zip"), None)?;
    p.fact("actions-artifact-zip-is-direct", response.status == 200, &response);
    if response.status == 200 {
        return Ok(response);
    }
    if ![301, 302, 303, 307, 308].contains(&response.status) {
        return Err(format!("artifact ZIP download: HTTP {}", response.status).into());
    }
    let head = std::str::from_utf8(&response.head)?;
    let location = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("location").then(|| value.trim())
        })
        .ok_or("artifact ZIP redirect has no Location")?;
    let base = p.recorder.f.instance.base();
    let target = redirect_target(&base, location).ok_or("artifact ZIP redirect leaves the throwaway forge")?;
    let response = p.send("actions-artifact-zip-target", "GET", target, None)?;
    if response.status != 200 {
        return Err(format!("artifact ZIP redirect target: HTTP {}", response.status).into());
    }
    Ok(response)
}

fn redirect_target<'a>(base: &str, location: &'a str) -> Option<&'a str> {
    let target = location.strip_prefix(base).unwrap_or(location);
    (target.starts_with('/') && !target.starts_with("//") && !target.contains('#')).then_some(target)
}

#[test]
fn archive_redirects_stay_on_the_fixture_origin() {
    for (location, expected) in [
        ("/fixture/turn/actions/artifacts/1", Some("/fixture/turn/actions/artifacts/1")),
        ("http://127.0.0.1:3000/fixture/turn/actions/artifacts/1", Some("/fixture/turn/actions/artifacts/1")),
        ("http://127.0.0.1:30001/archive", None),
        ("https://other.example/archive", None),
        ("//other.example/archive", None),
        ("/archive#fragment", None),
    ] {
        assert_eq!(redirect_target("http://127.0.0.1:3000", location), expected);
    }
}
