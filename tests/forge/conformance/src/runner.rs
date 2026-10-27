use crate::json;
use crate::json::{At, Json};
use crate::{
    Result,
    facts::{self, Fact, Outcome},
    fixture::{Fixture, R},
    mask::{self, Mask},
    registration,
    write::{self, SecretKind, Secrets},
};
use skein_throwaway_forgejo::{Exchange, Range};
use std::{
    fs::{self, File},
    path::Path,
    process::{Child, Command},
    time::Duration,
};
const NAMES: [&str; 13] = [
    "actions-report-each-job-as-a-status-on-its-head",
    "actions-status-link-is-runs-n-jobs-i",
    "actions-status-link-names-no-attempt",
    "run-number-is-index-in-repo",
    "run-jobs-come-in-index-order",
    "job-log-range-answers-206",
    "job-steps-number-from-0-set-up-first-complete-last",
    "failed-workflow-step-and-completion-step-are-failure",
    "step-log-holds-that-steps-lines-alone",
    "step-log-suffix-range-answers-206-with-its-last-bytes",
    "step-log-suffix-content-range-names-its-first-byte",
    "empty-log-suffix-range-answers-206-empty",
    "step-past-the-last-is-404",
];
pub(crate) struct Runner {
    child: Option<Child>,
}
impl Drop for Runner {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _kill = child.kill();
            let _wait = child.wait();
        }
    }
}
pub(crate) fn command(binary: &Path, scratch: &Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.env_clear().env("HOME", scratch.join("home")).env("PATH", "/usr/bin:/bin").current_dir(scratch);
    cmd
}
pub fn run(
    binary: Option<&Path>,
    f: &Fixture,
    out: &Path,
    mask: &mut Mask,
    secrets: &mut Secrets,
) -> Result<(String, Vec<Fact>)> {
    let mut facts = if let Some(binary) = binary { registration::facts(f, binary, secrets)? } else { Vec::new() };
    let (version, actions) = run_with(binary, f, Some(out), mask, secrets)?;
    facts.extend(actions);
    Ok((version, facts))
}
pub fn component(binary: Option<&Path>, f: &Fixture, secrets: &mut Secrets) -> Result<()> {
    run_with(binary, f, None, &mut Mask::new(f.instance.port()), secrets)?;
    Ok(())
}
fn run_with(
    binary: Option<&Path>,
    f: &Fixture,
    out: Option<&Path>,
    mask: &mut Mask,
    secrets: &mut Secrets,
) -> Result<(String, Vec<Fact>)> {
    let Some(binary) = binary else {
        return Ok(("none".into(), unchecked_facts(out.is_some())));
    };
    let (_runner, version) = start(f, binary, secrets)?;
    let online = registration::online(f)?;
    f.api("PATCH", R, &json::object(&[("has_actions", &true)])?)?;
    f.branch(R, "actions", "landing")?;
    f.put(R,"actions",".forgejo/workflows/conformance.yaml","name: conformance\non: push\njobs:\n  pass:\n    runs-on: kit-host\n    steps:\n      - run: exit 0\n  fail:\n    runs-on: kit-host\n    steps:\n      - run: |\n          printf 'KIT_CONFORMANCE_FAILED_JOB\\n'\n          exit 1\n")?;
    let head = f
        .get(&format!("{R}/branches/actions"))?
        .root()
        .get("commit")
        .get("id")
        .str()
        .ok_or("actions head missing")?
        .to_owned();
    let target = format!("{R}/commits/{head}/status?page=1&limit=2");
    let mut settled = None;
    for _ in 0..120 {
        let row = f.get(&target)?;
        if row.root().get("statuses").array().is_some_and(|s| {
            s.len() == 2 && s.iter().all(|s| s.get("status").is("success") || s.get("status").is("failure"))
        }) {
            settled = Some(row);
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let combined = settled.ok_or("Actions jobs did not settle in 60 seconds")?;
    capture(f, out, mask, secrets, "actions-combined-status", &target, None)?;
    let statuses = combined.root().get("statuses").array().ok_or("statuses missing")?;
    let link = combined.root().get("statuses").nth(0).get("target_url").str().ok_or("status link missing")?;
    let number =
        link.split("/actions/runs/").nth(1).and_then(|s| s.split('/').next()).ok_or("status link run missing")?;
    let runs_response = exchange(
        f,
        out,
        mask,
        secrets,
        "actions-runs",
        &format!("{R}/actions/runs?head_sha={head}&run_number={number}&page=1&limit=1"),
        None,
    )?;
    let runs = crate::json(&runs_response)?;
    let run = rows(&runs, "workflow_runs").into_iter().next().ok_or("Actions run missing")?;
    let id = run.get("id").u64().ok_or("run id missing")?;
    capture(f, out, mask, secrets, "actions-run", &format!("{R}/actions/runs/{id}"), None)?;
    let jobs = capture(f, out, mask, secrets, "actions-jobs", &format!("{R}/actions/runs/{id}/jobs"), None)?;
    let jobs = rows(&jobs, "jobs");
    let job = jobs
        .iter()
        .find(|j| j.get("name").is("fail") || j.get("status").is("failure") || j.get("conclusion").is("failure"))
        .ok_or("failed job missing")?;
    let job_id = job.get("id").u64().ok_or("job id missing")?;
    let attempt = job.get("attempt").u64().ok_or("job attempt missing")?;
    let log = format!("{R}/actions/jobs/{job_id}/logs?attempt={attempt}");
    let whole = exchange(f, out, mask, secrets, "actions-job-log", &log, None)?;
    let response =
        f.exchange("GET", &log, None, Some(&f.writer.token), Some(skein_throwaway_forgejo::Range::Head(3)))?;
    record(out, mask, secrets, "actions-job-log-range", &response)?;
    let mut facts = Vec::new();
    connection_facts(&mut facts, &statuses, online);
    facts::add(
        &mut facts,
        NAMES[0],
        combined.root().get("sha").is(&head)
            && statuses.iter().all(|s| s.get("url").str().is_some_and(|url| url.ends_with(&head)))
            && statuses.iter().any(|s| s.get("status").is("success"))
            && statuses.iter().any(|s| s.get("status").is("failure")),
        "job statuses did not name the workflow head and both outcomes",
    );
    facts::add(&mut facts, NAMES[1], statuses.iter().all(|s| s.get("target_url").str().is_some_and(valid_link)), link);
    facts::add(
        &mut facts,
        NAMES[2],
        statuses.iter().all(|s| s.get("target_url").str().is_some_and(|s| !s.contains('?') && !s.contains("attempt"))),
        link,
    );
    facts::add(
        &mut facts,
        NAMES[3],
        run.get("run_number").u64() == number.parse().ok() || run.get("index_in_repo").u64() == number.parse().ok(),
        &facts::diagnostic(&runs_response.body),
    );
    facts::add(
        &mut facts,
        NAMES[4],
        jobs_in_index_order(&statuses, &jobs),
        &jobs.iter().map(|j| format!("{:?}", j.get("id").u64())).collect::<Vec<_>>().join(","),
    );
    facts::add(&mut facts, NAMES[5], response.status == 206, &format!("HTTP {}", response.status));
    facts.extend(job_steps(f, out, mask, secrets, &jobs, &whole)?);
    if let Some(out) = out {
        facts.push(crate::job_clone::fact(f)?);
        facts.extend(crate::turn::record(f, out, mask, secrets)?);
    }
    Ok((version, facts))
}
fn unchecked_facts(turn: bool) -> Vec<Fact> {
    let mut facts: Vec<Fact> = NAMES
        .into_iter()
        .chain(registration::NAMES)
        .map(|name| Fact { name, outcome: Outcome::Unchecked("--runner not supplied".into()) })
        .collect();
    if turn {
        facts.extend(crate::turn::unchecked());
    }
    facts
}
fn connection_facts(facts: &mut Vec<Fact>, statuses: &[At<'_>], online: bool) {
    facts::add(
        facts,
        registration::NAMES[7],
        online,
        "connection did not come online in 30 seconds without a registration file",
    );
    facts::add(
        facts,
        registration::NAMES[8],
        statuses.iter().any(|s| s.get("status").is("success"))
            && statuses.iter().any(|s| s.get("status").is("failure")),
        "connection jobs did not show both outcomes",
    );
}
fn jobs_in_index_order(statuses: &[At<'_>], jobs: &[At<'_>]) -> bool {
    jobs.len() == 2
        && statuses.iter().all(|s| {
            s.get("target_url")
                .str()
                .and_then(|s| s.rsplit('/').next())
                .and_then(|s| s.parse::<usize>().ok())
                .is_some_and(|i| {
                    jobs.get(i).is_some_and(|job| {
                        s.get("context")
                            .is(&format!("conformance / {} (push)", job.get("name").str().unwrap_or_default()))
                    })
                })
        })
}
fn rows<'a>(value: &'a Json, key: &str) -> Vec<At<'a>> {
    value.root().array().or_else(|| value.root().get(key).array()).unwrap_or_default()
}
fn valid_link(link: &str) -> bool {
    let Some(tail) = link.split("/actions/runs/").nth(1) else {
        return false;
    };
    let parts: Vec<_> = tail.split('/').collect();
    parts.len() == 3 && parts[0].parse::<u64>().is_ok() && parts[1] == "jobs" && parts[2].parse::<u64>().is_ok()
}
fn capture(
    f: &Fixture,
    out: Option<&Path>,
    mask: &mut Mask,
    secrets: &Secrets,
    name: &str,
    target: &str,
    range: Option<skein_throwaway_forgejo::Range>,
) -> Result<Json> {
    Json::parse(&exchange(f, out, mask, secrets, name, target, range)?.body).or_else(|_| Json::parse(b"null"))
}
fn record(
    out: Option<&Path>,
    mask: &mut Mask,
    secrets: &Secrets,
    name: &str,
    response: &skein_throwaway_forgejo::Exchange,
) -> Result<()> {
    write::check(name, &response.sent, &response.body, secrets)?;
    let request = mask::request(mask, response);
    let response = mask::response(mask, response);
    if let Some(out) = out {
        write::case(out, name, &request, &response, secrets)?;
    }
    Ok(())
}
pub(crate) fn start(f: &Fixture, binary: &Path, secrets: &mut Secrets) -> Result<(Runner, String)> {
    let scratch = f.instance.scratch().join("runner");
    fs::create_dir_all(scratch.join("home"))?;
    let version = version(binary, &scratch)?;
    secrets.add(SecretKind::RunnerSecret, registration::RUNNER_SECRET);
    let secret = scratch.join("secret");
    registration::private(&secret, registration::RUNNER_SECRET)?;
    let registered = registration::register(f, &secret, "kit-conformance", "kit-host:host")?;
    if registered.code != Some(0) || !registered.stderr.is_empty() || registered.stdout != registration::RUNNER_UUID {
        return Err(format!("runner registration: {registered:?}").into());
    }
    let config = scratch.join("config.yaml");
    fs::write(
        &config,
        registration::configuration(&f.instance.base(), registration::RUNNER_UUID, &secret, &scratch.join("jobs")),
    )?;
    let log = File::create(scratch.join("runner.log"))?;
    let child = command(binary, &scratch)
        .arg("--config")
        .arg(&config)
        .arg("daemon")
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    Ok((Runner { child: Some(child) }, version))
}

pub(crate) fn version(binary: &Path, scratch: &Path) -> Result<String> {
    let output = registration::capture(command(binary, scratch).arg("--version"))?;
    let version = String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or_default().to_owned();
    if !output.status.success() || !skein_throwaway_forgejo::names_runner(&version) {
        return Err(format!("runner {}: {version}", binary.display()).into());
    }
    Ok(version)
}

fn exchange(
    f: &Fixture,
    out: Option<&Path>,
    mask: &mut Mask,
    secrets: &Secrets,
    name: &str,
    target: &str,
    range: Option<Range>,
) -> Result<Exchange> {
    let response = f.exchange("GET", target, None, Some(&f.writer.token), range)?;
    record(out, mask, secrets, name, &response)?;
    Ok(response)
}
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|part| part == needle)
}
fn content_range(exchange: &Exchange) -> Option<String> {
    String::from_utf8_lossy(&exchange.head)
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-range"))
        .map(|(_, value)| value.trim().to_owned())
}
fn job_steps(
    f: &Fixture,
    out: Option<&Path>,
    mask: &mut Mask,
    secrets: &Secrets,
    jobs: &[At<'_>],
    whole: &Exchange,
) -> Result<Vec<Fact>> {
    const SUFFIX: u64 = 10;
    let job = jobs.iter().find(|j| j.get("name").is("fail")).ok_or("failed job missing")?;
    let id = job.get("id").u64().ok_or("job id missing")?;
    let attempt = job.get("attempt").u64().ok_or("job attempt missing")?;
    let response = exchange(f, out, mask, secrets, "actions-job", &format!("{R}/actions/jobs/{id}"), None)?;
    let read = crate::json(&response)?;
    let steps = read.root().get("steps").array().ok_or("job steps missing")?;
    let failed = steps
        .iter()
        .find(|step| step.get("status").is("failure"))
        .and_then(|s| s.get("number").u64())
        .ok_or("failed step missing")?;
    let last = steps.iter().filter_map(|step| step.get("number").u64()).max().ok_or("last step missing")?;
    let log = format!("{R}/actions/jobs/{id}/logs?attempt={attempt}&step=");
    let passing = jobs.iter().find(|j| j.get("name").is("pass")).ok_or("passing job missing")?;
    let targets = [
        ("actions-job-log-step", format!("{log}{failed}"), None),
        ("actions-job-log-step-suffix", format!("{log}{failed}"), Some(Range::Tail(SUFFIX))),
        ("actions-job-log-past-last-step", format!("{log}{}", last + 1), None),
        (
            "actions-job-log-empty-suffix",
            format!(
                "{R}/actions/jobs/{}/logs?attempt={}&step=1",
                passing.get("id").u64().ok_or("passing job id missing")?,
                passing.get("attempt").u64().ok_or("passing job attempt missing")?
            ),
            Some(Range::Tail(SUFFIX)),
        ),
    ];
    let mut captured = Vec::new();
    for (name, target, range) in targets {
        captured.push(exchange(f, out, mask, secrets, name, &target, range)?);
    }
    let [step, suffix, past, empty] = captured.as_slice() else { unreachable!("four captures") };
    let n = step.body.len();
    let tail = usize::try_from(SUFFIX)?;
    let range = content_range(suffix);
    let empty_range = content_range(empty);
    let lengths = (whole.body.len(), n, suffix.body.len(), empty.body.len());
    let statuses = (step.status, suffix.status, empty.status, past.status);
    let seen = format!(
        "steps={}; lengths={lengths:?}; statuses={statuses:?}; suffix={range:?}; empty={empty_range:?}",
        facts::diagnostic(&response.body)
    );
    let numbered = steps.iter().enumerate().all(|(i, step)| step.get("number").u64() == u64::try_from(i).ok());
    let checks = [
        steps.len() >= 3
            && numbered
            && steps.first().is_some_and(|s| s.get("name").is("Set up job"))
            && steps.last().is_some_and(|s| s.get("name").is("Complete job")),
        steps.first().is_some_and(|s| s.get("status").is("success"))
            && 0 < failed
            && failed < last
            && steps.last().is_some_and(|s| s.get("status").is("failure")),
        step.status == 200
            && contains(&step.body, b"KIT_CONFORMANCE_FAILED_JOB")
            && n < whole.body.len()
            && contains(&whole.body, &step.body)
            && !contains(&step.body, b"Run Main")
            && !contains(&step.body, b"Job failed"),
        suffix.status == 206 && n > tail && step.body.get(n.saturating_sub(tail)..) == Some(suffix.body.as_slice()),
        n > tail && range == Some(format!("bytes {}-{}/{n}", n.saturating_sub(tail), n.saturating_sub(1))),
        empty.status == 206 && empty.body.is_empty() && empty_range.as_deref() == Some("bytes 0--1/0"),
        past.status == 404,
    ];
    let mut facts = Vec::new();
    for (name, holds) in NAMES[6..].iter().zip(checks) {
        facts::add(&mut facts, name, holds, &seen);
    }
    Ok(facts)
}
