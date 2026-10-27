//! Connector additions checked against the groundwork's observed behavior.
#![expect(clippy::disallowed_methods, reason = "step tests are ordinary Rust (testing-strategy.md, section 4)")]
use super::*;

fn opened(h: &mut Harness, head: &[u8]) -> u64 {
    let Answer::Created(number) = h.open(head).unwrap() else { unreachable!("a created pull") };
    number
}

#[test]
fn clean_update_keeps_two_parents_reports_hooks_and_checks_the_new_head() {
    let mut configured = setup();
    configured.checks.cue = Some(Cue::File { path: copy_of(b"ci"), green: copy_of(b"green") });
    let mut h = Harness::with(CALM, configured);
    let work = h.commit(FIRST, &[(b"README", b"changed")]);
    h.push(ENGINE, b"work", work).unwrap();
    let number = opened(&mut h, b"work");
    h.settle();
    assert_eq!(h.pull(number).statuses[0].state, Check::Passed);
    let base = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"ci", b"red", MAINTAINER).unwrap();
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Merge })),
        Ok(Answer::Done)
    );
    let pull = h.pull(number);
    assert_ne!(pull.commit, work);
    assert_eq!(pull.statuses[0].state, Check::Pending);
    let object = h.domain.object(pull.commit).unwrap();
    assert_eq!(object.parent, Some(work));
    assert_eq!(object.merge_parent, Some(base));
    assert_eq!(&**object.tree.get(&b"README"[..]).unwrap(), b"changed");
    assert_eq!(&**object.tree.get(&b"ci"[..]).unwrap(), b"red");
    h.settle();
    assert_eq!(h.pull(number).statuses[0].state, Check::Failed, "CI belongs to the new head");
    assert!(h.hooks.iter().any(|hook| match hook {
        Request::Hook { change, commit, .. } => *change == Change::Push && *commit == Some(pull.commit),
        Request::Reply { .. } => false,
    }));
    assert!(h.hooks.iter().any(|hook| match hook {
        Request::Hook { change, number: heard, .. } => *change == Change::Pull && *heard == Some(number),
        Request::Reply { .. } => false,
    }));
    let remaining = h.domain.room().commits;
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Merge })),
        Ok(Answer::Done)
    );
    assert_eq!(h.domain.room().commits, remaining, "an already up-to-date update makes no extra commit");
    assert_eq!(h.branch(b"work"), Some(pull.commit));
}

#[test]
fn conflicting_update_changes_neither_head_nor_objects() {
    let mut h = Harness::new(CALM);
    let work = h.commit(FIRST, &[(b"README", b"ours")]);
    h.push(ENGINE, b"work", work).unwrap();
    let number = opened(&mut h, b"work");
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"README", b"theirs", MAINTAINER).unwrap();
    let remaining = h.domain.room().commits;
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Merge })),
        Err(Error::Conflict)
    );
    assert_eq!(h.branch(b"work"), Some(work));
    assert_eq!(h.pull(number).commit, work);
    assert_eq!(h.domain.room().commits, remaining);
    assert_eq!(
        h.call(PERSON, write(Write::Update { number, style: crate::api::UpdateStyle::Merge })),
        Err(Error::Forbidden)
    );
    h.call(ENGINE, write(Write::Close { number })).unwrap();
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Merge })),
        Err(Error::Closed)
    );
}

#[test]
fn pull_file_pages_are_distinct_and_name_the_exact_head() {
    let mut h = Harness::new(CALM);
    let work = crate::commit(&mut h.domain, &CALM, FIRST, files(&[(b"new", b"one")]), b"").unwrap().unwrap();
    h.push(ENGINE, b"work", work).unwrap();
    let number = opened(&mut h, b"work");
    let mut names = skein_lib::Set::with_capacity(3);
    for page in 1..=4_u32 {
        let Answer::PullFiles { head, files, more } = h.ok(PERSON, read(Read::PullFiles { number, page, limit: 1 }))
        else {
            unreachable!("a file page")
        };
        assert_eq!(head, work);
        if page == 4 {
            assert!(files.is_empty() && !more);
            continue;
        }
        assert_eq!(files.len(), 1);
        assert_eq!(more, page < 3);
        let file = files.first().unwrap();
        assert!(names.insert(file.path.clone()).unwrap());
        if *file.path == *b"new" {
            assert_eq!(file.before, None);
            assert_eq!(file.after.as_deref(), Some(&b"one"[..]));
        } else {
            assert!(file.before.is_some() && file.after.is_none(), "deleted files retain their before contents");
        }
    }
    assert_eq!(names.len(), 3);
}

#[test]
fn comparisons_ignore_paging_and_refuse_incomplete_data_explicitly() {
    let mut h = Harness::new(CALM);
    let one = h.commit(FIRST, &[(b"one", b"one")]);
    let two = h.commit(one, &[(b"two", b"two")]);
    h.push(ENGINE, b"work", two).unwrap();
    let first = h.ok(PERSON, read(Read::Compare { base: FIRST, head: two, page: 1, limit: 1 }));
    assert_eq!(first, h.ok(PERSON, read(Read::Compare { base: FIRST, head: two, page: 4, limit: 1 })));
    let Answer::Comparison { base, head, merge_base, contains_base: _, files: changed, commits } = first else {
        unreachable!("a comparison")
    };
    assert_eq!((base, head), (FIRST, two));
    assert_eq!(merge_base, Some(FIRST));
    assert_eq!(&*commits, &[one, two]);
    assert_eq!(changed.len(), 2);
    let excessive = crate::commit(&mut h.domain, &CALM, FIRST, files(&[(b"a", b"a"), (b"b", b"b"), (b"c", b"c")]), b"")
        .unwrap()
        .unwrap();
    h.push(ENGINE, b"other", excessive).unwrap();
    assert_eq!(
        h.call(PERSON, read(Read::Compare { base: FIRST, head: excessive, page: 1, limit: 1 })),
        Err(Error::TooLarge)
    );
    assert_eq!(
        h.call(PERSON, read(Read::Compare { base: FIRST, head: u64::MAX, page: 1, limit: 1 })),
        Err(Error::Missing(What::Commit))
    );
}

#[test]
fn adoption_metadata_and_protection_permission_match_the_probe() {
    let mut configured = setup();
    let protection = Protection {
        linear: true,
        code_owners: true,
        bypassing_users: Box::from([ENGINE]),
        queue: false,
        branch: copy_of(MAIN),
        contexts: names(&[b"ci"]),
        approvals: 1,
        dismiss_stale: true,
    };
    configured.protection = Some(protection.clone());
    let mut h = Harness::with(CALM, configured);
    assert_eq!(h.call(ENGINE, read(Read::Protection { branch: copy_of(MAIN) })), Err(Error::Forbidden));
    assert_eq!(h.call(ENGINE, read(Read::Protection { branch: copy_of(b"absent") })), Err(Error::Forbidden));
    assert_eq!(h.ok(ADMIN, read(Read::Protection { branch: copy_of(MAIN) })), Answer::Protection(Some(protection)));
    assert_eq!(h.ok(ADMIN, read(Read::Protection { branch: copy_of(b"absent") })), Answer::Protection(None));
    let metadata = crate::api::Settings { default: copy_of(MAIN), merge: false, rebase: true, squash: false };
    crate::settings(&mut h.domain, REPOSITORY, metadata.clone());
    assert_eq!(h.ok(PERSON, read(Read::Settings)), Answer::Settings(metadata));
    let Answer::Collaborators(users) = h.ok(PERSON, read(Read::Collaborators)) else { unreachable!("collaborators") };
    assert_eq!(users.len(), 5);
    assert!(users.iter().any(|user| user.user == ENGINE && user.permission == Permission::Write));
    assert!(users.iter().any(|user| user.user == ADMIN && user.permission == Permission::Admin));
    assert_eq!(
        h.call(PERSON, write(Write::CreateBranch { branch: copy_of(b"new"), commit: FIRST })),
        Err(Error::Forbidden)
    );
    assert_eq!(
        h.ok(ENGINE, write(Write::CreateBranch { branch: copy_of(b"new"), commit: FIRST })),
        Answer::Branch(Created::Created)
    );
    assert_eq!(h.branch(b"new"), Some(FIRST));
    assert_eq!(
        h.ok(ENGINE, write(Write::CreateBranch { branch: copy_of(b"new"), commit: FIRST })),
        Answer::Branch(Created::Exists)
    );
    assert_eq!(
        h.call(ENGINE, write(Write::CreateBranch { branch: copy_of(MAIN), commit: FIRST })),
        Err(Error::Protected)
    );
}

#[test]
fn a_manually_reported_failure_has_no_ci_job() {
    let mut h = Harness::new(CALM);
    h.call(ENGINE, status(FIRST, b"ci", Check::Failed)).unwrap();
    let Answer::Checks(checks) = h.ok(PERSON, read(Read::Checks { commit: FIRST })) else { unreachable!("checks") };
    assert_eq!(checks.len(), 1);
    let check = checks.first().unwrap();
    assert_eq!(check.status.state, Check::Failed);
    assert_eq!(&*check.description, b"");
    assert_eq!(&*check.link, b"");
    assert_eq!(check.job, None);
    assert_eq!(h.call(PERSON, read(Read::Log { job: 0, attempt: 1, step: None })), Err(Error::Missing(What::Job)));
}

#[test]
fn oversized_api_branch_creation_is_refused_before_a_delayed_write_can_hold_it() {
    let config = Config { landing: 1000, ..CALM };
    let mut h = Harness::new(config);
    let name = skein_lib::bytes::zeroed(usize::try_from(config.limits.name_bytes.checked_add(1).unwrap()).unwrap());
    assert_eq!(h.call(ENGINE, write(Write::CreateBranch { branch: name, commit: FIRST })), Err(Error::TooLarge));
    assert_eq!(h.domain.tally().landed, 0);
}

#[test]
fn api_branch_creation_does_not_depend_on_git_transport_reachability() {
    let mut h = Harness::new(CALM);
    crate::set_reachable(&mut h.domain, REPOSITORY, false);
    assert_eq!(h.call(ENGINE, Op::Git(Git::Clone)), Err(Error::Unreachable));
    assert_eq!(
        h.call(ENGINE, write(Write::CreateBranch { branch: copy_of(b"rest"), commit: FIRST })),
        Ok(Answer::Branch(Created::Created))
    );
    assert_eq!(h.branch(b"rest"), Some(FIRST));
}

#[test]
fn oversized_update_repository_is_not_retained_for_a_delayed_write() {
    let config = Config { landing: 1000, ..CALM };
    let mut h = Harness::new(config);
    let name = skein_lib::bytes::zeroed(usize::try_from(config.limits.name_bytes.checked_add(1).unwrap()).unwrap());
    assert_eq!(
        h.call_on(&name, ENGINE, write(Write::Update { number: 1, style: crate::api::UpdateStyle::Merge })),
        Err(Error::Missing(What::Repository))
    );
    assert_eq!(h.domain.tally().landed, 0);
}

#[test]
fn comparisons_carry_the_shared_base_of_divergent_and_unrelated_histories() {
    let mut h = Harness::new(CALM);
    let left = h.commit(FIRST, &[(b"one", b"left")]);
    let right = h.commit(FIRST, &[(b"two", b"right")]);
    h.push(ENGINE, b"left", left).unwrap();
    h.push(ENGINE, b"right", right).unwrap();
    let Answer::Comparison { merge_base, contains_base, .. } =
        h.ok(PERSON, read(Read::Compare { base: left, head: right, page: 1, limit: 1 }))
    else {
        unreachable!("comparison")
    };
    assert_eq!(merge_base, Some(FIRST));
    assert!(!contains_base);
    let mut other = setup();
    other.name = copy_of(b"acme/unrelated");
    other.id = 3;
    let unrelated = crate::repository(&mut h.domain, &CALM, other);
    h.push(ENGINE, b"unrelated", unrelated).unwrap();
    let Answer::Comparison { merge_base, contains_base, .. } =
        h.ok(PERSON, read(Read::Compare { base: left, head: unrelated, page: 1, limit: 1 }))
    else {
        unreachable!("comparison")
    };
    assert_eq!(merge_base, None);
    assert!(!contains_base);
}

#[test]
fn inspection_reads_protection_without_client_authorization_or_calls() {
    let mut configured = setup();
    let protection = Protection {
        linear: true,
        code_owners: true,
        bypassing_users: Box::from([ENGINE]),
        queue: false,
        branch: copy_of(MAIN),
        contexts: names(&[b"ci"]),
        approvals: 1,
        dismiss_stale: true,
    };
    configured.protection = Some(protection.clone());
    let h = Harness::with(CALM, configured);
    let tally = h.domain.tally();
    assert_eq!(h.domain.inspect_protection(REPOSITORY, MAIN), Some(&protection));
    assert_eq!(h.domain.inspect_protection(REPOSITORY, b"other"), None);
    assert_eq!(h.domain.inspect_protection(b"missing/repo", MAIN), None);
    assert_eq!(h.domain.tally(), tally);
    let unprotected = Harness::new(CALM);
    assert_eq!(unprotected.domain.inspect_protection(REPOSITORY, MAIN), None);
}

#[test]
fn a_fast_forward_merge_moves_the_base_to_the_head() {
    let mut h = Harness::new(CALM);
    let head = h.commit(FIRST, &[(b"README", b"new")]);
    h.push(ENGINE, b"work", head).unwrap();
    let number = opened(&mut h, b"work");
    let room = h.domain.room().commits;
    assert_eq!(
        h.call(ENGINE, write(Write::Merge { number, head, how: crate::api::MergeHow::FastForwardOnly })),
        Ok(Answer::Merged(head))
    );
    assert_eq!(h.branch(MAIN), Some(head));
    assert_eq!(h.pull(number).merged, Some(head));
    assert_eq!(h.pull(number).state, State::Closed);
    assert_eq!(h.domain.room().commits, room);
}
#[test]
fn a_fast_forward_merge_of_a_head_behind_its_base_is_a_conflict() {
    let mut h = Harness::new(CALM);
    let head = h.commit(FIRST, &[(b"README", b"work")]);
    h.push(ENGINE, b"work", head).unwrap();
    let number = opened(&mut h, b"work");
    let base = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"ci", b"base", MAINTAINER).unwrap();
    let room = h.domain.room().commits;
    assert_eq!(
        h.call(ENGINE, write(Write::Merge { number, head, how: crate::api::MergeHow::FastForwardOnly })),
        Err(Error::Conflict)
    );
    assert_eq!(h.branch(MAIN), Some(base));
    assert_eq!(h.pull(number).state, State::Open);
    assert_eq!(h.domain.room().commits, room);
}
#[test]
fn an_update_by_rebase_replays_the_branch_onto_the_base() {
    let mut h = Harness::new(CALM);
    let one =
        crate::commit(&mut h.domain, &CALM, FIRST, files(&[(b"README", b"one"), (b"ci", b"green")]), b"first patch")
            .unwrap()
            .unwrap();
    let two =
        crate::commit(&mut h.domain, &CALM, one, files(&[(b"README", b"two"), (b"ci", b"green")]), b"second patch")
            .unwrap()
            .unwrap();
    h.push(ENGINE, b"work", two).unwrap();
    let number = opened(&mut h, b"work");
    let base = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"ci", b"base", MAINTAINER).unwrap();
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Rebase })),
        Ok(Answer::Done)
    );
    let tip = h.branch(b"work").unwrap();
    let last = h.domain.object(tip).unwrap();
    assert_eq!(&*last.message, b"second patch");
    assert_eq!(&**last.tree.get(b"README".as_slice()).unwrap(), b"two");
    assert_eq!(&**last.tree.get(b"ci".as_slice()).unwrap(), b"base");
    let first = h.domain.object(last.parent.unwrap()).unwrap();
    assert_eq!(first.parent, Some(base));
    assert_eq!(&*first.message, b"first patch");
    assert_ne!(last.parent, Some(one));
    assert!(h.domain.has(REPOSITORY).contains(&last.parent.unwrap()));
    let pull = h.pull(number);
    assert_eq!(pull.commit, tip);
    assert_eq!(pull.statuses[0].state, Check::Pending);
}
#[test]
fn an_update_by_rebase_refuses_a_conflict() {
    let mut h = Harness::new(CALM);
    let one = h.commit(FIRST, &[(b"ci", b"first")]);
    let two = h.commit(one, &[(b"README", b"work")]);
    h.push(ENGINE, b"work", two).unwrap();
    let number = opened(&mut h, b"work");
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"README", b"base", MAINTAINER).unwrap();
    let room = h.domain.room().commits;
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Rebase })),
        Err(Error::Conflict)
    );
    assert_eq!(h.branch(b"work"), Some(two));
    assert_eq!(h.pull(number).commit, two);
    assert_eq!(h.domain.room().commits, room, "a later conflict stores no earlier replay");
}
#[test]
fn an_update_by_rebase_of_a_branch_on_the_tip_changes_nothing() {
    let mut h = Harness::new(CALM);
    let head = h.commit(FIRST, &[(b"README", b"work")]);
    h.push(ENGINE, b"work", head).unwrap();
    let number = opened(&mut h, b"work");
    let room = h.domain.room().commits;
    assert_eq!(
        h.call(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Rebase })),
        Ok(Answer::Done)
    );
    assert_eq!(h.branch(b"work"), Some(head));
    assert_eq!(h.domain.room().commits, room);
}

fn judge_changes(h: &mut Harness, decision: Option<(u64, u64)>) -> crate::promises::Verdict {
    use crate::promises::{Bounds, Decision, Judge, Profile, Recorded};
    let mut judge = Judge::new(Profile {
        client: ENGINE,
        marker: (copy_of(b"<key>"), copy_of(b"</key>")),
        owned_branches: Box::new([]),
        allowed: Box::new([]),
        bounds: Bounds { objects: 32, decisions: 8 },
    });
    if let Some((number, head)) = decision {
        judge.decide(&Decision { repository: copy_of(REPOSITORY), number, head });
    }
    while let Some(observation) = h.domain.pop_observation() {
        let forward = if let Observation::Moved { from: Some(from), to, .. } = &observation {
            Some(h.domain.is_ancestor(*from, *to))
        } else {
            None
        };
        judge.observe(&Recorded::Observed { observation, forward });
    }
    judge.observe(&Recorded::Lost(h.domain.observations_lost()));
    judge.decided();
    judge.verdict()
}

#[test]
fn a_fast_forward_merge_is_judged_at_the_decided_head_and_its_base_move_is_exempt() {
    let mut h = Harness::new(CALM);
    let head = h.commit(FIRST, &[(b"README", b"work")]);
    h.push(ENGINE, b"work", head).unwrap();
    let number = opened(&mut h, b"work");
    h.ok(ENGINE, write(Write::Merge { number, head, how: crate::api::MergeHow::FastForwardOnly }));
    assert_eq!(judge_changes(&mut h, Some((number, head))), crate::promises::Verdict::Pass);
}

#[test]
fn a_rebased_branch_move_is_seen_by_the_forced_promise() {
    let mut h = Harness::new(CALM);
    let head = h.commit(FIRST, &[(b"README", b"work")]);
    h.push(ENGINE, b"work", head).unwrap();
    let number = opened(&mut h, b"work");
    crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"ci", b"base", MAINTAINER).unwrap();
    h.ok(ENGINE, write(Write::Update { number, style: crate::api::UpdateStyle::Rebase }));
    assert_eq!(
        judge_changes(&mut h, None),
        crate::promises::Verdict::Fail(Box::new([crate::promises::Breach {
            promise: crate::promises::Promise::Forced,
            subject: copy_of(b"ai%2Fkit/work")
        }]))
    );
}
