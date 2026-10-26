use super::{fixtures as f, read_support as r, write_support as w};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api,
    github::request::{Method, Request},
};

fn open() -> api::Write {
    api::Write::OpenPull {
        title: f::boxed(b"feature"),
        body: f::boxed(b"pull body"),
        head: f::boxed(b"feature"),
        base: f::boxed(b"trunk"),
    }
}
fn feature(h: &mut f::Harness) -> u64 {
    let commit = fake::commit(
        &mut h.domain,
        &h.env.limits,
        h.first,
        Box::from([a::File { path: f::boxed(b"one"), content: f::boxed(b"feature") }]),
        b"feature",
    )
    .unwrap()
    .unwrap();
    r::call(h, a::Op::Git(a::Git::Push { branch: f::boxed(b"feature"), commit, expected: None }));
    commit
}
#[test]
fn github_fake_open_pull_plan_creates_and_reads_back() {
    let mut h = w::harness();
    let head = feature(&mut h);
    let token = r::token(&mut h);
    let api::Answer::Created(number) = w::write(&mut h, &token, open()).unwrap() else { panic!("created pull") };
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("pull readback")
    };
    assert_eq!(pull.commit, r::object(head));
    assert_eq!(pull.head.as_ref(), b"feature");
    assert_eq!(pull.base.as_ref(), b"trunk");
    assert_eq!(w::write(&mut h, &token, open()), Err(api::Error::Exists));
    let response = h.ask(
        Method::Post,
        b"/repos/acme/widgets/pulls",
        Some(token.expose()),
        br#"{"title":"feature","body":"body","head":"feature","base":"trunk"}"#,
    );
    let message = f::parts(&[crate::texts::PULL_EXISTS, b"acme:feature."]);
    f::refusal(&h, &response, 422, b"Validation Failed");
    let (_, skein_forge::github::document::Document::Error(error)) =
        h.decode(skein_forge::github::document::Kind::Error, &response)
    else {
        panic!("pull validation")
    };
    assert_eq!(error.errors.as_ref(), &[message]);
    assert!(skein_lib::bytes::find(&response.body, br#""resource":"PullRequest","code":"custom""#).is_some());
}
#[test]
fn github_fake_empty_head_refuses_open_pull_without_creating() {
    let mut h = w::harness();
    h.branch(b"feature");
    let token = r::token(&mut h);
    assert_eq!(w::write(&mut h, &token, open()), Err(api::Error::NothingToMerge));
    let response = h.ask(
        Method::Post,
        b"/repos/acme/widgets/pulls",
        Some(token.expose()),
        br#"{"title":"feature","body":"body","head":"feature","base":"trunk"}"#,
    );
    let message = f::parts(&[crate::texts::NO_COMMITS, b"trunk and feature"]);
    f::refusal(&h, &response, 422, b"Validation Failed");
    let (_, skein_forge::github::document::Document::Error(error)) =
        h.decode(skein_forge::github::document::Kind::Error, &response)
    else {
        panic!("pull validation")
    };
    assert_eq!(error.errors.as_ref(), &[message]);
    assert!(skein_lib::bytes::find(&response.body, br#""resource":"PullRequest","code":"custom""#).is_some());
    assert_eq!(r::read(&mut h, &token, api::Read::Pull { number: 1 }), Err(api::Error::Missing));
}
#[test]
fn github_fake_update_plan_merges_base_and_checks_expected_head() {
    let mut h = w::harness();
    fake::settings(
        &mut h.domain,
        b"acme/widgets",
        a::Settings { default: f::boxed(b"trunk"), merge: true, squash: true, rebase: true },
    );
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let base = fake::commit(
        &mut h.domain,
        &h.env.limits,
        h.first,
        Box::from([a::File { path: f::boxed(b"base"), content: f::boxed(b"base") }]),
        b"base",
    )
    .unwrap()
    .unwrap();
    r::call(&mut h, a::Op::Git(a::Git::Push { branch: f::boxed(b"trunk"), commit: base, expected: None }));
    assert_eq!(w::write(&mut h, &token, api::Write::Update { number }), Ok(api::Answer::Done));
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("updated pull")
    };
    assert_ne!(pull.commit, r::object(head));
    assert_eq!(pull.base_commit, Some(r::object(base)));
    let api::Answer::File { bytes, .. } =
        r::read(&mut h, &token, api::Read::File { head: pull.commit, path: f::boxed(b"base"), max_bytes: 128 })
            .unwrap()
    else {
        panic!("updated tree")
    };
    assert_eq!(bytes.as_ref(), b"base");
    let request =
        Request::Update { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), number, head: r::object(head) };
    let response = w::ask(&mut h, &token, &request);
    f::refusal(&h, &response, 422, crate::texts::UPDATE_STALE);
    let api::Answer::Pull(after) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("stale update readback")
    };
    assert_eq!(after.commit, pull.commit);
}
#[test]
fn github_fake_update_plan_refuses_head_moved_after_its_read() {
    let mut h = w::harness();
    let (number, head) = r::pull(&mut h);
    let moved = fake::commit(
        &mut h.domain,
        &h.env.limits,
        head,
        Box::from([a::File { path: f::boxed(b"moved"), content: f::boxed(b"moved") }]),
        b"moved",
    )
    .unwrap()
    .unwrap();
    let token = r::token(&mut h);
    assert_eq!(w::write_after_push(&mut h, &token, api::Write::Update { number }, Some(moved)), Err(api::Error::Stale));
}
#[test]
fn github_fake_review_and_reviewer_plans_read_back() {
    let mut h = w::harness();
    let (number, _) = r::pull(&mut h);
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Write);
    let token = r::token(&mut h);
    assert_eq!(
        w::write(&mut h, &token, api::Write::SetReviewers { number, reviewers: Box::from([31]) }),
        Ok(api::Answer::Done)
    );
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("reviewers")
    };
    assert_eq!(pull.reviewers.as_ref(), &[31]);
    assert_eq!(
        w::write(&mut h, &token, api::Write::SetReviewers { number, reviewers: Box::from([]) }),
        Ok(api::Answer::Done)
    );
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("reviewers removed")
    };
    assert_eq!(pull.reviewers.as_ref(), &[]);
    let api::Answer::Reviewed(id) = w::write(
        &mut h,
        &token,
        api::Write::Review {
            number,
            key: f::boxed(b"review-key"),
            verdict: api::Verdict::Approve,
            body: f::boxed(b"approved"),
        },
    )
    .unwrap() else {
        panic!("reviewed")
    };
    let api::Answer::Reviews { reviews, .. } = r::read(&mut h, &token, api::Read::Reviews { number, page: 1 }).unwrap()
    else {
        panic!("review readback")
    };
    let review = reviews.first().unwrap();
    assert_eq!(review.id, id);
    assert_eq!(review.verdict, api::Verdict::Approve);
    assert_eq!(review.key.as_deref(), Some(b"review-key".as_slice()));
}
#[test]
fn github_fake_review_refuses_approving_own_pull() {
    let mut h = w::harness();
    feature(&mut h);
    let token = r::token(&mut h);
    let api::Answer::Created(number) = w::write(&mut h, &token, open()).unwrap() else { panic!("created") };
    assert_eq!(
        w::write(
            &mut h,
            &token,
            api::Write::Review {
                number,
                key: f::boxed(b"own"),
                verdict: api::Verdict::Approve,
                body: f::boxed(b"approved")
            }
        ),
        Err(api::Error::Refused)
    );
    let response = w::ask(
        &mut h,
        &token,
        &Request::Review {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"widgets"),
            number,
            verdict: api::Verdict::Approve,
            body: f::boxed(b"approve"),
        },
    );
    f::refusal(&h, &response, 422, b"Unprocessable Entity");
    assert!(skein_lib::bytes::find(&response.body, crate::texts::OWN_REVIEW_ERROR).is_some());
    assert!(skein_lib::bytes::find(&response.body, br#""errors":[""#).is_some());
}
#[test]
fn github_fake_pull_writes_outside_installation_are_missing() {
    let mut h = w::harness();
    let token = r::token(&mut h);
    for request in [
        Request::OpenPull {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"absent"),
            title: Box::from([]),
            body: Box::from([]),
            head: Box::from([]),
            base: Box::from([]),
        },
        Request::Update { owner: f::boxed(b"acme"), name: f::boxed(b"absent"), number: 1, head: r::object(1) },
        Request::Review {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"absent"),
            number: 1,
            verdict: api::Verdict::Comment,
            body: Box::from([]),
        },
        Request::Reviewers {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"absent"),
            number: 1,
            add: true,
            logins: Box::from([]),
        },
        Request::Reviewers {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"absent"),
            number: 1,
            add: false,
            logins: Box::from([]),
        },
    ] {
        let response = w::ask(&mut h, &token, &request);
        f::refusal(&h, &response, 404, b"Not Found");
    }
    assert_eq!(h.calls, 0);
}

#[test]
fn github_fake_update_conflict_preserves_the_pull_head() {
    let mut h = w::harness();
    fake::settings(
        &mut h.domain,
        b"acme/widgets",
        a::Settings { default: f::boxed(b"trunk"), merge: true, squash: true, rebase: true },
    );
    let (number, head) = r::pull(&mut h);
    let base = fake::commit(
        &mut h.domain,
        &h.env.limits,
        h.first,
        Box::from([a::File { path: f::boxed(b"one"), content: f::boxed(b"conflicting") }]),
        b"conflict",
    )
    .unwrap()
    .unwrap();
    r::call(&mut h, a::Op::Git(a::Git::Push { branch: f::boxed(b"trunk"), commit: base, expected: None }));
    let token = r::token(&mut h);
    assert_eq!(w::write(&mut h, &token, api::Write::Update { number }), Err(api::Error::Conflict));
    let response = w::ask(
        &mut h,
        &token,
        &Request::Update { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), number, head: r::object(head) },
    );
    f::refusal(&h, &response, 422, crate::texts::UPDATE_CONFLICT);
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("conflicted pull")
    };
    assert_eq!(pull.commit, r::object(head));
}
