use super::{fixtures as f, read_support as r, write_support as w};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api,
    github::{plans::tables::Method, request::Request},
};

fn settings(method: Method) -> a::Settings {
    a::Settings {
        default: f::boxed(b"trunk"),
        merge: method == Method::Merge,
        squash: method == Method::Squash,
        rebase: method == Method::Rebase,
    }
}
fn request(number: u64, head: u64, method: Method) -> Request {
    Request::Merge { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), number, head: r::object(head), method }
}
fn write(number: u64, head: u64) -> api::Write {
    api::Write::Merge { number, head: r::object(head) }
}
fn protected(approvals: u32, contexts: &[&[u8]]) -> f::Harness {
    let mut config = r::domain_config();
    config.limits.contexts = 4;
    config.limits.statuses = 4;
    let mut names = skein_lib::List::with_capacity(4);
    for name in contexts {
        names.push(f::boxed(name)).unwrap();
    }
    let protection = a::Protection {
        linear: false,
        code_owners: false,
        bypassing_users: Box::from([]),
        queue: false,
        branch: f::boxed(b"trunk"),
        contexts: names.into_boxed(),
        approvals,
        dismiss_stale: true,
    };
    let mut h = f::Harness::with_protection(f::limits(), f::config(), config, Some(protection));
    fake::grant(&mut h.domain, b"acme/widgets", f::BOT, a::Permission::Write);
    h
}
#[test]
fn github_fake_merge_plans_land_each_allowed_method_and_read_back() {
    for method in [Method::Merge, Method::Squash, Method::Rebase] {
        let mut h = w::harness();
        fake::settings(&mut h.domain, b"acme/widgets", settings(method));
        let (number, head) = r::pull(&mut h);
        let token = r::token(&mut h);
        let api::Answer::Merged(landed) = w::write(&mut h, &token, write(number, head)).unwrap() else {
            panic!("merged")
        };
        let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
            panic!("merged pull")
        };
        assert_eq!(pull.state, api::State::Closed);
        assert_eq!(pull.merged, Some(landed));
        let api::Answer::Commit(commit) =
            r::read(&mut h, &token, api::Read::Branch { branch: f::boxed(b"trunk") }).unwrap()
        else {
            panic!("base moved")
        };
        assert_eq!(commit, landed);
        let api::Answer::File { bytes, .. } =
            r::read(&mut h, &token, api::Read::File { head: landed, path: f::boxed(b"one"), max_bytes: 128 }).unwrap()
        else {
            panic!("merged tree")
        };
        assert_eq!(bytes.as_ref(), b"one contents");
    }
}
#[test]
fn github_fake_merge_moved_head_is_409_and_plan_stale() {
    let mut h = w::harness();
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    assert_eq!(w::write(&mut h, &token, write(number, head.checked_sub(1).unwrap())), Err(api::Error::Stale));
    let response = w::ask(&mut h, &token, &request(number, head.checked_sub(1).unwrap(), Method::Squash));
    f::refusal(&h, &response, 409, crate::texts::MERGE_STALE);
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("stale readback")
    };
    assert_eq!(pull.commit, r::object(head));
    assert_eq!(pull.state, api::State::Open);
}
#[test]
fn github_fake_merge_conflict_is_405_and_preserves_head() {
    let mut h = w::harness();
    let (number, head) = r::pull(&mut h);
    let base = fake::commit(
        &mut h.domain,
        &h.env.limits,
        h.first,
        Box::from([a::File { path: f::boxed(b"one"), content: f::boxed(b"conflict") }]),
        b"conflict",
    )
    .unwrap()
    .unwrap();
    r::call(&mut h, a::Op::Git(a::Git::Push { branch: f::boxed(b"trunk"), commit: base, expected: None }));
    let token = r::token(&mut h);
    assert_eq!(w::write(&mut h, &token, write(number, head)), Err(api::Error::Conflict));
    let response = w::ask(&mut h, &token, &request(number, head, Method::Squash));
    f::refusal(&h, &response, 405, crate::texts::NOT_MERGEABLE);
    let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("conflict readback")
    };
    assert_eq!(pull.commit, r::object(head));
    assert_eq!(pull.state, api::State::Open);
}
#[test]
fn github_fake_merge_method_refusal_uses_the_requested_method() {
    for (method, prefix) in
        [(Method::Merge, b"Merge ".as_slice()), (Method::Squash, b"Squash "), (Method::Rebase, b"Rebase ")]
    {
        let mut h = w::harness();
        fake::settings(&mut h.domain, b"acme/widgets", settings(method));
        let (number, head) = r::pull(&mut h);
        let token = r::token(&mut h);
        let disabled = a::Settings { default: f::boxed(b"trunk"), merge: false, squash: false, rebase: false };
        assert_eq!(w::write_with_settings(&mut h, &token, write(number, head), disabled), Err(api::Error::Refused));
        let response = w::ask(&mut h, &token, &request(number, head, method));
        let message = f::parts(&[prefix, crate::texts::MERGE_METHOD]);
        f::refusal(&h, &response, 405, &message);
    }
}
#[test]
fn github_fake_merge_approval_rule_has_recorded_shape_and_configured_count() {
    for (count, message) in [
        (1, b"Repository rule violations found\n\nAt least 1 approving review is required by reviewers with write access.\n\n".as_slice()),
        (2, b"Repository rule violations found\n\nAt least 2 approving reviews are required by reviewers with write access.\n\n"),
        (3, b"Repository rule violations found\n\nAt least 3 approving reviews are required by reviewers with write access.\n\n"),
    ] {
        let mut h = protected(count, &[]);
        let (number, head) = r::pull(&mut h);
        let token = r::token(&mut h);
        assert_eq!(w::write(&mut h, &token, write(number, head)), Err(api::Error::Protected));
        let response = w::ask(&mut h, &token, &request(number, head, Method::Squash));
        f::refusal(&h, &response, 405, message);
        assert!(
            skein_lib::bytes::find(&response.body, b"https://docs.github.com/rest/pulls/pulls#merge-a-pull-request")
                .is_some()
        );
        assert!(skein_lib::bytes::find(&response.body, b"\"status\":\"405\"").is_some());
        r::call_as(
            &mut h,
            f::BOT,
            a::Op::Write(a::Write::Review { number, verdict: Some(a::Verdict::Approve), body: f::boxed(b"approved") }),
        );
        if count == 1 {
            let api::Answer::Merged(_) = w::write(&mut h, &token, write(number, head)).unwrap() else {
                panic!("approved merge")
            };
        }
    }
}
#[test]
fn github_fake_merge_check_rules_name_each_unmet_context_and_state() {
    let mut h = protected(0, &[b"missing", b"running", b"failed", b"passed"]);
    let (number, head) = r::pull(&mut h);
    for (context, state) in
        [(b"running".as_slice(), a::Check::Pending), (b"failed", a::Check::Failed), (b"passed", a::Check::Passed)]
    {
        r::call(&mut h, a::Op::Write(a::Write::Status { commit: head, context: f::boxed(context), state }));
    }
    let token = r::token(&mut h);
    assert_eq!(w::write(&mut h, &token, write(number, head)), Err(api::Error::Protected));
    let response = w::ask(&mut h, &token, &request(number, head, Method::Squash));
    let message = f::parts(&[
        crate::texts::RULES,
        b"\n\n",
        crate::texts::CHECK_START,
        b"missing",
        crate::texts::CHECK_EXPECTED,
        b"\n\n",
        crate::texts::CHECK_START,
        b"running",
        crate::texts::CHECK_PROGRESS,
        b"\n\n",
        crate::texts::CHECK_START,
        b"failed",
        crate::texts::CHECK_FAILING,
        b"\n\n",
    ]);
    f::refusal(&h, &response, 405, &message);
}
#[test]
fn github_fake_merge_outside_installation_is_missing_before_domain_calls() {
    let mut h = w::harness();
    let token = r::token(&mut h);
    let response = w::ask(
        &mut h,
        &token,
        &Request::Merge {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"absent"),
            number: 1,
            head: r::object(1),
            method: Method::Squash,
        },
    );
    f::refusal(&h, &response, 404, b"Not Found");
    assert_eq!(h.calls, 0);
}

#[test]
fn github_fake_merge_waits_for_head_or_base_settle_and_stale_sha_keeps_precedence() {
    for branch in [b"feature".as_slice(), b"trunk"] {
        let mut h = w::harness();
        let (number, head) = r::pull(&mut h);
        let token = r::token(&mut h);
        h.drain_timers = false;
        let parent = if branch == b"feature" { head } else { h.first };
        let next = fake::commit(
            &mut h.domain,
            &h.env.limits,
            parent,
            Box::from([a::File { path: f::boxed(b"separate"), content: f::boxed(b"move") }]),
            b"move",
        )
        .unwrap()
        .unwrap();
        r::call(&mut h, a::Op::Git(a::Git::Push { branch: f::boxed(branch), commit: next, expected: None }));
        let current = if branch == b"feature" { next } else { head };
        let response = w::ask(&mut h, &token, &request(number, current, Method::Squash));
        f::refusal(&h, &response, 405, crate::texts::NOT_MERGEABLE);
        let stale = h.first;
        let response = w::ask(&mut h, &token, &request(number, stale, Method::Squash));
        f::refusal(&h, &response, 409, crate::texts::MERGE_STALE);
        h.env.now = h.env.now.saturating_add(h.env.limits.settle_max);
        h.now = crate::tests::wall(1202);
        let response = w::ask(&mut h, &token, &request(number, current, Method::Squash));
        assert_eq!(response.status, 200);
        let api::Answer::Pull(pull) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
            panic!("merged readback")
        };
        assert_eq!(pull.state, api::State::Closed);
        assert!(pull.merged.is_some());
    }
}

#[test]
fn github_fake_merged_pull_accepts_repeat_merges_with_any_sha() {
    let mut h = w::harness();
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let first = w::ask(&mut h, &token, &request(number, head, Method::Squash));
    assert_eq!(first.status, 200);
    for sha in [head, 0, h.first] {
        let repeat = w::ask(&mut h, &token, &request(number, sha, Method::Squash));
        assert_eq!(repeat.status, 200);
        assert_eq!(repeat.body, first.body);
    }
}
