use super::{Probe, REPO, Result, number, pause, rows, text};

pub(super) fn record(p: &mut Probe<'_>) -> Result<()> {
    trees(p)?;
    statuses(p)?;
    merges(p)?;
    updates(p)
}
fn trees(p: &mut Probe<'_>) -> Result<()> {
    let f = p.recorder.f;
    f.put(REPO, "main", "tests.rs", "root file\n")?;
    f.put(REPO, "main", "tests/one.rs", "child one\n")?;
    f.put(REPO, "main", "tests/two.rs", "child two\n")?;
    let head = p.head("tree-fixture-head", "main")?;
    let root = root_tree(p)?;
    let name = "tree-commit-id-answers-root-tree";
    let by_commit = p.get(name, &format!("{REPO}/git/trees/{head}?page=1&per_page=2"))?;
    let by_tree = p.get("tree-root-id", &format!("{REPO}/git/trees/{root}?page=1&per_page=2"))?;
    p.check(name, root != head && entries(&by_commit)? == entries(&by_tree)?, "commit and root tree responses differ");
    let name = "tree-entries-use-git-order";
    let page2 = p.get(name, &format!("{REPO}/git/trees/{head}?page=2&per_page=2"))?;
    let first = rows(&by_commit, "tree")?;
    let second = rows(&page2, "tree")?;
    p.check(
        name,
        first.iter().chain(&second).filter_map(|e| e.get("path").str()).collect::<Vec<_>>()
            == ["README.md", "tests.rs", "tests"],
        "expected README.md, tests.rs, tests",
    );
    let name = "tree-directory-id-answers-directory";
    let directory = first.iter().chain(&second).find(|e| e.get("type").is("tree")).ok_or("tree directory missing")?;
    let directory = text(directory.get("sha"))?;
    let child = p.get(name, &format!("{REPO}/git/trees/{directory}?page=1&per_page=2"))?;
    p.check(
        name,
        child.root().get("sha").is(&directory)
            && rows(&child, "tree")?.iter().filter_map(|e| e.get("path").str()).collect::<Vec<_>>()
                == ["one.rs", "two.rs"],
        "directory entries differ",
    );
    let name = "tree-pages-and-truncated";
    let page3 = p.get(name, &format!("{REPO}/git/trees/{head}?page=3&per_page=2"))?;
    p.check(
        name,
        first.len() == 2
            && second.len() == 1
            && rows(&page3, "tree")?.is_empty()
            && by_commit.root().get("truncated").bool() == Some(true)
            && page2.root().get("truncated").bool() == Some(false)
            && page3.root().get("truncated").bool() == Some(false),
        "expected page lengths 2,1,0 and truncated true,false,false",
    );
    let name = "tree-past-last-page-is-an-empty-array";
    let empty = p.get(name, &format!("{REPO}/git/trees/{head}?page=3&per_page=2"))?;
    p.check(
        name,
        empty.root().get("tree").array().is_some_and(|a| a.is_empty()),
        "Forgejo answers tree:null, truncated:false",
    );
    Ok(())
}
fn root_tree(p: &Probe<'_>) -> Result<String> {
    use std::process::Command;
    let scratch = p.recorder.f.instance.scratch();
    let repository = scratch.join("turn.git");
    let git = || {
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", scratch)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    };
    let clone = git()
        .args(["clone", "--quiet", "--bare"])
        .arg(format!("{}/fixture/turn.git", p.recorder.f.instance.base()))
        .arg(&repository)
        .output()?;
    if !clone.status.success() {
        return Err("cannot clone public throwaway tree fixture".into());
    }
    let root = git().arg("-C").arg(&repository).args(["rev-parse", "main^{tree}"]).output()?;
    if !root.status.success() {
        return Err("cannot read throwaway root tree id".into());
    }
    Ok(String::from_utf8(root.stdout)?.trim().to_owned())
}
fn entries(json: &crate::json::Json) -> Result<Vec<(&str, &str, &str, &str)>> {
    rows(json, "tree")?
        .into_iter()
        .map(|e| {
            Ok((
                e.get("path").str().ok_or("tree path missing")?,
                e.get("mode").str().ok_or("tree mode missing")?,
                e.get("type").str().ok_or("tree kind missing")?,
                e.get("sha").str().ok_or("tree object missing")?,
            ))
        })
        .collect()
}
fn statuses(p: &mut Probe<'_>) -> Result<()> {
    let head = p.head("status-fixture-head", "main")?;
    let body = r#"{"state":"success","context":"turn/status","description":"turn status words","target_url":"https://example.invalid/turn/status"}"#;
    p.send("status-write-words-and-link", "POST", &format!("{REPO}/statuses/{head}"), Some(body))?;
    for (name, target, field) in [
        (
            "status-combined-keeps-description-and-target-url",
            format!("{REPO}/commits/{head}/status?page=1&limit=2"),
            "statuses",
        ),
        ("status-list-keeps-description-and-target-url", format!("{REPO}/statuses/{head}?page=1&limit=2"), ""),
    ] {
        let read = p.get(name, &target)?;
        let at = if field.is_empty() { read.root() } else { read.root().get(field) };
        let status = at.array().and_then(|rows| rows.into_iter().find(|s| s.get("context").is("turn/status")));
        p.check(
            name,
            status.is_some_and(|s| {
                s.get("description").is("turn status words")
                    && s.get("target_url").is("https://example.invalid/turn/status")
            }),
            "status words or link differ",
        );
    }
    Ok(())
}
fn pull(p: &mut Probe<'_>, head: &str, base: &str) -> Result<u64> {
    let read = p.recorder.f.pull(REPO, head, base)?;
    number(read.root().get("number"))
}
fn merge(p: &mut Probe<'_>, name: &str, pull: u64, head: &str) -> Result<skein_throwaway_forgejo::Exchange> {
    p.send(
        name,
        "POST",
        &format!("{REPO}/pulls/{pull}/merge"),
        Some(&format!(r#"{{"Do":"fast-forward-only","head_commit_id":"{head}"}}"#)),
    )
}
fn merges(p: &mut Probe<'_>) -> Result<()> {
    let f = p.recorder.f;
    f.branch(REPO, "ff-base", "main")?;
    f.branch(REPO, "ff-head", "ff-base")?;
    f.put(REPO, "ff-head", "ff.txt", "fast forward\n")?;
    let head = p.head("merge-ff-head", "ff-head")?;
    let pr = pull(p, "ff-head", "ff-base")?;
    f.ready(REPO, pr, None)?;
    let name = "merge-fast-forward-only-moves-base-to-head";
    let answer = merge(p, name, pr, &head)?;
    let base = p.head("merge-ff-base-after", "ff-base")?;
    p.fact(name, answer.status == 200 && answer.body.is_empty() && base == head, &answer);
    let name = "merge-fast-forward-only-marks-pull-merged-at-head";
    let merged = p.get(name, &format!("{REPO}/pulls/{pr}"))?;
    p.check(
        name,
        merged.root().get("merged").bool() == Some(true) && merged.root().get("merge_commit_sha").is(&head),
        "merged flag or merge commit differs",
    );
    f.branch(REPO, "ff-diverged-head", "main")?;
    f.branch(REPO, "ff-diverged-base", "main")?;
    f.put(REPO, "ff-diverged-head", "head.txt", "head\n")?;
    f.put(REPO, "ff-diverged-base", "base.txt", "base\n")?;
    let head = p.head("merge-diverged-head", "ff-diverged-head")?;
    let before = p.head("merge-diverged-base-before", "ff-diverged-base")?;
    let pr = pull(p, "ff-diverged-head", "ff-diverged-base")?;
    f.ready(REPO, pr, None)?;
    let name = "merge-fast-forward-only-refuses-diverged-head";
    let answer = merge(p, name, pr, &head)?;
    let after = p.head("merge-diverged-base-after", "ff-diverged-base")?;
    p.fact(name, answer.status == 500 && before == after, &answer);
    let name = "merge-fast-forward-only-diverged-is-405";
    let answer = merge(p, name, pr, &head)?;
    p.fact(name, answer.status == 405, &answer);
    Ok(())
}
fn updates(p: &mut Probe<'_>) -> Result<()> {
    let f = p.recorder.f;
    f.branch(REPO, "rebase-base", "main")?;
    f.branch(REPO, "rebase-head", "rebase-base")?;
    f.put(REPO, "rebase-head", "first.txt", "first\n")?;
    f.put(REPO, "rebase-head", "second.txt", "second\n")?;
    let old = p.head("update-rebase-old-head", "rebase-head")?;
    f.put(REPO, "rebase-base", "base.txt", "base\n")?;
    let base = p.head("update-rebase-base-tip", "rebase-base")?;
    let pr = pull(p, "rebase-head", "rebase-base")?;
    f.ready(REPO, pr, None)?;
    let name = "update-rebase-rewrites-onto-base-and-keeps-messages";
    let answer = p.send(name, "POST", &format!("{REPO}/pulls/{pr}/update?style=rebase"), None)?;
    let head = wait_head(p, "update-rebase-new-head", "rebase-head", &old)?;
    let second = p.get("update-rebase-second-commit", &format!("{REPO}/git/commits/{head}"))?;
    let parent = text(second.root().get("parents").nth(0).get("sha"))?;
    let first = p.get("update-rebase-first-commit", &format!("{REPO}/git/commits/{parent}"))?;
    p.fact(
        name,
        answer.status == 200
            && head != old
            && first.root().get("parents").nth(0).get("sha").is(&base)
            && first.root().get("commit").get("message").str().is_some_and(|m| m.trim_end() == "first.txt")
            && second.root().get("commit").get("message").str().is_some_and(|m| m.trim_end() == "second.txt"),
        &answer,
    );
    let name = "update-rebase-containing-base-changes-nothing";
    let answer = p.send(name, "POST", &format!("{REPO}/pulls/{pr}/update?style=rebase"), None)?;
    let after = p.head("update-rebase-containing-head-after", "rebase-head")?;
    p.fact(name, answer.status == 200 && after == head, &answer);
    f.branch(REPO, "rebase-conflict-head", "main")?;
    f.branch(REPO, "rebase-conflict-base", "main")?;
    f.put(REPO, "rebase-conflict-head", "tests.rs", "head conflict\n")?;
    f.put(REPO, "rebase-conflict-base", "tests.rs", "base conflict\n")?;
    let before = p.head("update-rebase-conflict-head-before", "rebase-conflict-head")?;
    let pr = pull(p, "rebase-conflict-head", "rebase-conflict-base")?;
    let name = "update-rebase-conflict-is-409-and-keeps-head";
    let answer = p.send(name, "POST", &format!("{REPO}/pulls/{pr}/update?style=rebase"), None)?;
    let after = p.head("update-rebase-conflict-head-after", "rebase-conflict-head")?;
    p.fact(name, answer.status == 409 && before == after, &answer);
    Ok(())
}
fn wait_head(p: &mut Probe<'_>, name: &str, branch: &str, old: &str) -> Result<String> {
    for _ in 0..100 {
        let head = p.head(name, branch)?;
        if head != old {
            return Ok(head);
        }
        pause();
    }
    // Preserve a failed fact when Forgejo refuses or does not rewrite.
    p.head(name, branch)
}
