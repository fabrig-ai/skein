//! Named cases exercise wire observations and the dialect's own plans.
use crate::{
    Result, fake,
    journal::Made,
    runner::{Runner, Target, percent},
    wire::Request,
};
use skein_fake_forge_domain::api::File;
use skein_forge::{
    api,
    github::{self, request::Method},
};
use skein_forge_conformance::{
    facts::{Fact, Outcome},
    json,
};

#[derive(Debug)]
struct Absent(&'static str);
impl std::fmt::Display for Absent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "input absent: {}", self.0)
    }
}
impl std::error::Error for Absent {}

pub(crate) const NAMES: &[&str] = &[
    "token-expires-in-an-hour",
    "token-narrowed-creates-not-edits",
    "rate-core-5000",
    "jwt-calls-no-rate",
    "viewer-is-bot",
    "user-on-installation-403",
    "bot-permission-is-not-a-role",
    "app-routes-401-on-installation-token",
    "jwt-iss-string",
    "mergeable-null-then-settles",
    "combined-pending-with-no-status",
    "job-log-302-blob",
    "job-log-range-honoured",
    "merge-stale-409",
    "merge-after-move-not-merged",
    "merge-unheld-by-ci",
    "update-stale-422",
    "update-202-later",
    "etag-304-free",
    "log-lists-failed-deliveries",
    "log-cursor-in-link",
    "log-ids-19-digits",
    "log-delivery-whole",
    "log-installation-events-unscoped",
    "installation-target-in-log",
    "approve-own-422",
    "ruleset-approval-405",
    "bypass-always-merges",
    "queue-direct-merge-405",
    "enqueue-stale-unprocessable",
    "enqueue-pending-check-protected",
    "queue-merged-no-merge-commit-sha",
    "queue-dequeued-delivered",
    "signatures-push-gh013",
    "rules-branches-current-user-can-bypass",
    "classic-protection-404-under-rulesets",
    "installation-repositories-at-once",
    "issues-list-pulls",
    "new-issue-may-miss-listing",
    "branch-with-slash-reads",
    "issues-since-inclusive",
    "issues-since-epoch-empty",
    "user-by-id-reads",
    "repository-allows-shown",
    "pulls-422-no-commits",
    "pulls-422-exists",
    "refs-422-exists",
    "refs-delete-missing-422",
    "update-conflict-422",
    "merge-repeat-idempotent",
    "merge-repeat-any-sha-200",
    "compare-files-capped-300",
    "comment-updated-at-on-edit",
    "inactive-hook-visible",
    "manifest-hook-invalid-host-accepted",
    "job-log-host-per-owner",
    "installation-repository-removal-refusals",
];
fn case(r: &mut Runner<'_>, name: &'static str, run: impl FnOnce(&mut Runner<'_>) -> Result<bool>) -> Result<()> {
    observation(r, name, |r| {
        let holds = run(r)?;
        r.fact(name, holds, "wire observation differs from the fact");
        Ok(())
    })
}
fn observation(r: &mut Runner<'_>, name: &'static str, run: impl FnOnce(&mut Runner<'_>) -> Result<()>) -> Result<()> {
    r.next(name)?;
    match run(r) {
        Ok(()) => {}
        Err(error) => match r.wire.take() {
            Some(text) => r.fact(name, false, &format!("wire: {text}")),
            None => match error.downcast_ref::<Absent>() {
                Some(absent) => r.fact(name, false, &absent.to_string()),
                None => return Err(error),
            },
        },
    }
    Ok(())
}
fn files(path: &str, content: &str) -> Vec<File> {
    vec![File { path: fake::boxed(path.as_bytes()), content: fake::boxed(content.as_bytes()) }]
}
fn oid(text: &str) -> Result<api::Commit> {
    github::id::commit(text.as_bytes()).ok_or("invalid commit oid".into())
}
fn pull_doc(r: &mut Runner<'_>, index: usize, number: u64) -> Result<crate::wire::Response> {
    let repo = r.repo(index)?;
    let token = r.token(index)?;
    r.raw(Method::Get, format!("/repos/{repo}/pulls/{number}"), &token, b"")
}
fn merge(r: &mut Runner<'_>, index: usize, number: u64, sha: &str) -> Result<crate::wire::Response> {
    let repo = r.repo(index)?;
    let token = r.token(index)?;
    let body = json::object(&[("merge_method", &"merge"), ("sha", &sha)])?;
    r.raw(Method::Put, format!("/repos/{repo}/pulls/{number}/merge"), &token, &body)
}
fn merge_repeat_idempotent(r: &mut Runner<'_>, number: u64, sha: &str) -> Result<()> {
    r.next("merge-repeat-idempotent")?;
    let first = merge(r, 0, number, sha)?;
    let second = merge(r, 0, number, sha)?;
    match r.plan(0, api::Op::Read(api::Read::Pull { number }))? {
        Ok(api::Answer::Pull(pull)) => {
            let landed = pull.merged.and_then(|commit| github::id::write_commit(&commit));
            r.fact(
                "merge-repeat-idempotent",
                first.status == 200
                    && second.status == 200
                    && first.json()?.root().get("merged").bool() == Some(true)
                    && second.json()?.root().get("merged").bool() == Some(true)
                    && second.json()?.root().get("message").is("Pull Request successfully merged")
                    && first.json()?.root().get("sha").str() == second.json()?.root().get("sha").str()
                    && landed.is_some()
                    && first.json()?.root().get("sha").str()
                        == landed.as_ref().and_then(|sha| std::str::from_utf8(sha).ok()),
                "wire observation differs from the fact",
            );
        }
        Ok(_) => return Err("merged pull plan answer invalid".into()),
        Err(e) => r.fact("merge-repeat-idempotent", false, &format!("merged pull plan failed: {e:?}")),
    }
    Ok(())
}
fn update(r: &mut Runner<'_>, number: u64, sha: &str) -> Result<crate::wire::Response> {
    let repo = r.repo(0)?;
    let token = r.token(0)?;
    let body = json::object(&[("expected_head_sha", &sha)])?;
    r.raw(Method::Put, format!("/repos/{repo}/pulls/{number}/update-branch"), &token, &body)
}
fn enqueue(r: &mut Runner<'_>, index: usize, number: u64, sha: &str) -> Result<crate::wire::Response> {
    let repo = r.repo(index)?;
    let (owner, name) = repo.split_once('/').ok_or("repository name invalid")?;
    let query = github::request::Request::PullGraphql {
        owner: fake::boxed(owner.as_bytes()),
        name: fake::boxed(name.as_bytes()),
        number,
    };
    let encoded = github::request::encode(&query, &fake::bounds()).map_err(|_| "pull query encoding refused")?;
    let token = r.token(index)?;
    let response = r.raw(encoded.method, "/graphql", &token, encoded.body.as_deref().unwrap_or_default())?;
    let document = response.json()?;
    let node = document
        .root()
        .get("data")
        .get("repository")
        .get("pullRequest")
        .get("id")
        .str()
        .ok_or("pull GraphQL node id absent")?
        .to_owned();
    let request = github::request::Request::Enqueue { id: fake::boxed(node.as_bytes()), head: oid(sha)? };
    let encoded = github::request::encode(&request, &fake::bounds()).map_err(|_| "enqueue encoding refused")?;
    let token = r.token(index)?;
    r.raw(
        encoded.method,
        String::from_utf8(encoded.target.into_vec())?,
        &token,
        encoded.body.as_deref().unwrap_or_default(),
    )
}
fn scenario(r: &mut Runner<'_>, index: usize, message: &str) -> Result<(String, String, String, u64)> {
    let first = r.base(index)?;
    let base = r.copy_branch(index, &first)?;
    let (head, sha) = r.branch(index, &first, message, files("probe.txt", message))?;
    let n = r.pull(index, &head, &base, false)?;
    Ok((base, head, sha, n))
}
fn rule(r: &mut Runner<'_>, index: usize, base: &str, approvals: u64, queue: bool, bypass: bool) -> Result<()> {
    r.remove_rules()?;
    let repo = r.repo(index)?;
    let name = r.name("rule");
    r.before(Made::Rules { repo: repo.clone(), name: name.clone() })?;
    let app = r.fixtures.app;
    let body = json::write(|out| {
        out.object_start();
        out.key(b"name");
        out.string(name.as_bytes());
        out.key(b"target");
        out.string(b"branch");
        out.key(b"enforcement");
        out.string(b"active");
        out.key(b"conditions");
        out.object_start();
        out.key(b"ref_name");
        out.object_start();
        out.key(b"include");
        out.array_start();
        out.string(format!("refs/heads/{base}").as_bytes());
        out.array_end();
        out.key(b"exclude");
        out.array_start();
        out.array_end();
        out.object_end();
        out.object_end();
        out.key(b"bypass_actors");
        out.array_start();
        if bypass {
            out.object_start();
            out.key(b"actor_id");
            out.unsigned(app);
            out.key(b"actor_type");
            out.string(b"Integration");
            out.key(b"bypass_mode");
            out.string(b"always");
            out.object_end();
        }
        out.array_end();
        out.key(b"rules");
        out.array_start();
        out.object_start();
        out.key(b"type");
        out.string(b"pull_request");
        out.key(b"parameters");
        out.object_start();
        out.key(b"required_approving_review_count");
        out.unsigned(approvals);
        for name in [
            b"dismiss_stale_reviews_on_push".as_slice(),
            b"require_code_owner_review",
            b"require_last_push_approval",
            b"required_review_thread_resolution",
        ] {
            out.key(name);
            out.boolean(false);
        }
        out.object_end();
        out.object_end();
        if queue {
            queue_rules(out, r.case == "queue-dequeued-delivered");
        }
        out.array_end();
        out.object_end();
    })?;
    let person = r.person(index)?;
    let response = r.raw(Method::Post, format!("/repos/{repo}/rulesets"), &person, &body)?;
    if response.status != 201 || response.json()?.root().get("id").u64().is_none() {
        return Err("person ruleset creation refused".into());
    }
    Ok(())
}
fn queue_rules(out: &mut skein_json::writer::Encoder, squash: bool) {
    out.object_start();
    out.key(b"type");
    out.string(b"required_status_checks");
    out.key(b"parameters");
    out.object_start();
    out.key(b"strict_required_status_checks_policy");
    out.boolean(false);
    out.key(b"required_status_checks");
    out.array_start();
    out.object_start();
    out.key(b"context");
    out.string(b"ci");
    out.object_end();
    out.array_end();
    out.object_end();
    out.object_end();
    out.object_start();
    out.key(b"type");
    out.string(b"merge_queue");
    out.key(b"parameters");
    out.object_start();
    // One entry and no batching wait let a single-pull case proceed immediately.
    for (name, value) in [
        (b"check_response_timeout_minutes".as_slice(), 60),
        (b"max_entries_to_build", 5),
        (b"max_entries_to_merge", 5),
        (b"min_entries_to_merge", 1),
        (b"min_entries_to_merge_wait_minutes", 0),
    ] {
        out.key(name);
        out.unsigned(value);
    }
    out.key(b"grouping_strategy");
    out.string(b"ALLGREEN");
    out.key(b"merge_method");
    out.string(if squash { b"SQUASH" } else { b"MERGE" });
    out.object_end();
    out.object_end();
}
pub(crate) fn run(r: &mut Runner<'_>) -> Result<()> {
    authentication(r)?;
    pulls(r)?;
    rules(r)?;
    repositories(r)?;
    logs(r)?;
    setup(r)?;
    r.next("signatures-push-gh013")?;
    r.facts.push(Fact {
        name: "signatures-push-gh013",
        outcome: Outcome::Unchecked(
            "required signatures are unbuilt in the fake (fake-forge.md 10.2); live is not probed".into(),
        ),
    });
    Ok(())
}
fn authentication(r: &mut Runner<'_>) -> Result<()> {
    case(r, "token-expires-in-an-hour", |r| {
        let before = r.now().as_nanos();
        let id = r.fixtures.installations[0].id;
        r.before(Made::Mint { installation: id, at: before / 1_000_000_000 })?;
        let response = r.app(Method::Post, format!("/app/installations/{id}/access_tokens"), b"")?;
        let expires = response
            .json()?
            .root()
            .get("expires_at")
            .str()
            .and_then(|s| skein_forge::forgejo::time::read(s.as_bytes()))
            .ok_or(Absent("mint expiry"))?;
        Ok(response.status == 201 && expires.as_nanos().abs_diff(before + 3_600_000_000_000) < 2_000_000_000)
    })?;
    case(r, "token-narrowed-creates-not-edits", |r| {
        let token = r.mint(0, true)?;
        let repo = r.repo(0)?;
        let title = r.name("narrowed-issue");
        r.before(Made::Issue { repo: repo.clone(), title: title.clone() })?;
        let body = json::object(&[("body", &title), ("title", &title)])?;
        let response = r.raw(Method::Post, format!("/repos/{repo}/issues"), &token, &body)?;
        let accepted = response.header("x-accepted-github-permissions");
        r.provenance.push(format!(
            "# token-narrowed-creates-not-edits issue POST status {} accepted permissions {}",
            response.status,
            accepted.unwrap_or("absent")
        ));
        if response.status != 201 {
            return Ok(false);
        }
        let number = response.json()?.root().get("number").u64().ok_or(Absent("created issue number"))?;
        let body = json::object(&[("title", &format!("{title}-edited"))])?;
        let edited = r.raw(Method::Patch, format!("/repos/{repo}/issues/{number}"), &token, &body)?;
        r.provenance.push(format!("# token-narrowed-creates-not-edits issue title PATCH status {}", edited.status));
        Ok(accepted.is_some() && edited.is(403, "Resource not accessible by integration"))
    })?;
    case(r, "rate-core-5000", |r| {
        let token = r.token(0)?;
        let repo = r.repo(0)?;
        let response = r.raw(Method::Get, format!("/repos/{repo}"), &token, b"")?;
        Ok(response.status == 200
            && response.header("x-ratelimit-limit") == Some("5000")
            && response.header("x-ratelimit-resource") == Some("core"))
    })?;
    case(r, "jwt-calls-no-rate", |r| {
        let response = r.app(Method::Get, "/app", b"")?;
        Ok(response.status == 200 && response.header("x-ratelimit-limit").is_none())
    })?;
    case(r, "viewer-is-bot", |r| {
        let token = r.token(0)?;
        let response =
            r.raw(Method::Post, "/graphql", &token, br#"{"query":"query { viewer { login databaseId } }"}"#)?;
        Ok(response.status == 200
            && response
                .json()?
                .root()
                .get("data")
                .get("viewer")
                .get("login")
                .str()
                .is_some_and(|login| login == format!("{}[bot]", r.fixtures.slug)))
    })?;
    case(r, "user-on-installation-403", |r| {
        let token = r.token(0)?;
        Ok(r.raw(Method::Get, "/user", &token, b"")?.is(403, "Resource not accessible by integration"))
    })?;
    bot_permission_is_not_a_role(r)?;
    case(r, "app-routes-401-on-installation-token", |r| {
        let token = r.token(0)?;
        let mut holds = true;
        for target in ["/app", "/app/installations?per_page=100&page=1", "/app/hook/config"] {
            holds &= r.raw(Method::Get, target, &token, b"")?.status == 401;
        }
        Ok(holds)
    })?;
    case(r, "jwt-iss-string", |r| {
        let response = r.app(Method::Get, "/app", b"")?;
        let claims = r.jwt.split('.').nth(1).ok_or("JWT claims absent")?;
        let bytes = skein_lib::base64::decode(skein_lib::base64::Alphabet::Url, claims.as_bytes(), 1024)
            .map_err(|_| "JWT claims invalid")?;
        let json = json::Json::parse(&bytes)?;
        Ok(response.status == 200 && json.root().get("iss").str() == Some(r.fixtures.app.to_string().as_str()))
    })?;
    Ok(())
}
fn pulls(r: &mut Runner<'_>) -> Result<()> {
    r.next("mergeable-null-then-settles")?;
    let (base, head, previous, n) = scenario(r, 0, "FAIL")?;
    let created = pull_doc(r, 0, n)?;
    let sha = r.advance(0, &previous, &head, "FAIL initial move", files("probe.txt", "FAIL initial move"))?;
    let immediate = pull_doc(r, 0, n)?;
    r.wait(4);
    let settled = pull_doc(r, 0, n)?;
    r.fact(
        "mergeable-null-then-settles",
        (created.json()?.root().get("mergeable").kind() == Some(skein_json::Kind::Null)
            || immediate.json()?.root().get("mergeable").kind() == Some(skein_json::Kind::Null)
            || r.created_pull_null(n))
            && settled.json()?.root().get("mergeable").bool().is_some(),
        "mergeability was not null on create or after a move, then boolean within the wait",
    );
    case(r, "combined-pending-with-no-status", |r| {
        let first = r.base(0)?;
        let token = r.token(0)?;
        let repo = r.repo(0)?;
        let response =
            r.raw(Method::Get, format!("/repos/{repo}/commits/{first}/status?per_page=100&page=1"), &token, b"")?;
        Ok(response.status == 200
            && response.json()?.root().get("state").is("pending")
            && response.json()?.root().get("statuses").items().is_empty())
    })?;
    r.wait_ci();
    job_logs(r, 0, &sha)?;
    case(r, "merge-stale-409", |r| {
        Ok(merge(r, 0, n, "0000000000000000000000000000000000000000")?
            .is(409, "Head branch was modified. Review and try the merge again."))
    })?;
    case(r, "update-stale-422", |r| {
        Ok(update(r, n, "0000000000000000000000000000000000000000")?
            .is(422, "expected head sha didn’t match current head ref."))
    })?;
    r.next("merge-after-move-not-merged")?;
    let moved = r.advance(0, &sha, &head, "FAIL moved", files("probe.txt", "FAIL moved"))?;
    let response = merge(r, 0, n, &moved)?;
    let after = pull_doc(r, 0, n)?;
    r.fact(
        "merge-after-move-not-merged",
        (response.is(405, "Pull Request is not mergeable")
            || response.is(409, "Head branch was modified. Review and try the merge again."))
            && after.json()?.root().get("merged").bool() == Some(false),
        "head move did not refuse merge with the recorded 405 or 409, or the pull merged",
    );
    r.wait(4);
    case(r, "etag-304-free", |r| {
        let repo = r.repo(0)?;
        let token = r.token(0)?;
        let target = format!("/repos/{repo}");
        let first = r.raw(Method::Get, &target, &token, b"")?;
        let tag = first.header("etag").ok_or(Absent("repository ETag"))?.to_owned();
        let mut request = Request::new(Method::Get, target, Some(&token), b"");
        request.headers.push(("If-None-Match".into(), tag));
        let second = r.ask(&request)?;
        Ok(first.status == 200
            && second.status == 304
            && second.body.is_empty()
            && first.header("x-ratelimit-remaining").is_some()
            && first.header("x-ratelimit-remaining") == second.header("x-ratelimit-remaining"))
    })?;
    case(r, "merge-unheld-by-ci", |r| {
        let answer = r.plan(
            0,
            api::Op::Write(api::Write::Merge { number: n, head: oid(&moved)?, how: skein_forge::api::MergeHow::Style }),
        )?;
        Ok(matches!(answer, Ok(api::Answer::Merged(_))))
    })?;
    merge_repeat_idempotent(r, n, &moved)?;
    case(r, "merge-repeat-any-sha-200", |r| {
        let response = merge(r, 0, n, "0000000000000000000000000000000000000000")?;
        Ok(response.status == 200 && response.json()?.root().get("merged").bool() == Some(true))
    })?;
    case(r, "update-202-later", |r| {
        let (base, head, sha, n) = scenario(r, 0, "head")?;
        let initial = r.base(0)?;
        r.advance(0, &initial, &base, "base", files("base.txt", "base"))?;
        let response = update(r, n, &sha)?;
        r.wait(4);
        let pull = pull_doc(r, 0, n)?;
        Ok(response.status == 202
            && pull.json()?.root().get("head").get("sha").str().is_some_and(|new| new != sha)
            && !head.is_empty())
    })?;
    case(r, "update-conflict-422", |r| {
        let (base, _head, sha, n) = scenario(r, 0, "one")?;
        let initial = r.base(0)?;
        r.advance(0, &initial, &base, "different", files("probe.txt", "different"))?;
        Ok(update(r, n, &sha)?.is(422, "merge conflict between base and head"))
    })?;
    let _ = base;
    Ok(())
}
fn failing_job(r: &mut Runner<'_>, index: usize, sha: &str) -> Result<api::JobAttempt> {
    let answer =
        r.plan(index, api::Op::Read(api::Read::Checks { commit: oid(sha)? }))?.map_err(|_| "checks plan failed")?;
    let api::Answer::Checks(statuses) = answer else {
        return Err("checks answer invalid".into());
    };
    statuses.iter().find_map(|s| s.job).ok_or("failed job absent".into())
}
fn job_logs(r: &mut Runner<'_>, index: usize, sha: &str) -> Result<()> {
    r.next("job-log-302-blob")?;
    let job = failing_job(r, index, sha)?;
    let repo = r.repo(index)?;
    let token = r.token(index)?;
    let response = r.raw(Method::Get, format!("/repos/{repo}/actions/jobs/{}/logs", job.job), &token, b"")?;
    let location = response.header("location").ok_or("job log redirect absent")?.to_owned();
    let path = location
        .split_once("://")
        .and_then(|(_, tail)| tail.split_once('/'))
        .map(|(_, path)| format!("/{path}"))
        .ok_or("invalid log location")?;
    let mut request = Request::new(Method::Get, if r.fake() { path.clone() } else { location.clone() }, None, b"");
    request.blob = true;
    let whole = r.ask(&request)?;
    let plan = r.plan(index, api::Op::Read(api::Read::Job { attempt: job, max_bytes: 4096 }))?;
    r.fact(
        "job-log-302-blob",
        response.status == 302
            && whole.status == 200
            && !whole.body.is_empty()
            && matches!(plan, Ok(api::Answer::Job { .. })),
        "job log redirect did not yield a credential-free blob",
    );
    r.next("job-log-range-honoured")?;
    request.headers.push(("Range".into(), "bytes=0-0".into()));
    let ranged = r.ask(&request)?;
    r.fact(
        "job-log-range-honoured",
        ranged.status == 206
            && ranged.header("content-range") == Some(format!("bytes 0-0/{}", whole.body.len()).as_str())
            && ranged.body == whole.body[..1],
        "Range did not yield the first byte with Content-Range",
    );
    Ok(())
}
fn rules(r: &mut Runner<'_>) -> Result<()> {
    r.next("approve-own-422")?;
    let first = r.base(0)?;
    let base = r.copy_branch(0, &first)?;
    let (head, sha) = r.branch(0, &first, "rule", files("probe.txt", "rule"))?;
    let n = r.pull(0, &head, &base, true)?;
    r.wait(4);
    let repo = r.repo(0)?;
    let person = r.person(0)?;
    let response = r.raw(
        Method::Post,
        format!("/repos/{repo}/pulls/{n}/reviews"),
        &person,
        br#"{"event":"APPROVE","body":"own"}"#,
    )?;
    r.fact(
        "approve-own-422",
        response.is(422, "Unprocessable Entity")
            && response.json()?.root().get("errors").nth(0).is("Review Can not approve your own pull request"),
        "own approval was not refused",
    );
    r.next("ruleset-approval-405")?;
    rule(r, 0, &base, 2, false, false)?;
    let response = merge(r, 0, n, &sha)?;
    r.fact(
        "ruleset-approval-405",
        response.status == 405
            && response.json()?.root().get("message").str().is_some_and(|v| {
                v.starts_with("Repository rule violations found") && v.contains("At least 2 approving reviews")
            }),
        "required reviews did not refuse merge",
    );
    case(r, "rules-branches-current-user-can-bypass", |r| {
        let token = r.token(0)?;
        let response = r.raw(
            Method::Get,
            format!("/repos/{repo}/rules/branches/{}?per_page=100&page=1", percent(base.as_bytes())),
            &token,
            b"",
        )?;
        let answer = r.plan(0, api::Op::Read(api::Read::Protection { branch: fake::boxed(base.as_bytes()) }))?;
        let id = response.json()?.root().nth(0).get("ruleset_id").u64().ok_or(Absent("branch ruleset id"))?;
        let detail = r.raw(Method::Get, format!("/repos/{repo}/rulesets/{id}"), &token, b"")?;
        Ok(response.status == 200
            && detail.status == 200
            && detail
                .json()?
                .root()
                .get("current_user_can_bypass")
                .str()
                .is_some_and(|value| ["never", "always", "pull_requests_only"].contains(&value))
            && matches!(answer, Ok(api::Answer::Protection(Some(_)))))
    })?;
    case(r, "classic-protection-404-under-rulesets", |r| {
        let token = r.token(0)?;
        Ok(r.raw(Method::Get, format!("/repos/{repo}/branches/{}/protection", percent(base.as_bytes())), &token, b"")?
            .is(404, "Branch not protected"))
    })?;
    r.next("bypass-always-merges")?;
    rule(r, 0, &base, 2, false, true)?;
    let answer = r.plan(
        0,
        api::Op::Write(api::Write::Merge { number: n, head: oid(&sha)?, how: skein_forge::api::MergeHow::Style }),
    )?;
    r.fact("bypass-always-merges", matches!(answer, Ok(api::Answer::Merged(_))), "always bypass did not merge");
    queue(r)
}
pub(crate) fn queue(r: &mut Runner<'_>) -> Result<()> {
    let index = r.queue_installation.ok_or("organization installation absent")?;
    r.next("queue-direct-merge-405")?;
    let (base, _head, sha, n) = scenario(r, index, "queue")?;
    rule(r, index, &base, 1, true, false)?;
    r.wait(4);
    let response = merge(r, index, n, &sha)?;
    r.fact(
        "queue-direct-merge-405",
        response.status == 405
            && response
                .json()?
                .root()
                .get("message")
                .str()
                .is_some_and(|m| m.contains("Changes must be made through the merge queue")),
        "direct merge ignored queue",
    );
    case(r, "enqueue-stale-unprocessable", |r| {
        let response = enqueue(r, index, n, "0000000000000000000000000000000000000000")?;
        Ok(response.status == 200
            && response.json()?.root().get("errors").items().iter().any(|e| e.get("type").is("UNPROCESSABLE")))
    })?;
    queue_pending(r, index, n, &sha)?;
    queue_merged(r, index, n, &sha)?;
    observation(r, "queue-dequeued-delivered", |r| queue_dequeued(r, index))?;
    Ok(())
}
fn queue_plan(
    r: &mut Runner<'_>,
    index: usize,
    op: api::Op,
    person: bool,
    reason: &str,
) -> Result<Option<api::Answer>> {
    match r.plan_as(index, op, person)? {
        Ok(answer) => Ok(Some(answer)),
        Err(e) => {
            r.fact(r.case, false, &format!("{reason}: {e:?}"));
            Ok(None)
        }
    }
}
fn queue_approval(r: &mut Runner<'_>, index: usize, number: u64) -> Result<bool> {
    Ok(queue_plan(
        r,
        index,
        api::Op::Write(api::Write::Review {
            number,
            key: fake::boxed(b"approval"),
            verdict: api::Verdict::Approve,
            body: fake::boxed(b"approval"),
        }),
        true,
        "queue approval refused",
    )?
    .is_some())
}
// At most thirteen head reads in 180 seconds and twenty-one group reads in 300.
const CI_POLL_SECONDS: u64 = 15;
fn queue_ci(
    r: &mut Runner<'_>,
    index: usize,
    commit: api::Commit,
    seconds: u64,
    head: bool,
) -> Result<Option<api::Check>> {
    for attempt in 0..=seconds / CI_POLL_SECONDS {
        let Some(answer) =
            queue_plan(r, index, api::Op::Read(api::Read::Checks { commit }), false, "queue ci read failed")?
        else {
            return Ok(None);
        };
        let api::Answer::Checks(checks) = answer else { return Err("queue ci answer invalid".into()) };
        if let Some(ci) = checks.iter().find(|check| check.context.as_ref() == b"ci") {
            match ci.check {
                api::Check::Passed | api::Check::Failed => return Ok(Some(ci.check)),
                api::Check::Pending => {}
            }
        }
        if attempt < seconds / CI_POLL_SECONDS {
            r.wait(CI_POLL_SECONDS);
        }
    }
    r.fact(r.case, false, if head { "queue head ci wait expired" } else { "queue group ci wait expired" });
    Ok(None)
}
fn queue_head(r: &mut Runner<'_>, index: usize, commit: api::Commit) -> Result<bool> {
    match queue_ci(r, index, commit, 180, true)? {
        Some(api::Check::Passed) => Ok(true),
        Some(api::Check::Pending | api::Check::Failed) => {
            r.fact(r.case, false, "queue head ci did not pass");
            Ok(false)
        }
        None => Ok(false),
    }
}
// At most twenty-five branch reads, five seconds apart: 120 seconds live.
const GROUP_POLL_SECONDS: u64 = 5;
const GROUP_POLL_READS: usize = 25;
const BRANCHES_PAGE_SIZE: usize = 100;
fn queue_group_branch(r: &mut Runner<'_>, index: usize, reads: &mut usize) -> Result<Option<Box<[u8]>>> {
    for attempt in 1..=GROUP_POLL_READS {
        *reads = attempt;
        let Some(branches) = queue_plan(
            r,
            index,
            api::Op::Read(api::Read::Branches { prefix: Box::from(&b"gh-readonly-queue/"[..]) }),
            false,
            "queue branches read failed",
        )?
        else {
            return Ok(None);
        };
        let api::Answer::Branches(branches) = branches else {
            return Err("branches answer invalid".into());
        };
        if branches.len() >= BRANCHES_PAGE_SIZE {
            r.fact(r.case, false, "queue branches listing full");
            return Ok(None);
        }
        if let Some(group) = branches.iter().find(|b| b.starts_with(b"gh-readonly-queue/")) {
            return Ok(Some(group.clone()));
        }
        if attempt < GROUP_POLL_READS {
            r.wait(GROUP_POLL_SECONDS);
        }
    }
    r.fact(r.case, false, "queue group branch absent");
    Ok(None)
}
fn queue_group(r: &mut Runner<'_>, index: usize) -> Result<Option<api::Commit>> {
    let mut reads = 0;
    let group = queue_group_branch(r, index, &mut reads);
    r.provenance.push(format!("# queue-dequeued-delivered group branch reads {reads}"));
    let Some(group) = group? else { return Ok(None) };
    let Some(answer) = queue_plan(
        r,
        index,
        api::Op::Read(api::Read::Branch { branch: group.clone() }),
        false,
        "queue group read failed",
    )?
    else {
        return Ok(None);
    };
    let api::Answer::Commit(commit) = answer else {
        return Err("queue group oid absent".into());
    };
    if r.case == "queue-dequeued-delivered" {
        let repo = r.repo(index)?;
        let token = r.token(index)?;
        let target = format!("/repos/{repo}/branches/{}", percent(group.as_ref()).replace("%2F", "/"));
        let response = r.raw(Method::Get, target, &token, b"")?;
        let document = response.json()?;
        let message =
            document.root().get("commit").get("commit").get("message").str().ok_or(Absent("queue group message"))?;
        r.provenance.push(format!("# queue-dequeued-delivered group commit message {message:?}"));
    }
    Ok(Some(commit))
}
fn queue_pending(r: &mut Runner<'_>, index: usize, number: u64, sha: &str) -> Result<()> {
    r.next("enqueue-pending-check-protected")?;
    if !queue_approval(r, index, number)? {
        return Ok(());
    }
    let answer = r.plan(
        index,
        api::Op::Write(api::Write::Merge { number, head: oid(sha)?, how: skein_forge::api::MergeHow::Style }),
    )?;
    let response = enqueue(r, index, number, sha)?;
    r.fact(
        "enqueue-pending-check-protected",
        matches!(answer, Err(api::Error::Protected))
            && response.status == 200
            && response.json()?.root().get("errors").items().iter().any(|e| {
                e.get("type").is("UNPROCESSABLE")
                    && e.get("message")
                        .str()
                        .is_some_and(|message| message.starts_with("Pull request Required status check "))
            }),
        "pending required check did not refuse enqueue as UNPROCESSABLE and Protected",
    );
    Ok(())
}
fn queue_merged(r: &mut Runner<'_>, index: usize, number: u64, sha: &str) -> Result<()> {
    r.next("queue-merged-no-merge-commit-sha")?;
    if !queue_approval(r, index, number)? || !queue_head(r, index, oid(sha)?)? {
        return Ok(());
    }
    let Some(queued) = queue_plan(
        r,
        index,
        api::Op::Write(api::Write::Merge { number, head: oid(sha)?, how: skein_forge::api::MergeHow::Style }),
        false,
        "queue admission refused",
    )?
    else {
        return Ok(());
    };
    if !matches!(queued, api::Answer::Queued) {
        r.fact(r.case, false, "queue merge did not answer Queued");
        return Ok(());
    }
    // The group's own workflow must finish before the queue can merge it.
    for attempt in 0..=300 / CI_POLL_SECONDS {
        let response = pull_doc(r, index, number)?;
        if response.json()?.root().get("merged").bool() == Some(true) {
            let Some(merged) =
                queue_plan(r, index, api::Op::Read(api::Read::Pull { number }), false, "merged pull read failed")?
            else {
                return Ok(());
            };
            r.fact(
                "queue-merged-no-merge-commit-sha",
                response.json()?.root().get("merge_commit_sha").kind().is_none()
                    && matches!(merged, api::Answer::Pull(p) if p.merged.is_some()),
                "queue merge did not use GraphQL for the merged commit",
            );
            return Ok(());
        }
        if attempt < 300 / CI_POLL_SECONDS {
            r.wait(CI_POLL_SECONDS);
        }
    }
    r.fact(r.case, false, "queue merge wait expired before the pull read merged");
    Ok(())
}
fn queue_dequeued(r: &mut Runner<'_>, index: usize) -> Result<()> {
    let repo = r.repo(index)?;
    let token = r.token(index)?;
    let repository = r.raw(Method::Get, format!("/repos/{repo}"), &token, b"")?.json()?;
    let setting = repository.root().get("squash_merge_commit_title").str().unwrap_or("absent");
    r.provenance.push(format!("# queue-dequeued-delivered squash_merge_commit_title {setting}"));
    if setting != "COMMIT_OR_PR_TITLE" {
        r.facts.push(Fact {
            name: r.case,
            outcome: Outcome::Unchecked("repository squash title setting is not COMMIT_OR_PR_TITLE".into()),
        });
        return Ok(());
    }
    let first = r.base(index)?;
    let base = r.copy_branch(index, &first)?;
    let (_intermediate, parent) = r.branch(index, &first, "queue first", files("first.txt", "first"))?;
    let (head, sha) = r.branch(index, &parent, "queue second", files("second.txt", "second"))?;
    let n = r.pull(index, &head, &base, false)?;
    let title =
        api::Op::Write(api::Write::Edit { number: n, title: Some(fake::boxed(b"FAIL queue title")), body: None });
    if queue_plan(r, index, title, false, "queue title edit refused")?.is_none() {
        return Ok(());
    }
    rule(r, index, &base, 1, true, false)?;
    if !queue_approval(r, index, n)? || !queue_head(r, index, oid(&sha)?)? {
        return Ok(());
    }
    let response = enqueue(r, index, n, &sha)?;
    let document = response.json()?;
    if document.root().get("errors").kind().is_some() {
        let messages = document
            .root()
            .get("errors")
            .items()
            .iter()
            .filter_map(|e| e.get("message").str())
            .collect::<Vec<_>>()
            .join("; ");
        r.fact(r.case, false, &format!("queue admission refused: {messages}"));
        return Ok(());
    }
    let Some(commit) = queue_group(r, index)? else { return Ok(()) };
    match queue_ci(r, index, commit, 300, false)? {
        Some(api::Check::Passed) => {
            r.facts.push(Fact {
                name: r.case,
                outcome: Outcome::Unchecked(
                    "squash group's ci passed; title-driven failure is unconfirmed live".into(),
                ),
            });
            return Ok(());
        }
        Some(api::Check::Failed) => {}
        Some(api::Check::Pending) | None => return Ok(()),
    }
    r.wait(5);
    let log = r.app(Method::Get, "/app/hook/deliveries?per_page=100", b"")?;
    let rows = log.json()?;
    r.fact(
        "queue-dequeued-delivered",
        log.status == 200
            && rows
                .root()
                .items()
                .iter()
                .any(|row| row.get("event").is("pull_request") && row.get("action").is("dequeued")),
        "dequeued delivery absent",
    );
    Ok(())
}

#[expect(clippy::too_many_lines, reason = "ordered cases share their owned branches and pull state")]
fn repositories(r: &mut Runner<'_>) -> Result<()> {
    case(r, "installation-repositories-at-once", |r| {
        let token = r.token(0)?;
        let response = r.raw(Method::Get, "/installation/repositories?per_page=100&page=1", &token, b"")?;
        let j = response.json()?;
        Ok(response.status == 200
            && j.root().get("repositories").items().len() == r.fixtures.installations[0].repositories.len()
            && response.header("link").is_none())
    })?;
    r.next("issues-list-pulls")?;
    let (_base, _head, _sha, pull) = scenario(r, 0, "listing")?;
    let token = r.token(0)?;
    let repo = r.repo(0)?;
    let listing = format!("/repos/{repo}/issues?state=all&sort=updated&direction=asc&per_page=100&page=1");
    let mut found = false;
    for attempt in 0..3 {
        let response = r.raw(Method::Get, listing.clone(), &token, b"")?;
        found = response.status == 200
            && response
                .json()?
                .root()
                .items()
                .iter()
                .any(|row| row.get("number").u64() == Some(pull) && row.get("pull_request").kind().is_some());
        if found {
            break;
        }
        if attempt < 2 {
            r.wait(2);
        }
    }
    r.fact("issues-list-pulls", found, "issues listing omitted a pull request after three tries");
    case(r, "new-issue-may-miss-listing", |r| {
        let issue = r.issue(0)?;
        let response = r.raw(
            Method::Get,
            format!("/repos/{repo}/issues?state=all&sort=updated&direction=asc&per_page=100&page=1"),
            &token,
            b"",
        )?;
        let listed = response.json()?;
        let visible = listed.root().items().iter().any(|row| row.get("number").u64() == Some(issue));
        r.provenance.push(format!("# new-issue-may-miss-listing immediately visible {visible}"));
        Ok(issue > 0 && response.status == 200 && listed.root().array().is_some())
    })?;
    case(r, "issues-since-inclusive", |r| {
        let settled = r.raw(Method::Get, listing.clone(), &token, b"")?.json()?;
        let row = settled
            .root()
            .items()
            .iter()
            .find(|row| row.get("number").u64() == Some(pull))
            .copied()
            .ok_or(Absent("settled pull"))?;
        let since = row.get("updated_at").str().ok_or(Absent("issue time"))?.to_owned();
        let issue = pull;
        let response = r.raw(
            Method::Get,
            format!(
                "/repos/{repo}/issues?state=all&since={}&sort=updated&direction=asc&per_page=100&page=1",
                percent(since.as_bytes())
            ),
            &token,
            b"",
        )?;
        let answer =
            r.plan(0, api::Op::Read(api::Read::Items { since: skein_lib::Time::ZERO, page: 1, kind: None }))?;
        Ok(response.status == 200
            && response.json()?.root().items().iter().any(|row| row.get("number").u64() == Some(issue))
            && answer.is_ok())
    })?;
    observation(r, "issues-since-epoch-empty", |r| {
        let issue = r.issue(0)?;
        let mut found = false;
        let mut reads = 0;
        for attempt in 0..3 {
            let response = r.raw(Method::Get, listing.clone(), &token, b"")?;
            reads += 1;
            found = response.status == 200
                && response.json()?.root().items().iter().any(|row| row.get("number").u64() == Some(issue));
            if found {
                break;
            }
            if attempt < 2 {
                r.wait(2);
            }
        }
        r.provenance.push(format!("# issues-since-epoch-empty listing reads {reads}"));
        if !found {
            r.fact(r.case, false, "made issue absent from listing");
            return Ok(());
        }
        let epoch = r.raw(
            Method::Get,
            listing.replace("?state=all", "?state=all&since=1970-01-01T00%3A00%3A00Z"),
            &token,
            b"",
        )?;
        let empty = epoch.status == 200
            && epoch.json().is_ok_and(|json| json.root().array().is_some_and(|rows| rows.is_empty()));
        let answer =
            r.plan(0, api::Op::Read(api::Read::Items { since: skein_lib::Time::ZERO, page: 1, kind: None }))?;
        let listed =
            matches!(answer, Ok(api::Answer::Items { items, .. }) if items.iter().any(|row| row.number == issue));
        r.fact(r.case, empty && listed, "wire observation differs from the fact");
        Ok(())
    })?;
    case(r, "branch-with-slash-reads", |r| {
        let first = r.base(0)?;
        let name = format!("{}/slash", r.name("branch"));
        r.before(Made::Branch { repo: repo.clone(), name: name.clone() })?;
        r.plan(
            0,
            api::Op::Write(api::Write::CreateBranch { branch: fake::boxed(name.as_bytes()), commit: oid(&first)? }),
        )?
        .map_err(|_| "slash branch creation refused")?;
        let answer = r.plan(0, api::Op::Read(api::Read::Branch { branch: fake::boxed(name.as_bytes()) }))?;
        Ok(matches!(answer,Ok(api::Answer::Commit(commit)) if Some(commit)==github::id::commit(first.as_bytes())))
    })?;
    case(r, "user-by-id-reads", |r| {
        let viewer =
            r.raw(Method::Post, "/graphql", &token, br#"{"query":"query { viewer { login databaseId } }"}"#)?;
        let user =
            viewer.json()?.root().get("data").get("viewer").get("databaseId").u64().ok_or(Absent("viewer id"))?;
        let response = r.raw(Method::Get, format!("/user/{user}"), &token, b"")?;
        Ok(response.status == 200 && response.json()?.root().get("id").u64() == Some(user))
    })?;
    case(r, "repository-allows-shown", |r| {
        let response = r.raw(Method::Get, format!("/repos/{repo}"), &token, b"")?;
        let answer = r.plan(0, api::Op::Read(api::Read::Settings))?;
        let json = response.json()?;
        Ok(response.status == 200
            && ["allow_merge_commit", "allow_squash_merge", "allow_rebase_merge"]
                .iter()
                .all(|key| json.root().get(key).bool().is_some())
            && matches!(answer, Ok(api::Answer::Settings(_))))
    })?;
    case(r, "pulls-422-no-commits", |r| {
        let first = r.base(0)?;
        let copy = r.copy_branch(0, &first)?;
        r.before(Made::Pull { repo: repo.clone(), head: copy.clone(), base: "main".into() })?;
        let body = json::object(&[("base", &"main"), ("body", &""), ("head", &copy), ("title", &"empty")])?;
        let response = r.raw(Method::Post, format!("/repos/{repo}/pulls"), &token, &body)?;
        Ok(response.status == 422
            && response
                .json()?
                .root()
                .get("errors")
                .items()
                .iter()
                .any(|e| e.get("message").str().is_some_and(|m| m.starts_with("No commits between "))))
    })?;
    case(r, "pulls-422-exists", |r| {
        let (base, head, _sha, _pull) = scenario(r, 0, "duplicate")?;
        let body = json::object(&[("base", &base), ("body", &""), ("head", &head), ("title", &"duplicate")])?;
        let response = r.raw(Method::Post, format!("/repos/{repo}/pulls"), &token, &body)?;
        Ok(response.status == 422
            && response
                .json()?
                .root()
                .get("errors")
                .items()
                .iter()
                .any(|e| e.get("message").str().is_some_and(|m| m.starts_with("A pull request already exists for "))))
    })?;
    case(r, "refs-422-exists", |r| {
        let first = r.base(0)?;
        let branch = r.copy_branch(0, &first)?;
        let body = json::object(&[("ref", &format!("refs/heads/{branch}")), ("sha", &first)])?;
        Ok(r.raw(Method::Post, format!("/repos/{repo}/git/refs"), &token, &body)?.is(422, "Reference already exists"))
    })?;
    case(r, "refs-delete-missing-422", |r| {
        let name = r.name("absent");
        Ok(r.raw(Method::Delete, format!("/repos/{repo}/git/refs/heads/{name}"), &token, b"")?
            .is(422, "Reference does not exist"))
    })?;
    case(r, "compare-files-capped-300", |r| {
        let first = r.base(0)?;
        let files = (0..301)
            .map(|n| File { path: fake::boxed(format!("file-{n}.txt").as_bytes()), content: fake::boxed(b"new") })
            .collect();
        let (_branch, sha) = r.branch(0, &first, "many files", files)?;
        let response =
            r.raw(Method::Get, format!("/repos/{repo}/compare/{first}...{sha}?per_page=100"), &token, b"")?;
        Ok(response.status == 200 && response.json()?.root().get("files").items().len() == 300)
    })?;
    case(r, "comment-updated-at-on-edit", |r| {
        let issue = r.issue(0)?;
        let response =
            r.raw(Method::Post, format!("/repos/{repo}/issues/{issue}/comments"), &token, br#"{"body":"before"}"#)?;
        let j = response.json()?;
        let id = j.root().get("id").u64().ok_or(Absent("comment id"))?;
        let listed = r
            .raw(Method::Get, format!("/repos/{repo}/issues/{issue}/comments?per_page=100&page=1"), &token, b"")?
            .json()?;
        let before = listed
            .root()
            .items()
            .iter()
            .find(|row| row.get("id").u64() == Some(id))
            .and_then(|row| row.get("updated_at").str())
            .ok_or(Absent("comment update time"))?
            .to_owned();
        r.wait(2);
        let response =
            r.raw(Method::Patch, format!("/repos/{repo}/issues/comments/{id}"), &token, br#"{"body":"after"}"#)?;
        let edited = response.json()?;
        Ok(response.status == 200
            && edited.root().get("body").is("after")
            && edited.root().get("updated_at").str().is_some_and(|after| after > before.as_str()))
    })?;
    case(r, "job-log-host-per-owner", |r| {
        let mut hosts = vec![];
        for index in 0..2 {
            let first = r.base(index)?;
            let (_branch, sha) = r.branch(index, &first, "FAIL owner probe", files("probe.txt", "FAIL"))?;
            r.wait_ci();
            let job = failing_job(r, index, &sha)?;
            let repo = r.repo(index)?;
            let token = r.token(index)?;
            let response = r.raw(Method::Get, format!("/repos/{repo}/actions/jobs/{}/logs", job.job), &token, b"")?;
            if response.status != 302 {
                return Ok(false);
            }
            let location = response.header("location").ok_or(Absent("job log redirect"))?;
            let (_, tail) = location.split_once("://").ok_or("job log redirect invalid")?;
            hosts.push(tail.split('/').next().ok_or(Absent("job log host"))?.to_owned());
        }
        r.provenance.push(format!(
            "# job-log-host-per-owner redirect hosts {}",
            if hosts[0] == hosts[1] { "equal" } else { "differ" }
        ));
        Ok(hosts.iter().all(|host| !host.is_empty()))
    })?;
    observation(r, "installation-repository-removal-refusals", access)?;
    Ok(())
}
fn access(r: &mut Runner<'_>) -> Result<()> {
    let mut holds = true;
    for index in 0..2 {
        let installation = r.fixtures.installations[index].id;
        let owner = r.fixtures.installations[index].owner.clone();
        let repo = r.repo(index)?;
        let token = r.token(index)?;
        let id = r
            .raw(Method::Get, format!("/repos/{repo}"), &token, b"")?
            .json()?
            .root()
            .get("id")
            .u64()
            .ok_or(Absent("repository id"))?;
        r.before(Made::Access { installation, repository: id, owner, add: true })?;
        let person = r.person(index)?;
        let response =
            r.raw(Method::Delete, format!("/user/installations/{installation}/repositories/{id}"), &person, b"")?;
        holds &= response.is(422, "Cannot remove the last repository from this installation.");
    }
    r.fact(
        "installation-repository-removal-refusals",
        holds,
        "last-repository removal differed from the observed refusal",
    );
    Ok(())
}
fn logs(r: &mut Runner<'_>) -> Result<()> {
    if r.fake() {
        case(r, "installation-target-in-log", |r| {
            if let Target::Fake(world) = &mut r.target
                && !world.peer.rename_github_installation(1, b"acme-renamed")
            {
                return Err("installed account rename failed".into());
            }
            r.wait(4);
            let response = r.app(Method::Get, "/app/hook/deliveries?per_page=100", b"")?;
            let holds = response.status == 200
                && response.json()?.root().items().iter().any(|row| row.get("event").is("installation_target"));
            if let Target::Fake(world) = &mut r.target {
                world.peer.rename_github_installation(1, b"acme");
            }
            Ok(holds)
        })?;
    } else {
        r.next("installation-target-in-log")?;
        r.facts.push(Fact { name: r.case, outcome: Outcome::Unchecked("the live run renames no account".into()) });
    }
    case(r, "log-lists-failed-deliveries", |r| {
        r.wait(3);
        let response = r.app(Method::Get, "/app/hook/deliveries?per_page=100", b"")?;
        Ok(response.status == 200
            && response
                .json()?
                .root()
                .items()
                .iter()
                .any(|row| row.get("status_code").u64().is_some_and(|code| code >= 400)))
    })?;
    case(r, "log-cursor-in-link", |r| {
        let response = r.app(Method::Get, "/app/hook/deliveries?per_page=1", b"")?;
        Ok(response.status == 200
            && response.header("link").is_some_and(|link| link.contains("cursor=") && link.contains("rel=\"next\"")))
    })?;
    case(r, "log-ids-19-digits", |r| {
        let response = r.app(Method::Get, "/app/hook/deliveries?per_page=100", b"")?;
        let rows = response.json()?;
        Ok(response.status == 200
            && !rows.root().items().is_empty()
            && rows.root().items().iter().all(|row| row.get("id").u64().is_some_and(|id| id.to_string().len() == 19)))
    })?;
    case(r, "log-delivery-whole", |r| {
        let response = r.app(Method::Get, "/app/hook/deliveries?per_page=1", b"")?;
        let id = response.json()?.root().nth(0).get("id").u64().ok_or(Absent("delivery id"))?;
        let response = r.app(Method::Get, format!("/app/hook/deliveries/{id}"), b"")?;
        let json = response.json()?;
        Ok(response.status == 200
            && response.logged_signature(r.fixtures.webhook.as_bytes())?
            && json.root().get("request").get("payload").kind().is_some()
            && json.root().get("request").get("headers").kind().is_some()
            && json.root().get("response").kind().is_some())
    })?;
    case(r, "log-installation-events-unscoped", |r| {
        let spare = r.fixtures.spare.clone();
        let person = r.person(0)?;
        let repo = r.raw(Method::Get, format!("/repos/{spare}"), &person, b"")?;
        let id = repo.json()?.root().get("id").u64().ok_or(Absent("spare id"))?;
        let installation = r.fixtures.installations[0].id;
        let owner = r.fixtures.installations[0].owner.clone();
        r.before(Made::Access { installation, repository: id, owner, add: false })?;
        let response =
            r.raw(Method::Put, format!("/user/installations/{installation}/repositories/{id}"), &person, b"")?;
        if response.status != 204 {
            return Err("spare grant refused".into());
        }
        r.wait(4);
        let response = r.app(Method::Get, "/app/hook/deliveries?per_page=100", b"")?;
        Ok(response.status == 200
            && response.json()?.root().items().iter().any(|row| {
                (row.get("event").is("installation") || row.get("event").is("installation_repositories"))
                    && row.get("installation_id").kind() == Some(skein_json::Kind::Null)
                    && row.get("repository_id").kind() == Some(skein_json::Kind::Null)
            }))
    })?;
    Ok(())
}
fn setup(r: &mut Runner<'_>) -> Result<()> {
    for name in ["inactive-hook-visible", "manifest-hook-invalid-host-accepted"] {
        r.next(name)?;
        if !r.fake() {
            r.facts.push(Fact {
                name,
                outcome: Outcome::Unchecked("live App is read only; fake-only manifest fact (fake-forge.md 11)".into()),
            });
            continue;
        }
        let response = r.app(Method::Get, "/app/hook/config", b"")?;
        let manifest = match &r.target {
            Target::Fake(w) => w.peer.github_manifest(b"example-state"),
            Target::Live(_) => None,
        };
        let hook = manifest.and_then(|m| m.hook_attributes.as_ref());
        let holds = if name == "inactive-hook-visible" {
            hook.is_some_and(|h| !h.active) && response.status == 200
        } else {
            response.status == 200 && response.json()?.root().get("url").is("https://example-app.invalid/")
        };
        r.fact(name, holds, "manifest hook attributes were not retained");
    }
    Ok(())
}

/// App mutations belong only to the hosted fake, after repository restoration and revocation.
pub(crate) fn setup_lifecycle(r: &mut Runner<'_>) -> Result<()> {
    r.next("setup-lifecycle")?;
    if !r.fake() {
        r.provenance.push("# unchecked strays, uninstall, hook-address-set: live App is read only".into());
        return r.flush();
    }
    #[cfg(test)]
    {
        r.phase = "setup-lifecycle";
    }
    let list = r.app(Method::Get, "/app/installations?per_page=100&page=1", b"")?;
    if list.status != 200 || list.json()?.root().items().len() != 2 {
        return Err("fake stray installations absent".into());
    }
    let hook = r.app(Method::Patch, "/app/hook/config", br#"{"url":"https://hook.example.test/github"}"#)?;
    let read = r.app(Method::Get, "/app/hook/config", b"")?;
    if hook.status != 200 || !read.json()?.root().get("url").is("https://hook.example.test/github") {
        return Err("fake hook address was not set".into());
    }
    for installation in r.fixtures.installations.clone() {
        let response = r.app(Method::Delete, format!("/app/installations/{}", installation.id), b"")?;
        let read = r.app(Method::Get, format!("/app/installations/{}", installation.id), b"")?;
        if response.status != 204 || read.status != 404 {
            return Err("fake stray uninstall failed".into());
        }
    }
    r.provenance.push("# holds strays, uninstall, hook-address-set: hosted fake lifecycle".into());
    r.flush()
}

pub(crate) fn bot_permission_is_not_a_role(r: &mut Runner<'_>) -> Result<()> {
    case(r, "bot-permission-is-not-a-role", |r| {
        let installation = r.fixtures.installations[0].id;
        let at = r.now().as_nanos() / 1_000_000_000;
        r.before(Made::Mint { installation, at })?;
        let mint = r.app(Method::Post, format!("/app/installations/{installation}/access_tokens"), b"")?;
        let mint_json = mint.json()?;
        let permissions = mint_json.root().get("permissions");
        let contents = permissions.get("contents").str();
        let pulls = permissions.get("pull_requests").str();
        r.provenance
            .push(format!("# bot-permission-is-not-a-role full mint contents {contents:?} pull_requests {pulls:?}"));
        let token = r.token(0)?;
        let repo = r.repo(0)?;
        let login = percent(format!("{}[bot]", r.fixtures.slug).as_bytes());
        let route = r.raw(Method::Get, format!("/repos/{repo}/collaborators/{login}/permission"), &token, b"")?;
        let route_json = route.json()?;
        let permission = route_json.root().get("permission").str();
        let role = route_json.root().get("role_name").str();
        r.provenance
            .push(format!("# bot-permission-is-not-a-role collaborator permission {permission:?} role_name {role:?}"));
        Ok(mint.status == 201
            && contents == Some("write")
            && pulls == Some("write")
            && route.status == 200
            && permission == Some("none")
            && role == Some(""))
    })
}
