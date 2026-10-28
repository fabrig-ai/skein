use skein_forge_connection::{Access, Change, Event, Request, Settings, Stopped, settings::Kind, start::Start};
use skein_forge_machine_world::{
    fixtures::{self, FORGE},
    github,
    machine::{Answer, Machine, Pause, Response},
};
use skein_lib::{Duration, Secret};

const SEED: u64 = 7;
const SECOND: Duration = Duration::from_secs(1);
const PAGE_ONE: &str = "GET /app/installations?per_page=100&page=1";
const PAGE_TWO: &str = "GET /app/installations?per_page=100&page=2";
const MINT: &str = "POST /app/installations/1/access_tokens";
const THIRD_MINT: &str = "POST /app/installations/3/access_tokens";
const VIEWER: &str = "POST /graphql";
const MISSING: u64 = 9;
const WRONG: &[u8] = br#"{"data":{"viewer":{"databaseId":8,"login":"other"}}}"#;
const HOUR: u64 = 3600;
fn access() -> Access {
    Access {
        credential: skein_forge::credential::Credential::App { id: 7, key: Secret::new(Box::from(github::app_key())) },
        user: 9,
        webhook_secret: None,
    }
}
fn settings() -> Settings {
    let mut settings = fixtures::settings();
    let forge = settings.forges.get_mut(0).expect("fixture forge");
    forge.kind = Kind::GitHub;
    forge.access = Some(access());
    settings
}
fn accepted(world: &mut Machine, ids: &[u64]) {
    world.action(Request::Installations { forge: FORGE, accepted: Box::from(ids) });
}
fn script(requests: &[&'static str]) -> Vec<Answer> {
    requests.iter().copied().map(Answer::served).collect()
}
fn changes(world: &Machine) -> Vec<(u64, Change)> {
    world
        .installation_events()
        .iter()
        .map(|event| match event {
            Event::Installation { installation, change, .. } => (*installation, *change),
            Event::Fact { .. }
            | Event::GitToken { .. }
            | Event::Answered(_)
            | Event::Hint(_)
            | Event::Connected { .. }
            | Event::Stopped { .. }
            | Event::Listening { .. }
            | Event::ListenerLost
            | Event::Closed => panic!("installation history"),
        })
        .collect()
}
fn ready(world: &Machine) {
    assert_eq!(world.start(FORGE), Some(Start::Ready { cap: 100 }));
    assert_eq!(world.connected_forges(), &[FORGE]);
}
#[test]
fn acceptance_after_empty_start_checks_identity_without_reconnecting() {
    let mut world =
        Machine::github(SEED, fixtures::limits(), settings(), script(&[PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER]));
    world.run_for(SECOND);
    let wall = world.wall();
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    ready(&world);
    assert_eq!(world.requests().len(), 5);
    assert_eq!(world.git_tokens().len(), 1);
    match world.git_tokens().first().expect("Contents token") {
        Event::GitToken { forge, installation, account, token, expires } => {
            assert_eq!((*forge, *installation, account.id, account.login.as_ref()), (FORGE, 1, 22, b"acme".as_slice()));
            assert_eq!(expires.as_secs(), wall.as_secs() + HOUR);
            assert!(!token.is_empty());
        }
        Event::Installation { .. }
        | Event::Fact { .. }
        | Event::Answered(_)
        | Event::Hint(_)
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Closed => panic!("token history"),
    }
    assert!(world.script_done());
    assert_eq!(world.connections_opened(), 1);
    assert!(world.close());
}
#[test]
fn suspension_refresh_reports_loss_without_reminting() {
    let mut world = Machine::github(
        SEED,
        fixtures::limits(),
        settings(),
        script(&[PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER, PAGE_ONE, PAGE_TWO]),
    );
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    assert!(world.github_installations_mut().suspend(1, true));
    accepted(&mut world, &[1, MISSING]);
    world.run_for(SECOND);
    assert!(changes(&world).contains(&(1, Change::Suspended)));
    assert_eq!(world.git_tokens().len(), 1);
    assert_eq!(world.requests().len(), 7);
    ready(&world);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn deletion_refresh_reports_loss_without_reminting() {
    let mut world = Machine::github(
        SEED,
        fixtures::limits(),
        settings(),
        script(&[PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER, PAGE_ONE]),
    );
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    assert!(world.github_installations_mut().remove(1));
    accepted(&mut world, &[1, MISSING]);
    world.run_for(SECOND);
    assert!(changes(&world).contains(&(1, Change::Deleted)));
    assert_eq!(world.git_tokens().len(), 1);
    assert_eq!(world.requests().len(), 6);
    ready(&world);
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn unsuspension_refresh_mints_without_restarting() {
    let mut world = Machine::github(
        SEED,
        fixtures::limits(),
        settings(),
        script(&[PAGE_ONE, PAGE_TWO, PAGE_ONE, PAGE_TWO, THIRD_MINT, THIRD_MINT, VIEWER]),
    );
    accepted(&mut world, &[3]);
    world.run_for(SECOND);
    assert!(world.github_installations_mut().suspend(3, false));
    accepted(&mut world, &[3, MISSING]);
    world.run_for(SECOND);
    assert!(changes(&world).contains(&(3, Change::Unsuspended)));
    assert_eq!(world.git_tokens().len(), 1);
    ready(&world);
    assert!(world.script_done());
    assert!(world.close());
}
fn wrong_script() -> Vec<Answer> {
    let mut script = script(&[PAGE_ONE, PAGE_TWO, MINT, MINT]);
    script.push(Answer { request: VIEWER, response: Response::Bytes(fixtures::response(200, WRONG)) });
    script
}
#[test]
fn late_wrong_viewer_stops_an_already_connected_generation() {
    let mut world = Machine::github(SEED, fixtures::limits(), settings(), wrong_script());
    world.run_for(SECOND);
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    assert_eq!(world.connected_forges(), &[FORGE]);
    assert_eq!(world.stopped(), Some((FORGE, Stopped::OtherUser { user: 8 })));
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn replacement_after_late_refusal_has_its_own_single_answers() {
    let mut script = wrong_script();
    script.extend(crate::script(&[PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER]));
    let mut world = Machine::github(SEED, fixtures::limits(), settings(), script);
    world.run_for(SECOND);
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    world.action(Request::Connect { forge: FORGE, access: access() });
    world.run_for(SECOND);
    assert_eq!(world.connected_forges(), &[FORGE, FORGE]);
    assert_eq!(world.stopped(), Some((FORGE, Stopped::OtherUser { user: 8 })));
    assert_eq!(world.start(FORGE), Some(Start::Ready { cap: 100 }));
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn replaced_viewer_answer_cannot_stop_the_new_generation() {
    let mut script = wrong_script();
    script.extend(crate::script(&[PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER]));
    let mut world = Machine::github(SEED, fixtures::limits(), settings(), script);
    world.run_for(SECOND);
    // Two pages and two mints precede the old Viewer.
    world.pause_at(4, Pause::Reading(0));
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    world.action(Request::Connect { forge: FORGE, access: access() });
    world.run_for(SECOND);
    assert_eq!(world.connected_forges(), &[FORGE, FORGE]);
    assert_eq!(world.stopped(), None);
    assert!(world.script_done());
    assert!(world.close());
}

#[test]
fn lost_mint_waiting_for_room_is_never_written() {
    const STARTUP_PAGES: usize = 2;
    let mut world = Machine::github(SEED, fixtures::limits(), settings(), script(&[PAGE_ONE, PAGE_TWO]));
    world.run_for(SECOND);
    world.pause_at(STARTUP_PAGES, Pause::Writing(0));
    accepted(&mut world, &[1]);
    accepted(&mut world, &[]);
    world.run_for(SECOND);
    assert_eq!(world.requests().len(), STARTUP_PAGES);
    assert!(world.git_tokens().is_empty());
    ready(&world);
    assert!(world.script_done());
    assert!(world.close());
}

#[test]
fn a_refused_answer_is_offered_once_per_offer() {
    use skein_forge::api;
    use skein_lib::Token;
    const FIRST: u64 = 41;
    const COMPETING: u64 = 42;
    const REPO: api::Repository = api::Repository { forge: FORGE, repository: 2 };
    const RESOLVE: &str = "GET /repositories/2";
    const BRANCH: &str = "GET /repos/acme/widgets/branches/main";
    let mut world = Machine::github(
        SEED,
        fixtures::limits(),
        settings(),
        script(&[PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER, RESOLVE, BRANCH, BRANCH]),
    );
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    let branch = || api::Op::Read(api::Read::Branch { branch: Box::from(b"main".as_slice()) });
    world.call(FIRST, REPO, branch());
    world.run_for(SECOND);
    let answer = world.answered(FIRST).expect("first answer").clone();
    world.action(Request::Refused(answer));
    world.call(COMPETING, REPO, branch());
    world.run_for(SECOND);
    world.action(Request::Offer);
    world.action(Request::Offer);
    assert_eq!(
        world.deliveries().iter().map(|answer| answer.call).collect::<Vec<_>>(),
        [Token::new(FIRST), Token::new(COMPETING), Token::new(FIRST)]
    );
    assert!(world.close());
}

#[test]
fn a_held_git_token_is_not_released_after_its_installation_is_lost() {
    // The second mint is Contents; its reply remains below while acceptance is withdrawn.
    const CONTENTS_REPLY: usize = 3;
    let mut world = Machine::github(SEED, fixtures::limits(), settings(), script(&[PAGE_ONE, PAGE_TWO, MINT, MINT]));
    world.pause_at(CONTENTS_REPLY, Pause::Reading(0));
    accepted(&mut world, &[1]);
    world.run_for(SECOND);
    accepted(&mut world, &[]);
    world.action(Request::Offer);
    world.release_response();
    world.run_for(SECOND);
    assert!(world.git_tokens().is_empty());
    assert!(world.script_done());
    assert!(world.close());
}
