pub mod fixtures;
use fixtures::issue;
use skein_forge::api;
use skein_forge_client::{Condition, Effect, Entry, Made, Outcome, Resource, Watch, What};
use skein_forge_client_world::{REPO, Settings, World};
fn post(entry: u64, number: u64) -> Entry {
    Entry {
        number: entry,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::Post {
                number,
                key: Box::new([1, 0, 3, b'k', b'i', b't', u8::try_from(entry).expect("test entry bound")]),
                body: Box::from(&b"milestone"[..]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    }
}
fn run(settings: Settings) -> skein_forge_client_world::Stats {
    let mut world = World::new(settings);
    world.make(issue(1));
    world.run_for(30);
    let mut number = None;
    for (_, outcome) in world.outcomes() {
        match outcome {
            Outcome::Made { made: Made::Created(n), .. } => number = Some(*n),
            Outcome::Made { .. }
            | Outcome::Failed(_)
            | Outcome::Raced { .. }
            | Outcome::Uncertain
            | Outcome::Queued
            | Outcome::Held
            | Outcome::Withdrawn => {}
        }
    }
    if let Some(number) = number {
        world.keep(Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Issue(number) },
            participating: true,
        }]));
        for entry in 2..=4 {
            world.make(post(entry, number));
        }
        world.run_for(1);
        world.restart();
        world.run_for(30);
    }
    world.finish();
    world.stats()
}
#[test]
fn creations_and_comments_survive_faults_late_landings_and_restart() {
    for seed in 0..4 {
        let stats = run(Settings::random(seed));
        assert!(stats.creations > 0);
        assert_eq!(stats.calls, stats.terminals);
    }
}

#[test]
fn an_uncertain_open_pull_is_found_without_retry() {
    let mut world = World::new(Settings::calm(46));
    world.historical_topic();
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::OpenPull {
                title: Box::from(&b"task"[..]),
                body: Box::new([]),
                head: Box::from(&b"topic"[..]),
                base: Box::from(&b"main"[..]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    });
    world.run_for(1);
    assert_eq!(world.stats().writes, 1);
    world.calm();
    world.restart();
    world.run_for(13);
    assert_eq!(world.stats().writes, 1);
    let ask = api::Read::PullFor { head: Box::from(&b"topic"[..]), base: Box::from(&b"main"[..]) };
    world.read(9, ask.clone());
    world.run_for(1);
    let number = match world.read_result(9) {
        Some(Ok(api::Answer::Pull(pull))) => {
            assert_eq!(pull.state, api::State::Open);
            pull.number
        }
        other => panic!("late opening was found: {other:?}"),
    };
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    world.read(10, ask);
    world.run_for(1);
    match world.read_result(10) {
        Some(Ok(api::Answer::Pull(pull))) => assert_eq!(pull.number, number),
        other => panic!("same pull remains open: {other:?}"),
    }
    assert!(
        world
            .outcomes()
            .iter()
            .any(|(entry, outcome)| { *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Created(_), .. }) })
    );
    world.finish();
}

#[test]
fn a_late_opening_closed_by_another_hand_is_reported_as_drift() {
    let mut world = World::new(Settings::calm(49));
    world.historical_topic();
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::OpenPull {
                title: Box::from(&b"task"[..]),
                body: Box::new([]),
                head: Box::from(&b"topic"[..]),
                base: Box::from(&b"main"[..]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    });
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(6);
    let ask = api::Read::PullFor { head: Box::from(&b"topic"[..]), base: Box::from(&b"main"[..]) };
    world.read(11, ask);
    world.run_for(1);
    let first = match world.read_result(11) {
        Some(Ok(api::Answer::Pull(pull))) => pull.number,
        other => panic!("late opening landed within its lifetime: {other:?}"),
    };
    world.outside_close_pull(first);
    world.run_for(6);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.stats().writes, 1);
    assert!(world.outcomes().contains(&(1, Outcome::Raced { made: Made::Created(first), why: api::Error::Closed })));
    world.finish();
}

#[test]
fn an_unrecoverable_creation_is_found_if_it_lands_within_its_lifetime() {
    let mut world = World::new(Settings::calm(42));
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(issue(1));
    world.run_for(1);
    assert_eq!(world.stats().writes, 1);
    world.calm();
    world.restart();
    world.run_for(12);
    assert!(world.outcomes().iter().any(|(entry, outcome)| {
        *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Created(_), found: true })
    }));
    assert_eq!(world.stats().writes, 1, "an unrecoverable creation is never retried");
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.stats().creations, 1);
    world.finish();
}

#[test]
fn an_uncertain_comment_found_by_key_is_made_without_retry() {
    let mut world = World::new(Settings::calm(47));
    let issue = world.historical_issue();
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(post(1, issue));
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(13);
    assert_eq!(world.stats().writes, 1);
    assert!(
        world.outcomes().iter().any(|(entry, outcome)| {
            *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Commented(_), found: true })
        }),
        "an observed marker settles the uncertain comment: {:?}",
        world.outcomes()
    );
    world.finish();
}

#[test]
fn an_uncertain_comment_missing_after_its_deadline_holds_the_task() {
    let mut world = World::new(Settings::calm(48));
    let issue = world.historical_issue().saturating_add(100);
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(post(1, issue));
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(12);
    assert!(world.outcomes().contains(&(1, Outcome::Held)));
    assert_eq!(world.stats().writes, 1);
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    world.finish();
}

#[test]
fn a_conditional_merge_lands_once_within_its_attempt_lifetime() {
    let mut world = World::new(Settings::calm(43));
    let pull = world.historical_pull();
    let head = world.branch(b"topic").expect("fixture pull head");
    let landing = world.branch(b"main").expect("fixture landing branch");
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::Merge {
                number: pull,
                head: skein_forge_client_world::translate::commit(head),
                how: skein_forge::api::MergeHow::Style,
            },
            condition: Condition::Merge { base: Box::from(&b"main"[..]) },
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    });
    world.run_for(1);
    assert_eq!(world.stats().writes, 1);
    world.calm();
    world.restart();
    world.run_for(8);
    assert_eq!(world.stats().writes, 1);
    world.run_for(5);
    assert_eq!(world.stats().writes, 1);
    let merged = world.branch(b"main").expect("merged landing branch");
    assert_ne!(merged, landing);
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.branch(b"main"), Some(merged), "the stale copy cannot merge a second time");
    assert!(
        world
            .outcomes()
            .iter()
            .any(|(entry, outcome)| { *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Merged(_), .. }) })
    );
    world.finish();
}

#[test]
fn a_late_set_reaches_its_final_state() {
    let mut world = World::new(Settings::calm(44));
    let head = skein_forge_client_world::translate::commit(world.branch(b"main").expect("fixture main"));
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::Status {
                commit: head,
                context: Box::from(&b"build"[..]),
                check: api::Check::Passed,
                description: Box::new([]),
                link: Box::new([]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    });
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(8);
    assert_eq!(world.stats().writes, 1);
    world.run_for(15);
    assert!(world.stats().writes <= 2);
    assert_eq!(world.stats().late_landings, 1);
    world.read(9, api::Read::Statuses { commit: head, page: 1 });
    world.run_for(1);
    match world.read_result(9) {
        Some(Ok(api::Answer::Statuses { statuses, .. })) => {
            assert_eq!(statuses.len(), 1);
            assert_eq!(statuses[0].context.as_ref(), b"build");
            assert_eq!(statuses[0].check, api::Check::Passed);
        }
        other => panic!("set status visible after the delayed write: {other:?}"),
    }
    world.finish();
}

#[test]
fn another_writer_taking_a_branch_name_is_reported_without_overwriting_it() {
    let mut world = World::new(Settings::calm(45));
    world.historical_pull();
    let intended = world.branch(b"main").expect("fixture main");
    let other = world.branch(b"topic").expect("fixture topic");
    assert_ne!(intended, other);
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::CreateBranch {
                branch: Box::from(&b"kit/7"[..]),
                commit: skein_forge_client_world::translate::commit(intended),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    });
    world.run_for(1);
    world.calm();
    world.outside_branch(b"kit/7", other);
    world.restart();
    world.run_for(2);
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.branch(b"kit/7"), Some(other));
    assert!(world.outcomes().contains(&(1, Outcome::Failed(api::Error::Exists))));
    world.run_for(20);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.branch(b"kit/7"), Some(other));
    world.finish();
}
#[test]
fn a_seed_replays_and_facts_change_no_decision() {
    let first = run(Settings::random(7));
    let second = run(Settings::random(7));
    assert_eq!(first, second);
    let no_facts = run(Settings { facts: 0, ..Settings::random(7) });
    assert_eq!(first, no_facts);
}
#[test]
fn idle_cost_is_independent_of_ten_times_the_history() {
    let mut empty = World::new(Settings::calm(1));
    let mut old = World::new(Settings { history: 20, ..Settings::calm(1) });
    let watch = Watch { resource: Resource { repository: REPO, what: What::Repository }, participating: false };
    empty.keep(Box::new([watch.clone()]));
    old.keep(Box::new([watch]));
    empty.run_for(20);
    old.run_for(20);
    assert_eq!(empty.stats().calls, old.stats().calls);
    assert_eq!(empty.stats().reads, old.stats().reads);
    empty.finish();
    old.finish();
}
#[test]
fn markers_have_the_protocol_spelling_and_arbitrary_bytes_round_trip() {
    let body = skein_forge_client_world::translate::keyed(&[0, 255], b"hello");
    assert_eq!(&*body, b"<!-- kit:key 00ff -->\nhello");
    assert_eq!(skein_forge_client_world::translate::key_of(&body), Some(Box::from(&[0, 255][..])));
}
#[test]
fn every_live_fresh_read_finishes_once_under_faults_and_rate_resets() {
    let mut world = World::new(Settings::random(31));
    for number in 1..=3 {
        world.read(number, api::Read::Branch { branch: Box::from(&b"main"[..]) });
    }
    world.run_for(20);
    world.finish();
    assert_eq!(world.stats().read_terminals, 3);
    assert_eq!(world.stats().calls, world.stats().terminals);
}
#[test]
fn the_groundwork_reads_keep_the_fake_api_facts_in_the_client_projection() {
    let mut world = World::new(Settings::calm(5));
    world.read(
        1,
        api::Read::Job {
            attempt: api::JobAttempt {
                head: skein_forge_client_world::translate::commit(1),
                run: 1,
                job: 1,
                attempt: 1,
            },
            max_bytes: 64,
        },
    );
    world.read(2, api::Read::Protection { branch: Box::from(&b"main"[..]) });
    world.read(3, api::Read::Settings);
    world.read(4, api::Read::Permission { user: 2 });
    world.run_for(1);
    assert_eq!(world.read_result(1), Some(&Err(api::Error::MissingJob)));
    assert_eq!(world.read_result(2), Some(&Ok(api::Answer::Protection(None))));
    assert_eq!(world.read_result(4), Some(&Ok(api::Answer::Permission(api::Permission::Write))));
    match world.read_result(3).expect("settings answered") {
        Ok(api::Answer::Settings(settings)) => {
            assert_eq!(&*settings.default_branch, b"main");
            assert!(settings.merge);
        }
        Ok(_) | Err(_) => panic!("settings projection"),
    }
    world.finish();
}

#[test]
fn bounded_job_bytes_translate_to_a_truncated_attempt_log() {
    let attempt = api::JobAttempt { head: skein_forge_client_world::translate::commit(3), run: 3, job: 7, attempt: 2 };
    let asked = api::Op::Read(api::Read::Job { attempt, max_bytes: 4 });
    let result = skein_forge_client_world::translate::answer(
        &asked,
        skein_fake_forge_domain::api::Answer::Job {
            job: skein_fake_forge_domain::api::Job {
                id: 7,
                run: 3,
                index: 0,
                attempt: 2,
                name: Box::new([]),
                state: skein_fake_forge_domain::api::RunState::Completed,
                conclusion: Some(skein_fake_forge_domain::api::Conclusion::Failure),
                steps: Box::new([]),
            },
            steps: Box::new([
                skein_fake_forge_domain::api::Step {
                    number: 0,
                    name: Box::from(&b"Set up job"[..]),
                    status: skein_fake_forge_domain::api::StepStatus::Success,
                    log: Box::from(&b"set up\n"[..]),
                },
                skein_fake_forge_domain::api::Step {
                    number: skein_fake_forge_domain::api::WORKFLOW_STEP,
                    name: Box::from(&b"Run build"[..]),
                    status: skein_fake_forge_domain::api::StepStatus::Failure,
                    log: Box::from(&b"error"[..]),
                },
                skein_fake_forge_domain::api::Step {
                    number: 2,
                    name: Box::from(&b"Complete job"[..]),
                    status: skein_fake_forge_domain::api::StepStatus::Failure,
                    log: Box::from(&b"job failed\n"[..]),
                },
            ]),
        },
        &skein_forge_client_world::LIMITS,
    );
    assert_eq!(
        result,
        api::Answer::Job {
            attempt,
            step: Some(api::Step { number: 1, name: Box::from(&b"Run build"[..]) }),
            log: Box::from(&b"rror"[..]),
            truncated: true
        }
    );
}
#[test]
fn adoption_reads_history_and_preserves_foreign_or_copied_marker_news() {
    let mut world = World::new(Settings::calm(17));
    let number = world.historical_issue();
    let own: Box<[u8]> = Box::from(&b"\x01\x00\x03kitold"[..]);
    let own_id = world.historical_comment(number, 1, Some(own.clone()));
    let foreign = world.historical_comment(number, 1, Some(Box::from(&b"\x01\x00\x05otherold"[..])));
    let copied = world.historical_comment(number, 2, Some(own));
    let human = world.historical_comment(number, 2, None);
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    let mut ids = skein_lib::List::with_capacity(4);
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Item { comments, .. } => {
                for comment in comments {
                    assert_ne!(comment.id, own_id);
                    ids.push(comment.id).expect("fixture news count");
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Commits { .. }
            | api::Answer::Located { .. }
            | api::Answer::Pull(_)
            | api::Answer::Reviews { .. }
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::File { .. }
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Queued
            | api::Answer::Branch(_)
            | api::Answer::Done => panic!("fixture has only issue news"),
        }
    }
    assert_eq!(ids.as_slice(), &[foreign, copied, human]);
    world.finish();
    assert_eq!(world.stats().calls, world.stats().terminals);
}
#[test]
fn comment_page_recovery_and_old_comment_edits_reach_the_subscriber_once() {
    let mut world = World::new(Settings::calm(23));
    let number = world.historical_issue();
    let mut expected = skein_lib::List::with_capacity(6);
    for _ in 0..5 {
        expected.push(world.historical_comment(number, 2, None)).expect("fixture comment count");
    }
    let first = *expected.get(0).expect("first comment");
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_until_inbox(2);
    world.edit_comment(number, first, Box::from(&b"revised human comment"[..]));
    world.restart();
    world.run_for(1);
    world.restart();
    world.run_for(1);
    expected.push(first).expect("one edit version");
    let observed = comment_ids(&world);
    assert_eq!(observed.as_slice(), expected.as_slice());
    world.restart();
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), expected.as_slice());
    world.finish();
}
fn comment_ids(world: &World) -> skein_lib::List<u64> {
    let mut ids = skein_lib::List::with_capacity(32);
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Item { comments, .. } => {
                for comment in comments {
                    ids.push(comment.id).expect("fixture comment versions");
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Commits { .. }
            | api::Answer::Located { .. }
            | api::Answer::Pull(_)
            | api::Answer::Reviews { .. }
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::File { .. }
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Queued
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    ids
}
#[test]
fn review_page_recovery_keeps_old_pending_submissions_without_duplicate_news() {
    let mut world = World::new(Settings::calm(29));
    let number = world.historical_pull();
    let pending = world.historical_review(number, true);
    let mut expected = skein_lib::List::with_capacity(4);
    for _ in 0..3 {
        expected.push(world.historical_review(number, false)).expect("review count");
    }
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Pull(number) },
        participating: true,
    }]));
    world.run_until_inbox(2);
    world.submit_review(number, pending);
    world.restart();
    world.run_for(1);
    world.restart();
    world.run_for(1);
    expected.push(pending).expect("late submission");
    let mut observed = skein_lib::List::with_capacity(4);
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Reviews { reviews, .. } => {
                for review in reviews {
                    observed.push(review.id).expect("fixture review versions");
                }
            }
            api::Answer::Items { .. }
            | api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Commits { .. }
            | api::Answer::Located { .. }
            | api::Answer::Item { .. }
            | api::Answer::Pull(_)
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::File { .. }
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Queued
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    assert_eq!(observed.as_slice(), expected.as_slice());
    world.restart();
    world.run_for(1);
    world.finish();
}
#[test]
fn inactive_history_does_not_change_a_participating_objects_idle_calls() {
    let mut short = World::new(Settings { history: 2, ..Settings::calm(41) });
    let mut long = World::new(Settings { history: 20, ..Settings::calm(41) });
    for world in [&mut short, &mut long] {
        let number = world.historical_issue();
        world.historical_comment(number, 2, None);
        world.keep(Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Issue(number) },
            participating: true,
        }]));
        world.run_for(60);
    }
    assert_eq!(short.stats().calls, long.stats().calls);
    assert_eq!(short.stats().reads, long.stats().reads);
    short.finish();
    long.finish();
}
#[test]
fn active_comment_history_has_a_bounded_slow_repair_cost_and_catches_a_lost_edit_hint() {
    let short = repair_cost(2);
    let long = repair_cost(20);
    assert_eq!(short.0, long.0, "routine tail polling does not reread unchanged history");
    assert_eq!(long.1.saturating_sub(short.1), 9, "slow repair reads the active history's additional pages");
    assert!(long.1 <= 32, "slow repair and ordinary listing calls are bounded in this horizon");
}
fn repair_cost(history: u32) -> (u32, u32) {
    let mut world = World::new(Settings::calm(43));
    let number = world.historical_issue();
    let mut old = None;
    for _ in 0..history {
        old = Some(world.historical_comment(number, 2, None));
    }
    let old = old.expect("nonempty active history");
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    let before = world.stats().calls;
    world.run_for(10);
    let routine = world.stats().calls.saturating_sub(before);
    world.missed_edit_comment(old, Box::from(&b"edit whose webhook was lost"[..]));
    let before = world.stats().calls;
    world.run_for(40);
    let repair = world.stats().calls.saturating_sub(before);
    let ids = comment_ids(&world);
    assert_eq!(ids.len(), history.checked_add(1).expect("fixture history cap"));
    assert_eq!(ids.get(history), Some(&old));
    world.finish();
    (routine, repair)
}
#[test]
fn slow_repair_finds_a_lost_review_hint_without_item_time_revision_evidence() {
    let mut world = World::new(Settings::calm(47));
    let number = world.historical_pull();
    let pending = world.recent_review(number, true);
    world.recent_review(number, false);
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Pull(number) },
        participating: true,
    }]));
    world.run_until_inbox(1);
    // The pending review is submitted within the same forge time quantum:
    // the item's updated time remains equal and its webhook is lost.
    world.missed_review_submission(number, pending);
    world.run_for(10);
    assert_eq!(review_count(&world), 1);
    world.run_for(40);
    assert_eq!(review_count(&world), 2);
    world.restart();
    world.run_for(1);
    assert_eq!(review_count(&world), 2);
    world.finish();
}
fn review_count(world: &World) -> u32 {
    let mut count = 0_u32;
    for (_, answer) in world.news() {
        match answer {
            api::Answer::Reviews { reviews, .. } => {
                count = count.saturating_add(u32::try_from(reviews.len()).expect("bounded review rows"));
            }
            api::Answer::Items { .. }
            | api::Answer::Runs { .. }
            | api::Answer::Run(_)
            | api::Answer::Tree { .. }
            | api::Answer::Commits { .. }
            | api::Answer::Located { .. }
            | api::Answer::Item { .. }
            | api::Answer::Pull(_)
            | api::Answer::Statuses { .. }
            | api::Answer::Remarks { .. }
            | api::Answer::Commit(_)
            | api::Answer::Branches(_)
            | api::Answer::PullFiles { .. }
            | api::Answer::Compare { .. }
            | api::Answer::Checks(_)
            | api::Answer::File { .. }
            | api::Answer::Job { .. }
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators { .. }
            | api::Answer::Permission(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Queued
            | api::Answer::Branch(_)
            | api::Answer::Done => {}
        }
    }
    count
}
#[test]
fn a_human_edit_of_an_old_own_marker_is_news_even_when_the_original_author_stays() {
    let mut world = World::new(Settings::calm(53));
    let number = world.historical_issue();
    let key = Box::from(&b"\x01\x00\x03kitold"[..]);
    let own = world.historical_comment(number, 1, Some(key));
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    assert!(comment_ids(&world).is_empty());
    let body = skein_forge_client_world::translate::keyed(b"\x01\x00\x03kitold", b"a human revised this");
    world.edit_comment(number, own, body);
    world.run_for(2);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.restart();
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.finish();
}
#[test]
fn a_pre_adoption_human_edit_is_news_despite_our_retained_author_and_marker() {
    let mut world = World::new(Settings::calm(59));
    let number = world.historical_issue();
    let own = world.historical_comment(number, 1, Some(Box::from(&b"\x01\x00\x03kitold"[..])));
    let body = skein_forge_client_world::translate::keyed(b"\x01\x00\x03kitold", b"human edit before adoption");
    world.missed_edit_comment(own, body);
    world.keep(Box::new([Watch {
        resource: Resource { repository: REPO, what: What::Issue(number) },
        participating: true,
    }]));
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.restart();
    world.run_for(1);
    assert_eq!(comment_ids(&world).as_slice(), &[own]);
    world.finish();
}

#[test]
fn a_completed_dispatch_exposes_its_failed_ids_artifact() {
    use skein_fake_forge_domain::api as raw;
    use skein_lib::Duration;
    // b plus its newline is the entire failed-id file.
    const FAILED_BYTES: u32 = 2;
    let mut world = World::with_workflows(
        Settings::calm(61),
        Box::new([raw::Workflow {
            file: Box::from(*b"gate.yml"),
            jobs: Box::new([Box::from(b"tests".as_slice())]),
            fails: Box::new([Box::from(b"a".as_slice()), Box::from(b"b".as_slice())]),
            report: Some(raw::Report {
                artifact: Box::from(*b"tests"),
                format: raw::ReportFormat::Lines,
                deflate: true,
            }),
            select: Some(Box::from(*b"select")),
            latency_min: Duration::from_secs(1),
            latency_max: Duration::from_secs(1),
            silent: 0,
            passes: 1000,
            cue: None,
        }]),
    );
    let head = skein_forge_client_world::translate::commit(world.branch(b"main").expect("main"));
    let mut entry = fixtures::issue(1);
    entry.effect.write = api::Write::Dispatch {
        workflow: Box::from(*b"gate.yml"),
        branch: Box::from(*b"main"),
        inputs: Box::new([api::Input { name: Box::from(*b"select"), value: Box::from(*b"b") }]),
    };
    world.make(entry);
    world.run_for(2);
    world.read(100, api::Read::Runs { workflow: Box::from(*b"gate.yml"), commit: head, page: 1 });
    world.run_for(1);
    let Some(Ok(api::Answer::Runs { runs, .. })) = world.read_result(100) else {
        panic!("run listing");
    };
    let id = runs[0].id;
    world.read(101, api::Read::Run { run: id });
    world.run_for(1);
    let Some(Ok(api::Answer::Run(run))) = world.read_result(101) else {
        panic!("completed run");
    };
    assert_eq!((run.state, run.conclusion), (api::RunState::Completed, Some(api::Conclusion::Failure)));
    world.read(102, api::Read::Artifact { run: id, name: Box::from(*b"tests"), max_bytes: FAILED_BYTES });
    world.run_for(1);
    assert_eq!(
        world.read_result(102),
        Some(&Ok(api::Answer::File {
            head,
            path: Box::from(*b"failed.txt"),
            bytes: Box::from(*b"b\n"),
            truncated: false
        }))
    );
    world.finish();
}
#[test]
fn range_commits_keep_their_messages_across_two_pages() {
    use skein_forge_client_world::translate::commit;
    let mut world = World::new(Settings::calm(67));
    let before = world.branch(b"main").expect("main");
    let first = world.historical_commit(before, b"first\n\nTrailer: one\n");
    let second = world.historical_commit(first, b"second\n\nTrailer: two\n");
    let third = world.historical_commit(second, b"third\n\nTrailer: three\n");
    for (page, expected) in [
        (
            1,
            api::Answer::Commits {
                commits: Box::new([
                    api::CommitMessage {
                        id: commit(first),
                        parents: Box::new([commit(before)]),
                        message: Box::from(*b"first\n\nTrailer: one\n"),
                    },
                    api::CommitMessage {
                        id: commit(second),
                        parents: Box::new([commit(first)]),
                        message: Box::from(*b"second\n\nTrailer: two\n"),
                    },
                ]),
                more: true,
            },
        ),
        (
            2,
            api::Answer::Commits {
                commits: Box::new([api::CommitMessage {
                    id: commit(third),
                    parents: Box::new([commit(second)]),
                    message: Box::from(*b"third\n\nTrailer: three\n"),
                }]),
                more: false,
            },
        ),
    ] {
        world.read(100 + u64::from(page), api::Read::Commits { before: commit(before), after: commit(third), page });
        world.run_for(1);
        assert_eq!(world.read_result(100 + u64::from(page)), Some(&Ok(expected)));
    }
    world.finish();
}
