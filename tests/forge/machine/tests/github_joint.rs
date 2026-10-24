use skein_fake_forge_github::keys::KEYS;
use skein_forge::{
    api::{self, Answer, Op, Read},
    credential::Credential,
};
use skein_forge_connection::{Access, Change, Component, Event, Request, Stopped, start::Start};
use skein_forge_machine_world::{
    fixtures::{self, FORGE},
    github::WALL_BASE,
    joint::{self, Joint, REPO},
};
use skein_lib::{Duration, Secret, Time, Wall};

const SEED: u64 = 7;
const SECOND: Duration = Duration::from_secs(1);
const ACCEPTED: u64 = 1;
const CALL: u64 = 1;
const READY: Start = Start::Ready { cap: 100 };
// The accepted unsuspended installation is silent; example and suspended other emit changes.
const INSTALLATION_EVENTS: usize = 2;
// Listing has two pages; startup also mints API and git tokens and reads the viewer.
const START_REQUESTS: u64 = 5;
// A fresh branch call resolves the repository and reads its branch.
const CALL_REQUESTS: u64 = 2;

fn world(seed: u64, cut_chance: u32) -> Joint {
    Joint::github(
        seed,
        cut_chance,
        fixtures::limits(),
        fixtures::github_settings(),
        fixtures::shared(),
        joint::checks(1000),
    )
}
fn connect(world: &mut Joint, access: Access) {
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([ACCEPTED]) });
    world.action(Request::Connect { forge: FORGE, access });
}
fn branch(world: &mut Joint) {
    world.call(CALL, REPO, fixtures::branch());
    world.run_for(SECOND);
}

#[test]
fn records_then_connect_make_github_ready_for_repository_calls() {
    let mut world = world(SEED, 0);
    world.run_for(SECOND);
    assert_eq!(world.connected, 0);
    assert_eq!(world.tally().total, 0);
    assert_eq!(world.component.start(FORGE), Some(Start::Unconnected));
    connect(&mut world, fixtures::github_access());
    world.run_for(SECOND);
    assert_eq!(world.connected, 1);
    assert_eq!(world.component.start(FORGE), Some(READY));
    assert_eq!(world.installations.len(), INSTALLATION_EVENTS);
    assert!(world.installations.iter().any(|event| matches!(event,
        Event::Installation { forge: FORGE, installation: 2, account: Some(account), change: Change::Created }
        if account.id == 23 && account.login.as_ref() == b"example"
    )));
    assert_eq!(world.git_tokens.len(), 1);
    assert!(matches!(world.git_tokens.first(), Some(Event::GitToken {
        forge: FORGE, installation: ACCEPTED, account, token, ..
    }) if account.id == 22 && account.login.as_ref() == b"acme" && !token.is_empty()));
    branch(&mut world);
    assert!(matches!(world.answered(CALL).expect("branch answer").result, Ok(Answer::Commit(_))));
    assert_eq!(world.answers.len(), 1);
    assert_eq!(world.tally().total, START_REQUESTS + CALL_REQUESTS);
    assert_eq!(world.start_counts().served, START_REQUESTS);
    assert_eq!(world.request_counts().written, CALL_REQUESTS);
    assert_eq!(world.tally().users.get(&9), Some(&(START_REQUESTS + CALL_REQUESTS)));
    assert_eq!(world.tally().routes.get(&skein_fake_forge_protocol::Route::Branch), Some(&1));
}

#[test]
fn cutting_github_pipes_connect_and_answer_each_call_once() {
    let mut cut_heads = 0;
    // Fixed seeds cover resets during startup and calls at a 30% connection cut chance.
    for seed in [3, 7, 19] {
        let mut world = world(seed, 300);
        connect(&mut world, fixtures::github_access());
        world.run_for(Duration::from_secs(30));
        assert_eq!(world.connected, 1, "seed {seed}");
        assert_eq!(world.component.start(FORGE), Some(READY), "seed {seed}");
        branch(&mut world);
        assert_eq!(world.answers.len(), 1, "seed {seed}");
        assert!(world.answered(CALL).expect("one branch terminal").result.is_ok(), "seed {seed}");
        cut_heads += world.start_counts().cut_in_head + world.request_counts().cut_in_head;
    }
    assert!(cut_heads > 0, "fixed seeds cut request heads");
}

#[test]
fn a_refused_github_app_key_is_kept_as_stopped() {
    let mut world = world(SEED, 0);
    let mut access = fixtures::github_access();
    access.credential =
        Credential::App { id: 7, key: Secret::new(Box::from(*KEYS.get(1).expect("other public test key"))) };
    connect(&mut world, access);
    world.run_for(SECOND);
    assert_eq!(world.stopped, [(FORGE, Stopped::KeyRefused)]);
    assert_eq!(world.connected, 0);
    assert!(world.git_tokens.is_empty());
    assert_eq!(world.tally().unauthorized, 1);
}

#[test]
fn github_time_follows_the_component_and_fake_wall() {
    let mut world = world(SEED, 0);
    world.time(Time::from_nanos(SECOND.as_nanos()));
    let wall = Wall::from_nanos(WALL_BASE.as_nanos() + SECOND.as_nanos());
    assert_eq!(world.env.wall, wall);
    assert_eq!(world.fake_env.wall, wall);
    assert_eq!(world.fake_env.now.as_nanos(), wall.as_nanos());
    assert_eq!(world.now(), Time::from_nanos(SECOND.as_nanos()));
}

#[test]
fn github_replacement_preserves_the_domain_and_configured_access() {
    let mut world = world(SEED, 0);
    connect(&mut world, fixtures::github_access());
    world.run_for(SECOND);
    world.issue(b"from the public domain", b"body");
    let mut settings = fixtures::github_settings();
    settings.forges.get_mut(0).expect("forge").access = Some(fixtures::github_access());
    let component = Component::new(settings, &world.env.limits, &fixtures::shared()).expect("component");
    world.replace(component);
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([ACCEPTED]) });
    world.run_for(SECOND);
    world.call(CALL, REPO, Op::Read(Read::Items { since: Time::ZERO, page: 1, kind: Some(api::Kind::Issue) }));
    world.run_for(SECOND);
    let Ok(Answer::Items { items, .. }) = &world.answered(CALL).expect("items").result else { panic!("items answer") };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title.as_ref(), b"from the public domain");
    assert_eq!(world.observations.len(), 1);
    assert!(!world.busy());
    assert_eq!(world.connected, 2);
}

#[test]
fn github_capacity_facts_are_kept_for_the_owner() {
    let mut limits = fixtures::limits();
    limits.installations = 1;
    let mut world =
        Joint::github(SEED, 0, limits, fixtures::github_settings(), fixtures::shared(), joint::checks(1000));
    connect(&mut world, fixtures::github_access());
    world.run_for(SECOND);
    assert_eq!(world.connected, 1);
    assert!(matches!(
        world.facts.as_slice(),
        [Event::Fact { forge: FORGE, fact: skein_forge_connection::Fact::Unkept { installation: 2 } }]
    ));
}

#[test]
fn a_github_joint_renews_each_installations_token_before_it_expires() {
    let mut limits = fixtures::limits();
    limits.renew_before = Duration::from_secs(600);
    limits.log_poll = Duration::from_secs(5);
    let mut world =
        Joint::github(SEED, 0, limits, fixtures::github_settings(), fixtures::shared(), joint::checks(1000));
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([1, 2]) });
    world.action(Request::Connect { forge: FORGE, access: fixtures::github_access() });
    for _ in 0..1000 {
        world.run_for(Duration::from_nanos(1_000_000));
        if world.connected == 1 {
            break;
        }
    }
    assert_eq!(world.connected, 1);
    assert_eq!(world.git_tokens.len(), 2);
    let initial: Vec<_> = world
        .git_tokens
        .iter()
        .map(|event| {
            let Event::GitToken { forge, installation, account, token, expires } = event else {
                panic!("git token event")
            };
            assert_eq!(*forge, FORGE);
            assert!(matches!(
                (*installation, account.id, account.login.as_ref()),
                (1, 22, b"acme") | (2, 23, b"example")
            ));
            assert!(*expires > world.env.wall);
            (*installation, token.clone(), *expires)
        })
        .collect();
    let life = skein_fake_forge_github::installations::TOKEN_LIFE;
    world.run_for(Duration::from_nanos(life.as_nanos() - limits.renew_before.as_nanos() + SECOND.as_nanos()));
    assert_eq!(world.git_tokens.len(), 4);
    for (installation, token, expires) in initial {
        let renewed: Vec<_> = world
            .git_tokens
            .iter()
            .filter(|event| {
                matches!(event,
                    Event::GitToken { installation: id, .. } if *id == installation
                )
            })
            .collect();
        assert_eq!(renewed.len(), 2);
        let Event::GitToken { forge, account, token: next, expires: later, .. } = renewed[1] else {
            panic!("renewed git token")
        };
        assert_eq!(*forge, FORGE);
        assert_eq!(account.id, if installation == 1 { 22 } else { 23 });
        assert_eq!(account.login.as_ref(), if installation == 1 { b"acme".as_slice() } else { b"example" });
        assert!(next.expose() != token.expose(), "renewal mints a different token");
        assert!(*later > expires);
        assert!(world.env.wall < expires, "renewal completes before the old token expires");
    }
    assert!(world.stopped.is_empty());
    assert_eq!(world.connected, 1);
}

#[test]
fn the_joints_request_lines_are_the_same_after_many_polls() {
    const POLLS: u64 = 128;
    let mut limits = fixtures::limits();
    limits.log_poll = Duration::from_secs(5);
    let mut world =
        Joint::github(SEED, 0, limits, fixtures::github_settings(), fixtures::shared(), joint::checks(1000));
    world.action(Request::Installations { forge: FORGE, accepted: Box::from([1, 2]) });
    world.action(Request::Connect { forge: FORGE, access: fixtures::github_access() });
    world.run_for(SECOND);
    let mut expected: Vec<_> = [
        "GET /app/installations?per_page=100&page=1",
        "GET /app/installations?per_page=100&page=2",
        "POST /app/installations/1/access_tokens",
        "POST /app/installations/1/access_tokens",
        "POST /app/installations/2/access_tokens",
        "POST /app/installations/2/access_tokens",
        "POST /graphql",
    ]
    .map(|line| format!("{line} HTTP/1.1\r\n").into_bytes())
    .into();
    assert_eq!(world.request_lines(), expected);
    for poll in 1..=POLLS {
        world.run_for(limits.log_poll);
        expected.push(b"GET /app/hook/deliveries?per_page=8 HTTP/1.1\r\n".to_vec());
        assert_eq!(world.request_lines(), expected, "poll {poll}");
        assert_eq!(world.request_counts().written, poll);
        assert_eq!(world.request_counts().cut_in_head, 0);
        assert_eq!(world.tally().total, 7 + poll);
    }
    assert_eq!(world.start_counts().written, 7);
    assert_eq!(world.start_counts().served, 7);
    assert!(world.stopped.is_empty());
    assert_eq!(world.connected, 1);
}

#[test]
fn github_compare_reads_a_two_commit_change_through_the_fake() {
    use skein_fake_forge_domain::{self as fake, api as raw};
    let mut world = world(SEED, 0);
    connect(&mut world, fixtures::github_access());
    world.run_for(SECOND);
    let raw::Answer::Commit(base) =
        world.direct(raw::Op::Read(raw::Read::Branch { branch: Box::from(b"main".as_slice()) })).expect("base")
    else {
        panic!("base commit")
    };
    let mut head = base;
    let mut commits = Vec::new();
    for content in [b"first".as_slice(), b"repair"] {
        head = fake::commit(
            &mut world.domain,
            &world.fake_env.limits,
            head,
            Box::from([raw::File { path: Box::from(b"file".as_slice()), content: Box::from(content) }]),
            b"change",
        )
        .expect("commit")
        .expect("changed");
        commits.push(skein_fake_forge_protocol::routes::object(head).bytes);
    }
    world
        .direct(raw::Op::Git(raw::Git::Push { branch: Box::from(b"feature".as_slice()), commit: head, expected: None }))
        .expect("push");
    let before = skein_fake_forge_protocol::routes::object(base).bytes;
    let after = skein_fake_forge_protocol::routes::object(head).bytes;
    world.call(CALL, REPO, Op::Read(Read::Compare { before, after }));
    world.run_for(SECOND);
    let answer = world.answered(CALL).expect("one terminal");
    assert_eq!(
        answer.result,
        Ok(Answer::Compare {
            before,
            after,
            contains_before: true,
            commits: commits.into_boxed_slice(),
            files: Box::from([api::File {
                path: Box::from(b"file".as_slice()),
                before: None,
                after: Some(Box::from(b"repair".as_slice()))
            }])
        })
    );
    assert_eq!(answer.cost, 4, "resolve, compare and two file sides");
    assert_eq!(world.tally().routes.get(&skein_fake_forge_protocol::Route::Compare), Some(&1));
}
