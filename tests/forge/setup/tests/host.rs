use core::num::NonZeroU64;
use skein_forge_setup::{
    host::{Need, Plan},
    item::{self, Item},
    reconcile::{Failure, Outcome, Stage, Why},
};
use skein_forge_setup_world::{Connect, Heard, HostWorld, fixtures};
use skein_lib::{Duration, Token};
fn stopped(stage: Stage, subject: &[u8], at: Duration) -> Heard {
    Heard::Stop {
        run: Token::new(1),
        failure: Failure { forge: 1, step: stage, subject: subject.into(), why: Why::Down },
        at,
    }
}
#[test]
fn the_host_reconciles_an_empty_forge_through_http() {
    let mut world = HostWorld::new(7, &[]);
    world.run(1);
    world.finish();
    assert_eq!(world.heard(), &[Heard::Done { run: Token::new(1), outcome: Outcome::Nothing, at: Duration::ZERO }]);
    assert_eq!(world.file(Item::User).expect("User written").expose(), &2u64.to_le_bytes());
    for key in [Item::ApiToken, Item::GitToken] {
        assert!(item::token(world.file(key).expect("token written").expose(), 64).is_some());
    }
}
#[test]
fn the_host_runs_a_plan_that_names_adopted_repositories() {
    use skein_fake_forge_protocol::Route;
    let mut without = HostWorld::new(7, &[]);
    without.run(1);
    without.finish();
    let mut world = HostWorld::new(7, &[]);
    world.run_adopted(1, &[1, 2]);
    world.finish();
    assert_eq!(world.heard(), &[Heard::Done { run: Token::new(1), outcome: Outcome::Nothing, at: Duration::ZERO }]);
    assert_eq!(world.requests().total, without.requests().total + 2);
    assert_eq!(world.requests().routes.get(&Route::RepositoryById), Some(&2));
    for (route, count) in &without.requests().routes {
        assert_eq!(world.requests().routes.get(route), Some(count));
    }
    assert_eq!(world.requests().routes.len(), without.requests().routes.len() + 1);
    assert_eq!(world.requests().unrouted, 0);
}
#[test]
fn refused_and_unavailable_first_tries_wait_doubling_then_converge() {
    let mut world = HostWorld::new(7, &[Connect::Refused, Connect::Unavailable, Connect::Refused]);
    world.run(1);
    world.finish();
    assert_eq!(
        world.heard(),
        &[Heard::Done { run: Token::new(1), outcome: Outcome::Nothing, at: Duration::from_millis(700) }]
    );
    assert_eq!(&world.connects()[..4], &[0, 100, 300, 700].map(Duration::from_millis));
}
#[test]
fn a_down_after_the_first_answer_stops_the_run_at_its_step() {
    let mut world = HostWorld::new(7, &[Connect::Made, Connect::Refused]);
    world.run(1);
    world.finish();
    assert_eq!(world.heard(), &[stopped(Stage::Bot, b"kit", Duration::ZERO)]);
}
#[test]
fn a_forge_that_never_answers_stops_down_at_its_deadline() {
    let mut world = HostWorld::new(7, &[Connect::Silent]);
    world.run(1);
    world.finish();
    assert_eq!(world.heard(), &[stopped(Stage::Admin, b"kit-admin", Duration::from_secs(120))]);
    assert_eq!(world.aborts(), 1);
}
#[test]
fn close_mid_run_ends_it_unheard_and_says_closed() {
    let mut world = HostWorld::new(7, &[Connect::Silent]);
    world.run(1);
    world.settle();
    world.close();
    world.settle();
    assert_eq!(world.heard(), &[Heard::Closed { at: Duration::ZERO }]);
    assert_eq!(world.aborts(), 1);
}

fn connected(world: &mut HostWorld) -> u64 {
    let id = fixtures::seed_connected_tokens(world.store_mut());
    world.put(Item::ApiToken, fixtures::secret(fixtures::API));
    world.put(Item::GitToken, fixtures::secret(fixtures::GIT));
    id
}
fn identity(id: u64, at: Duration) -> Heard {
    Heard::Done { run: Token::new(1), outcome: Outcome::Identity { id, login: b"alice".as_slice().into() }, at }
}
fn done(run: u64) -> Heard {
    Heard::Done { run: Token::new(run), outcome: Outcome::Nothing, at: Duration::ZERO }
}
#[test]
fn the_host_checks_two_tokens_of_one_user_and_hands_up_the_identity() {
    let mut world = HostWorld::new(7, &[]);
    let id = connected(&mut world);
    world.run_plan(1, Plan::Check { forge: 1 });
    world.finish();
    assert_eq!(world.heard(), &[identity(id, Duration::ZERO)]);
    assert_eq!(
        world.needs(),
        &[
            Need::Read(Item::ApiToken),
            Need::Read(Item::GitToken),
            Need::Read(Item::User),
            Need::Write { item: Item::User, bytes: fixtures::secret(&id.to_le_bytes()) },
        ]
    );
    assert_eq!(world.file(Item::User).expect("written before Done").expose(), &id.to_le_bytes());
}
#[test]
fn the_host_stops_a_check_of_two_users_naming_git() {
    let mut world = HostWorld::new(7, &[]);
    connected(&mut world);
    let other = world.store_mut().account(b"another", false, b"Another person");
    let token = fixtures::secret(b"another-persons-git-token");
    world.store_mut().token(other, b"git", &[b"write:repository", b"read:user"], token.clone());
    world.put(Item::GitToken, token);
    world.run_plan(1, Plan::Check { forge: 1 });
    world.finish();
    assert_eq!(
        world.heard(),
        &[Heard::Stop {
            run: Token::new(1),
            failure: Failure { forge: 1, step: Stage::Token, subject: b"git".as_slice().into(), why: Why::OtherUser },
            at: Duration::ZERO,
        }]
    );
    assert_eq!(world.needs(), &[Need::Read(Item::ApiToken), Need::Read(Item::GitToken)]);
    assert!(world.file(Item::User).is_none());
}
#[test]
fn a_check_with_no_user_item_kept_writes_the_id_as_eight_bytes() {
    let mut world = HostWorld::new(7, &[]);
    let id = connected(&mut world);
    world.pause_writes();
    world.run_plan(1, Plan::Check { forge: 1 });
    world.settle();
    assert!(world.heard().is_empty());
    assert!(world.file(Item::User).is_none());
    assert_eq!(
        world.needs(),
        &[
            Need::Read(Item::ApiToken),
            Need::Read(Item::GitToken),
            Need::Read(Item::User),
            Need::Write { item: Item::User, bytes: fixtures::secret(&id.to_le_bytes()) },
        ]
    );
    world.write_pending();
    world.finish();
    assert_eq!(world.heard(), &[identity(id, Duration::ZERO)]);
    assert_eq!(world.file(Item::User).expect("written").expose(), &id.to_le_bytes());
}
#[test]
fn a_check_waits_for_the_forge_as_a_reconcile_does() {
    let mut world = HostWorld::new(7, &[Connect::Refused, Connect::Unavailable]);
    let id = connected(&mut world);
    world.run_plan(1, Plan::Check { forge: 1 });
    world.finish();
    assert_eq!(world.heard(), &[identity(id, Duration::from_millis(300))]);
    assert_eq!(world.connects(), &[0, 100, 300, 300].map(Duration::from_millis));
}
#[test]
fn the_host_takes_over_the_handed_token_and_deletes_the_older_ones() {
    use skein_fake_forge_protocol::Route;
    let mut world = HostWorld::new(7, &[]);
    let admin = fixtures::seed_admin_handover(world.store_mut(), 1);
    let older: Vec<_> = world.store().issued().iter().take(2).map(|token| token.secret.clone()).collect();
    world.pause_writes();
    world.run_plan(1, Plan::TakeOver { forge: 1, admin_token: fixtures::secret(fixtures::TAKEN) });
    world.settle();
    assert!(world.heard().is_empty());
    assert_eq!(world.file(Item::AdminToken).expect("old token").expose(), fixtures::ADMIN);
    assert_eq!(world.needs(), &[Need::Write { item: Item::AdminToken, bytes: fixtures::secret(fixtures::TAKEN) }]);
    assert_eq!(world.requests().routes.get(&Route::DeleteToken), None);
    world.write_pending();
    world.finish();
    assert_eq!(world.heard(), &[done(1)]);
    assert_eq!(world.file(Item::AdminToken).expect("taken token written").expose(), fixtures::TAKEN);
    assert_eq!(world.requests().routes.get(&Route::DeleteToken), Some(&2));
    for secret in older {
        assert_eq!(world.store().token_user(secret.expose()), None);
    }
    assert_eq!(world.store().token_user(fixtures::TAKEN), Some(admin));
}
#[test]
fn a_reconcile_after_a_take_over_uses_the_taken_token() {
    let mut world = HostWorld::new(7, &[]);
    fixtures::seed_admin_handover(world.store_mut(), 1);
    world.run_plan(1, Plan::TakeOver { forge: 1, admin_token: fixtures::secret(fixtures::TAKEN) });
    world.finish();
    let since = world.needs().len();
    world.run(2);
    world.finish();
    assert_eq!(world.needs().get(since), Some(&Need::Read(Item::AdminToken)));
    assert_eq!(world.file(Item::AdminToken).expect("taken token").expose(), fixtures::TAKEN);
    assert_eq!(world.store().token_user(fixtures::ADMIN), None);
    assert_eq!(world.heard(), &[done(1), done(2)]);
}
#[test]
fn the_host_adopts_a_repository_and_hands_up_nothing() {
    use skein_forge::forgejo::request::Grant;
    let mut world = HostWorld::with_config(7, &[], fixtures::repository_config(&[b"alice/specimen"]));
    fixtures::seed_bot_tokens(world.store_mut());
    world.run_plan(1, Plan::Adopt { forge: 1, repository: NonZeroU64::new(1).unwrap() });
    world.finish();
    assert_eq!(world.store().collaborator(1, b"kit"), Some(Grant::Write));
    assert_eq!(world.heard(), &[done(1)]);
}
#[test]
fn an_adopt_of_a_repository_the_forge_lacks_stops_missing_naming_its_id() {
    let mut world = HostWorld::new(7, &[]);
    fixtures::seed_bot_tokens(world.store_mut());
    world.run_plan(1, Plan::Adopt { forge: 1, repository: NonZeroU64::new(99).unwrap() });
    world.finish();
    assert_eq!(
        world.heard(),
        &[Heard::Stop {
            run: Token::new(1),
            failure: Failure { forge: 1, step: Stage::Repository, subject: b"99".as_slice().into(), why: Why::Missing },
            at: Duration::ZERO,
        }]
    );
}

// forge-setup.md, 4.8: the fixture's picker page holds four repositories.
const PICKER_PAGE: u32 = 4;
const ADMIN_READ: Need = Need::Read(Item::AdminToken);
fn reachable(page: u32) -> Plan {
    Plan::Reachable {
        forge: 1,
        installation: None,
        page: core::num::NonZeroU32::new(page).expect("picker pages start at one"),
    }
}
fn picker_world(script: &[Connect]) -> HostWorld {
    let mut config = fixtures::config_with_cap(PICKER_PAGE);
    fixtures::picker_repositories(&mut config);
    let mut world = HostWorld::with_config(7, script, config);
    fixtures::picker_owners(world.store_mut());
    world
}
fn picker_page(heard: &Heard, run: u64, ids: &[u64], more: bool) {
    let Heard::Done { run: token, outcome: Outcome::Reachable { rows, more: found_more }, at } = heard else {
        panic!("picker Done: {heard:?}");
    };
    assert_eq!(*token, Token::new(run));
    assert_eq!(*at, Duration::ZERO);
    assert_eq!(*found_more, more);
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), ids);
}
fn busy(run: u64) -> Heard {
    Heard::Stop {
        run: Token::new(run),
        failure: Failure { forge: 1, step: Stage::Admin, subject: b"kit-admin".as_slice().into(), why: Why::Busy },
        at: Duration::ZERO,
    }
}
#[test]
fn the_host_pages_through_the_repositories_with_the_admins_token() {
    let mut world = picker_world(&[]);
    for page in [1, 2, 3] {
        world.run_plan(u64::from(page), reachable(page));
        world.finish();
    }
    assert_eq!(world.heard().len(), 3);
    for (heard, (run, ids, more)) in world.heard().iter().zip([
        (1, [1, 2, 3, 4].as_slice(), true),
        (2, [5, 6, 7, 8].as_slice(), true),
        (3, [9, 10].as_slice(), false),
    ]) {
        picker_page(heard, run, ids, more);
    }
    assert_eq!(world.needs(), &[ADMIN_READ; 3]);
    assert_eq!(world.requests().routes.get(&skein_fake_forge_protocol::Route::RepoSearch), Some(&3));
}
#[test]
fn a_repository_search_that_answered_ok_reaches_the_plan_as_its_rows() {
    let mut world = picker_world(&[]);
    world.run_plan(1, reachable(1));
    world.finish();
    assert_eq!(world.heard().len(), 1);
    picker_page(&world.heard()[0], 1, &[1, 2, 3, 4], true);
    assert_eq!(world.needs(), &[Need::Read(Item::AdminToken)]);
}
#[test]
fn a_repository_search_that_answered_not_ok_reaches_the_plan_as_a_document() {
    use skein_fake_forge_protocol::Route;
    let mut world = picker_world(&[Connect::Made, Connect::Made, Connect::Answers(200, br#"{"ok":false,"data":[]}"#)]);
    world.run_plan(1, reachable(1));
    world.finish();
    assert_eq!(
        world.heard(),
        &[Heard::Stop {
            run: Token::new(1),
            failure: Failure {
                forge: 1,
                step: Stage::Token,
                subject: b"kit-admin".as_slice().into(),
                why: Why::Unexpected
            },
            at: Duration::ZERO,
        }]
    );
    assert_eq!(world.connects(), &[Duration::ZERO; 3]);
    assert_eq!(world.requests().routes.get(&Route::User), Some(&1));
    assert_eq!(world.requests().routes.get(&Route::ApiSettings), Some(&1));
    assert_eq!(world.needs(), &[Need::Read(Item::AdminToken)]);
}
#[test]
fn a_reachable_run_is_served_beside_a_held_reconcile() {
    let mut world = picker_world(&[Connect::Silent]);
    world.run(1);
    world.settle();
    world.run_plan(2, reachable(1));
    world.settle();
    assert_eq!(world.heard().len(), 1);
    picker_page(&world.heard()[0], 2, &[1, 2, 3, 4], true);
    world.finish();
    assert_eq!(world.heard().get(1), Some(&stopped(Stage::Admin, b"kit-admin", Duration::from_secs(120))));
    assert_eq!(world.heard().len(), 2);
    assert_eq!(world.aborts(), 1);
}
#[test]
fn a_second_reachable_for_the_forge_stops_busy_while_the_first_is_held() {
    let mut world = picker_world(&[Connect::Silent]);
    world.run_plan(1, reachable(1));
    world.settle();
    world.run_plan(2, reachable(1));
    world.settle();
    assert_eq!(world.heard(), &[busy(2)]);
    world.finish();
    assert_eq!(world.heard(), &[busy(2), stopped(Stage::Admin, b"kit-admin", Duration::from_secs(120))]);
    assert_eq!(world.aborts(), 1);
    assert_eq!(world.connects(), &[Duration::ZERO]);
}
#[test]
fn the_host_admits_a_reachable_beside_a_writing_run_refuses_a_second_and_ends_both() {
    let mut world = picker_world(&[]);
    world.pause_reads();
    world.run(1);
    world.run_plan(2, reachable(1));
    world.run_plan(3, reachable(1));
    world.settle();
    assert_eq!(world.pending_reads(), &[(Token::new(1), Item::AdminToken), (Token::new(2), Item::AdminToken)]);
    assert_eq!(world.needs(), &[ADMIN_READ; 2]);
    assert_eq!(world.heard(), &[busy(3)]);
    assert!(world.connects().is_empty());
    world.read_pending(2);
    world.settle();
    assert_eq!(world.heard().len(), 2);
    picker_page(&world.heard()[1], 2, &[1, 2, 3, 4], true);
    world.resume_reads();
    world.finish();
    assert_eq!(world.heard().len(), 3);
    assert_eq!(world.heard()[0], busy(3));
    assert_eq!(world.heard()[2], done(1));
}

#[test]
fn an_unclosed_reply_holds_the_slot_until_ios_close_deadline() {
    let mut limits = fixtures::host_limits();
    limits.runs = 1;
    let mut world = HostWorld::new_with(7, &[Connect::AnswersOpen(401, b"")], fixtures::host_settings(), limits);
    world.run(1);
    world.settle();
    assert!(matches!(world.heard(), [Heard::Stop { failure, .. }] if failure.why == Why::Refused(401)));
    world.run(2);
    world.settle();
    assert!(
        matches!(world.heard().last(), Some(Heard::Stop { run, failure, .. }) if *run == Token::new(2) && failure.why == Why::Busy)
    );
    let deadline = skein_lib::Time::ZERO.saturating_add(limits.io.close_timeout);
    world.advance_to(skein_lib::Time::from_nanos(deadline.as_nanos() - 1));
    assert_eq!(world.aborts(), 0);
    world.advance_to(deadline);
    assert_eq!(world.aborts(), 1);
    world.run(2);
    world.finish();
    assert!(
        matches!(world.heard().last(), Some(Heard::Done { run, outcome: Outcome::Nothing, .. }) if *run == Token::new(2))
    );
}

#[test]
fn runs_finishing_together_keep_each_run_s_needs_with_it_and_in_order() {
    use skein_forge_setup::host::{Forge, Kind, Transport};
    use skein_lib::List;
    // Three independent checks share one host and finish at the same virtual time.
    const RUNS: [(u64, u16); 3] = [(11, 1), (22, 2), (33, 3)];
    const COUNT: u32 = 3;
    const PREPARE: u64 = 99;
    const CHECK_READS: [Item; 3] = [Item::ApiToken, Item::GitToken, Item::User];
    let mut settings = fixtures::host_settings();
    let mut forges = List::with_capacity(COUNT);
    for (_, number) in RUNS {
        let mut given = fixtures::given(false);
        given.forge = number;
        forges
            .push(Forge {
                kind: Kind::Forgejo(given),
                address: skein_io::Addr::Ip("127.0.0.1:3000".parse().expect("fixture address")),
                transport: Transport::Plaintext { name: None },
            })
            .expect("fixture forge");
    }
    settings.forges = forges;
    let mut world = HostWorld::new_with(
        7,
        &[],
        settings,
        skein_forge_setup::host::Limits { forges: COUNT, runs: COUNT, ..fixtures::host_limits() },
    );
    world.run(PREPARE);
    world.finish();
    let baseline = world.needs_by_run().len();
    world.pause_reads();
    for (run, forge) in RUNS {
        world.run_plan(run, Plan::Check { forge });
    }
    world.settle();
    // Hold each round of needs, then answer every run before allowing the next turn.
    for _ in CHECK_READS {
        for (run, _) in RUNS {
            world.read_pending(run);
        }
        world.settle();
    }
    for (run, _) in RUNS {
        let needs = world.needs_by_run()[baseline..]
            .iter()
            .filter_map(|(token, need)| (*token == Token::new(run)).then_some(need.clone()))
            .collect::<Vec<_>>();
        assert_eq!(needs, [Need::Read(Item::ApiToken), Need::Read(Item::GitToken), Need::Read(Item::User)]);
        assert!(world.heard().iter().any(
            |event| matches!(event, Heard::Done { run: token, at: Duration::ZERO, .. } if *token == Token::new(run))
        ));
    }
    assert_eq!(world.needs_by_run().len() - baseline, RUNS.len() * CHECK_READS.len());
}
