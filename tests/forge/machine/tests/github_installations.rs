use skein_forge_connection::{Access, Change, Event, Fact, Request, Settings, settings::Kind};
use skein_forge_machine_world::{
    fixtures::{self, FORGE},
    machine::{Answer, Machine},
};
use skein_lib::{Duration, Secret};

const SEED: u64 = 7;
const SECOND: Duration = Duration::from_secs(1);
const PAGE_ONE: &str = "GET /app/installations?per_page=100&page=1";
const PAGE_TWO: &str = "GET /app/installations?per_page=100&page=2";
const MINT: &str = "POST /app/installations/1/access_tokens";
const VIEWER: &str = "POST /graphql";
const MISSING: u64 = 9;
const ALSO_MISSING: u64 = 10;
fn settings() -> Settings {
    let mut settings = fixtures::settings();
    let forge = settings.forges.get_mut(0).expect("forge");
    forge.kind = Kind::GitHub;
    forge.access = Some(Access {
        credential: skein_forge::credential::Credential::App {
            id: 7,
            key: Secret::new(Box::from(*skein_fake_forge_github::keys::KEYS.first().expect("public test key"))),
        },
        user: 9,
        webhook_secret: None,
    });
    settings
}
type InstallationChange<'a> = (u64, Option<(u64, &'a [u8])>, Change);
fn changes(world: &Machine) -> Vec<InstallationChange<'_>> {
    world
        .installation_events()
        .iter()
        .map(|event| match event {
            Event::Installation { installation, account, change, .. } => {
                (*installation, account.as_ref().map(|account| (account.id, account.login.as_ref())), *change)
            }
            Event::Fact { .. }
            | Event::GitToken { .. }
            | Event::Answered(_)
            | Event::Hint(_)
            | Event::Connected { .. }
            | Event::Stopped { .. }
            | Event::Listening { .. }
            | Event::ListenerLost
            | Event::Closed => panic!("installation collection"),
        })
        .collect()
}
#[test]
fn startup_changes_follow_the_accepted_baseline() {
    let mut world = Machine::github(
        SEED,
        fixtures::limits(),
        settings(),
        [PAGE_ONE, PAGE_TWO, MINT, MINT, VIEWER].into_iter().map(Answer::served).collect(),
    );
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([1, 3, MISSING]) });
    world.run_for(SECOND);
    assert_eq!(
        changes(&world),
        vec![
            (2, Some((23, b"example".as_slice())), Change::Created),
            (3, Some((24, b"other".as_slice())), Change::Suspended),
            (MISSING, None, Change::Deleted),
        ]
    );
    assert_eq!(world.connected_forges(), &[FORGE]);
    assert_eq!(world.git_tokens().len(), 1);
    assert!(world.script_done());
    assert!(world.facts().is_empty());
    assert!(world.close());
}
#[test]
fn missing_installation_is_reported_once_on_the_wire() {
    let mut world = Machine::github(
        SEED,
        fixtures::limits(),
        settings(),
        [PAGE_ONE, PAGE_TWO, PAGE_ONE, PAGE_TWO].into_iter().map(Answer::served).collect(),
    );
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([3]) });
    world.run_for(SECOND);
    let baseline = world.installation_events().len();
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([3, MISSING, ALSO_MISSING]) });
    world.run_for(SECOND);
    assert_eq!(
        &changes(&world)[baseline..],
        &[(MISSING, None, Change::Deleted), (ALSO_MISSING, None, Change::Deleted)]
    );
    let count = world.installation_events().len();
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([ALSO_MISSING, 3, MISSING]) });
    world.run_for(Duration::from_secs(100));
    assert_eq!(world.requests().len(), 4);
    assert_eq!(world.installation_events().len(), count);
    assert_eq!(world.connected_forges(), &[FORGE]);
    assert!(world.git_tokens().is_empty());
    assert!(world.script_done());
    assert!(world.close());
}
#[test]
fn overflow_keeps_a_later_accepted_installation() {
    let mut world = Machine::github(
        SEED,
        skein_forge_connection::Limits { installations: 1, ..fixtures::limits() },
        settings(),
        [PAGE_ONE, PAGE_TWO].into_iter().map(Answer::served).collect(),
    );
    // The fake's id 3 is suspended, so the bounded read needs no token mint.
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([3]) });
    world.run_for(SECOND);
    assert_eq!(changes(&world), vec![(3, Some((24, b"other".as_slice())), Change::Suspended)]);
    assert_eq!(world.facts().len(), 1);
    match world.facts().first().expect("overflow fact") {
        Event::Fact { forge, fact } => assert_eq!((*forge, fact.clone()), (FORGE, Fact::Unkept { installation: 2 })),
        Event::Installation { .. }
        | Event::GitToken { .. }
        | Event::Answered(_)
        | Event::Hint(_)
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Closed => panic!("fact collection"),
    }
    assert_eq!(world.connected_forges(), &[FORGE]);
    assert!(world.git_tokens().is_empty());
    assert!(world.script_done());
    assert!(world.close());
}

#[test]
fn held_installation_changes_reach_the_owner_before_the_next_page_is_asked() {
    use skein_forge_machine_world::machine::Response;
    // Twenty retained rows exceed the sixteen-event entry-point allowance; one more makes an Unkept fact.
    const RETAINED: u32 = 20;
    const ROWS: u32 = RETAINED + 1;
    const OUTPUTS: u32 = RETAINED + 1;
    const MISSING_ACCOUNT: u64 = 100;
    let body = (1..=ROWS).map(|id| format!(
        r#"{{"id":{id},"account":{{"id":{id},"login":"account{id}","type":"User"}},"created_at":"1970-01-01T00:00:01Z","suspended_at":null}}"#
    )).collect::<Vec<_>>().join(",");
    let body = format!("[{body}]");
    let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nLink: <https://api.github.test/app/installations?per_page=100&page=2>; rel=\"next\"\r\n\r\n{body}", body.len()).into_bytes();
    let mut world = Machine::github(
        SEED,
        skein_forge_connection::Limits { installations: RETAINED, ..fixtures::limits() },
        settings(),
        vec![
            Answer { request: PAGE_ONE, response: Response::Bytes(response) },
            Answer { request: PAGE_TWO, response: Response::Bytes(fixtures::response(200, b"[]")) },
            Answer { request: PAGE_ONE, response: Response::Silent },
        ],
    );
    world.hold_output();
    world.run_for(SECOND);
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([MISSING_ACCOUNT]) });
    world.run_for(SECOND);
    assert_eq!(world.outputs_before_requests().get(1), Some(&1), "the held overflow fact precedes page two");
    assert_eq!(
        world.outputs_before_requests().get(2),
        Some(&OUTPUTS.saturating_add(1)),
        "all changes and Connected precede the refresh page"
    );
    world.release_output();
    assert_eq!(world.installation_events().len(), usize::try_from(RETAINED).expect("rows"));
    assert_eq!(world.facts().len(), 1);
    assert_eq!(
        changes(&world).iter().map(|row| row.0).collect::<Vec<_>>(),
        (1..=u64::from(RETAINED)).collect::<Vec<_>>()
    );
    assert!(world.close());
}
