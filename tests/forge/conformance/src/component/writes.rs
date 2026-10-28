use super::{
    Hosted, Operation,
    reads::{bytes, commit, repository},
};
use crate::json;
use crate::{
    Result,
    facts::Fact,
    fixture::{Fixture, R},
};
use skein_forge::api::{self, Answer, Read, Write};
pub(super) fn run(f: &Fixture, h: &mut Hosted, facts: &mut Vec<Fact>) -> Result<()> {
    let repo = repository(f)?;
    let issue = h.call(
        repo,
        Write::CreateIssue {
            key: Box::new([1, 0, 0, 1]),
            title: bytes("component issue"),
            body: bytes("component body"),
            labels: Box::new([]),
        }
        .operation(),
    );
    let number = if let Ok(Answer::Created(n)) = issue { n } else { 9999 };
    let route = format!("{R}/issues/{number}");
    let got = f.get(&route)?;
    facts.push(super::fact(
        "component-write-create-issue",
        matches!(issue, Ok(Answer::Created(_)))
            && got.root().get("title").is("component issue")
            && got.root().get("body").is("<!-- kit:key 01000001 -->\ncomponent body"),
        &issue,
    ));
    let pull = h.call(
        repo,
        Write::OpenPull {
            title: bytes("component pull"),
            body: bytes("pull body"),
            head: bytes("writer-head"),
            base: bytes("landing"),
        }
        .operation(),
    );
    let own = if let Ok(Answer::Created(n)) = pull { n } else { 9999 };
    let own_route = format!("{R}/pulls/{own}");
    let got = f.get(&own_route)?;
    facts.push(super::fact(
        "component-write-open-pull",
        matches!(pull, Ok(Answer::Created(_)))
            && got.root().get("title").is("component pull")
            && got.root().get("head").get("ref").is("writer-head")
            && got.root().get("base").get("ref").is("landing")
            && got.root().get("body").is("pull body"),
        &pull,
    ));
    let post = h.call(repo, Write::Post { number, key: Box::new([1, 0, 0, 2]), body: bytes("post body") }.operation());
    let comments = f.get(&format!("{route}/comments"))?;
    facts.push(super::fact("component-write-post", matches!(post, Ok(Answer::Commented(id)) if comments.root().array().is_some_and(|rows| rows.iter().any(|r| r.get("id").u64() == Some(id) && r.get("body").is("<!-- kit:key 01000002 -->\npost body")))), &post));
    let review = h.call(
        repo,
        Write::Review {
            number: f.feature,
            key: Box::new([1, 0, 0, 3]),
            verdict: api::Verdict::Comment,
            body: bytes("keyed review"),
        }
        .operation(),
    );
    let reviews = f.get(&format!("{R}/pulls/{}/reviews?page=1&limit=50", f.feature))?;
    facts.push(super::fact("component-write-review", matches!(review, Ok(Answer::Reviewed(id)) if reviews.root().array().is_some_and(|rows| rows.iter().any(|r| r.get("id").u64() == Some(id) && r.get("body").is("<!-- kit:key 01000003 -->\nkeyed review") && r.get("state").is("COMMENT")))), &review));
    let edit =
        h.call(repo, Write::Edit { number, title: Some(bytes("edited component issue")), body: None }.operation());
    facts.push(super::fact(
        "component-write-edit",
        edit == Ok(Answer::Done) && f.get(&route)?.root().get("title").is("edited component issue"),
        &edit,
    ));
    let reviewers =
        h.call(repo, Write::SetReviewers { number: f.feature, reviewers: Box::new([f.stranger.id]) }.operation());
    facts.push(super::fact(
        "component-write-set-reviewers",
        reviewers == Ok(Answer::Done)
            && f.get(&format!("{R}/pulls/{}", f.feature))?
                .root()
                .get("requested_reviewers")
                .array()
                .is_some_and(|rows| rows.len() == 1 && rows[0].get("id").u64() == Some(f.stranger.id)),
        &reviewers,
    ));
    for (name, op, state) in [
        ("component-write-close", Write::Close { number }, "closed"),
        ("component-write-reopen", Write::Reopen { number }, "open"),
    ] {
        let result = h.call(repo, op.operation());
        facts.push(super::fact(
            name,
            result == Ok(Answer::Done) && f.get(&route)?.root().get("state").is(state),
            &result,
        ));
    }
    effects(f, h, facts, number, own, &own_route)?;
    open_after_closed(f, h, facts)?;
    empty_pull(f, h, facts)?;
    let tally = super::fact("component-costs-equal-requests-sent", h.costs == h.sent, &(h.costs, h.sent));
    facts.insert(facts.len() - 2, tally);
    Ok(())
}
fn effects(f: &Fixture, h: &mut Hosted, facts: &mut Vec<Fact>, number: u64, own: u64, own_route: &str) -> Result<()> {
    let repo = repository(f)?;
    // A fresh pull is used for merging so update's asynchronous result cannot race it.
    f.branch(R, "component-merge", "landing")?;
    f.put(R, "component-merge", "merge.txt", "merged\n")?;
    let merge = f.pull(R, "component-merge", "landing")?;
    let merge_number = merge.root().get("number").u64().ok_or("merge number")?;
    f.ready(R, merge_number, None)?;
    let result = h.call(
        repo,
        Write::Merge {
            number: merge_number,
            head: commit(merge.root().get("head").get("sha").str().ok_or("merge head")?)?,
            how: api::MergeHow::Style,
        }
        .operation(),
    );
    let got = f.get(&format!("{R}/pulls/{merge_number}"))?;
    facts.push(super::fact("component-write-merge", matches!(result, Ok(Answer::Merged(head)) if got.root().get("merged").bool() == Some(true) && commit(got.root().get("merge_commit_sha").str().unwrap_or_default()).ok() == Some(head)), &result));
    f.ready(R, own, None)?;
    let before = f.get(own_route)?.root().get("head").get("sha").str().ok_or("update head")?.to_owned();
    let result = h.call(repo, Write::Update { number: own, style: api::UpdateStyle::Merge }.operation());
    if result == Ok(Answer::Done) {
        f.ready(R, own, Some(&before))?;
    }
    facts.push(super::fact(
        "component-write-update",
        result == Ok(Answer::Done) && f.get(own_route)?.root().get("head").get("sha").str() != Some(before.as_str()),
        &result,
    ));
    let head = commit(&f.head)?;
    let result = h.call(
        repo,
        Write::Status {
            commit: head,
            context: bytes("component/test"),
            check: api::Check::Passed,
            description: Box::new([]),
            link: Box::new([]),
        }
        .operation(),
    );
    let got = f.get(&format!("{R}/commits/{}/status?page=1&limit=50", f.head))?;
    facts.push(super::fact(
        "component-write-status",
        result == Ok(Answer::Done)
            && got.root().get("statuses").array().is_some_and(|rows| {
                rows.iter().any(|r| r.get("context").is("component/test") && r.get("status").is("success"))
            }),
        &result,
    ));
    let result = h.call(repo, Write::CreateBranch { branch: bytes("component-created"), commit: head }.operation());
    facts.push(super::fact(
        "component-write-create-branch",
        result == Ok(Answer::Branch(api::BranchCreation::Created))
            && f.get(&format!("{R}/branches/component-created"))?.root().get("commit").get("id").is(&f.head),
        &result,
    ));
    let result = h.call(repo, Write::DeleteBranch { branch: bytes("component-created") }.operation());
    let missing = f.exchange("GET", &format!("{R}/branches/component-created"), None, Some(&f.writer.token), None)?;
    facts.push(super::fact(
        "component-write-delete-branch",
        result == Ok(Answer::Done) && missing.status == 404,
        &result,
    ));
    let item = h.call(repo, Read::Item { number, after: 0 }.operation());
    let reviews = h.call(repo, Read::Reviews { number: f.feature, page: 1 }.operation());
    facts.push(super::fact("component-keys-found-on-read-back", matches!(&item, Ok(Answer::Item { item, comments, .. }) if item.key == Some(Box::new([1, 0, 0, 1])) && comments.iter().any(|c| c.key == Some(Box::new([1, 0, 0, 2])) && c.body.as_ref() == b"<!-- kit:key 01000002 -->\npost body")) && matches!(&reviews, Ok(Answer::Reviews { reviews, .. }) if reviews.iter().any(|r| r.key == Some(Box::new([1, 0, 0, 3])) && r.body.as_ref() == b"<!-- kit:key 01000003 -->\nkeyed review")), &(item, reviews)));
    Ok(())
}
fn open_after_closed(f: &Fixture, h: &mut Hosted, facts: &mut Vec<Fact>) -> Result<()> {
    f.branch(R, "component-repeat", "main")?;
    f.put(R, "component-repeat", "repeat.txt", "repeat\n")?;
    let first = f.pull(R, "component-repeat", "main")?.root().get("number").u64().ok_or("first pull")?;
    f.api("PATCH", &format!("{R}/issues/{first}"), &json::object(&[("state", &"closed")])?)?;
    let second = f.pull(R, "component-repeat", "main")?.root().get("number").u64().ok_or("second pull")?;
    facts.push(h.observe(
        "component-read-pull-for-open-after-closed",
        repository(f)?,
        Read::PullFor { head: bytes("component-repeat"), base: bytes("main") }.operation(),
        |a| matches!(a, Answer::Pull(p) if p.number == second && p.state == api::State::Open),
    ));
    Ok(())
}
fn empty_pull(f: &Fixture, h: &mut Hosted, facts: &mut Vec<Fact>) -> Result<()> {
    f.branch(R, "component-empty", "main")?;
    let result = h.call(
        repository(f)?,
        Write::OpenPull {
            title: bytes("empty"),
            body: bytes("empty"),
            head: bytes("component-empty"),
            base: bytes("main"),
        }
        .operation(),
    );
    let rows = f.get(&format!("{R}/pulls?state=all&page=1&limit=50"))?;
    facts.push(super::fact(
        "component-write-open-pull-no-changes",
        result == Err(api::Error::NothingToMerge)
            && rows.root().kind() == Some(skein_json::Kind::ArrayStart)
            && rows.root().items().iter().all(|r| r.get("head").get("ref").str() != Some("component-empty")),
        &result,
    ));
    Ok(())
}
