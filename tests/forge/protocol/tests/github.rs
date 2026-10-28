use skein_forge::api::{Answer, Error, Read, State, Write};
use skein_forge_client::Outcome;
use skein_forge_protocol_world::{World, fixtures};
use skein_lib::{Duration, Time};
use std::collections::BTreeSet;

fn invariants(world: &mut World) {
    world.finish();
    let (calls, answers) = world.calls_and_answers();
    assert!(calls > 0);
    assert_eq!(calls, answers);
    assert_eq!(world.owner_counts(), (0, 0));
    let a = world.accounting();
    assert_eq!(a.reported, a.written, "{a:?}");
    assert_eq!(a.served + a.cut_in_head, a.written, "{a:?}");
    assert!(world.secrets_absent());
    let (keys, jwts, tokens) = world.github_credentials();
    assert!(keys > 0 && jwts > 0 && tokens > 0);
}

#[test]
fn a_github_issue_creation_is_refused_without_posting() {
    let mut world = World::github(23, 0);
    assert!(matches!(world.read(Read::Items { since: Time::ZERO, page: 1, kind: None }), Ok(Answer::Items { .. })));
    let written = world.accounting().written;
    assert_eq!(
        world.write(Write::CreateIssue {
            key: skein_forge_client::effect_key(b"kit", b"github-refused", fixtures::LIMITS.op_bytes)
                .expect("effect key"),
            title: Box::from(b"kit".as_slice()),
            body: Box::from(b"kit".as_slice()),
            labels: Box::new([]),
        }),
        Outcome::Failed(Error::Refused)
    );
    // The client's clock read is the sole request; refusal sends no POST.
    assert_eq!(world.accounting().written, written + 1);
    assert_eq!(world.creations(), 0);
    // The readiness read takes owner number one; the write takes number two.
    assert!(world.attempt(2).is_some());
    assert_eq!(world.released_writes(), 1);
    world.restart();
    invariants(&mut world);
    assert!(!world.made(2));
    assert_eq!(world.creations(), 0);
}

#[test]
fn github_listings_follow_another_partys_changes_across_pages() {
    let mut world = World::github(5, 0);
    world.run_for(Duration::from_secs(1));
    let a = world.person_issue(b"A");
    let b = world.person_issue(b"B");
    let c = world.person_issue(b"C");
    world.person_edit(b, b"B2");
    world.person_close(c);
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let mut page = 1;
    loop {
        let Answer::Items { items, more, .. } =
            world.read(Read::Items { since: Time::ZERO, page, kind: None }).expect("listing")
        else {
            panic!("items")
        };
        for item in items {
            assert!(seen.insert(item.number));
            rows.push((item.number, item.title, item.state == State::Open));
        }
        if !more {
            break;
        }
        page += 1;
    }
    assert!(page > 1);
    rows.sort_by_key(|row| row.0);
    assert_eq!(rows, world.snapshot());
    assert_eq!(
        rows,
        vec![
            (a, Box::from(b"A".as_slice()), true),
            (b, Box::from(b"B2".as_slice()), true),
            (c, Box::from(b"C".as_slice()), false)
        ]
    );
    fixtures::github_close(&mut world, a);
    invariants(&mut world);
}

#[test]
fn github_delivery_log_reaches_owner_before_client_polling() {
    use skein_forge_client::{Event, Resource, Watch, What};
    use skein_forge_machine_world::joint::REPO;
    use skein_lib::Token;

    // Baseline after the fake's 2.2-second visibility delay.
    const LOG_CHANGE: Duration = Duration::from_secs(4);
    const KEEP_READ: Duration = Duration::from_millis(100);
    // Visibility (2.2s), the log poll (1s) and the client's hinted delay (1s)
    // fit in 4.5s. Short turns route the hint while its read can still finish.
    const HINT_TURNS: u64 = 45;
    const TURN: Duration = Duration::from_millis(100);

    let mut world = World::github_with_log_poll(7, 0, Duration::from_secs(1));
    world.run_for(Duration::from_secs(1));
    let number = world.person_issue(b"before");
    world.run_for(LOG_CHANGE);
    let resource = Resource { repository: REPO, what: What::Issue(number) };
    world.event(Event::Keep {
        owner: Token::new(1),
        watches: Box::new([Watch { resource: resource.clone(), participating: false }]),
    });
    world.run_for(KEEP_READ);
    let Some(Answer::Item { item, .. }) = world.changed(&resource) else { panic!("kept issue read") };
    assert_eq!(item.title.as_ref(), b"before");
    let before = world.hints();
    world.person_edit(number, b"after");
    assert!(KEEP_READ.saturating_add(TURN.saturating_mul(HINT_TURNS)) < fixtures::LIMITS.poll);
    for _ in 0..HINT_TURNS {
        world.run_for(TURN);
    }
    let Some(Answer::Item { item, .. }) = world.changed(&resource) else { panic!("hinted issue read") };
    assert_eq!(world.hints(), before + 1, "log delivery reached the client");
    assert_eq!(item.title.as_ref(), b"after");
    world.event(Event::Keep { owner: Token::new(1), watches: Box::from([]) });
    invariants(&mut world);
}

#[test]
fn github_watched_absent_branch_resolves_repository_once() {
    use skein_forge_client::{Event, Resource, Watch, What};
    use skein_forge_machine_world::joint::REPO;
    use skein_lib::Token;

    let mut world = World::github(71, 0);
    let branch = b"never-pushed";
    // The repository's listing establishes its row before the branch joins.
    world.event(Event::Keep {
        owner: Token::new(1),
        watches: Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Repository },
            participating: false,
        }]),
    });
    world.run_for(Duration::from_secs(1));
    world.event(Event::Keep {
        owner: Token::new(1),
        watches: Box::new([Watch {
            resource: Resource { repository: REPO, what: What::Branch(Box::from(branch.as_slice())) },
            participating: false,
        }]),
    });
    // Three slow rests cover stable absence while repository listings continue.
    world.run_for(fixtures::LIMITS.slow.saturating_mul(3).saturating_add(Duration::from_secs(2)));
    world.event(Event::Keep { owner: Token::new(2), watches: Box::new([]) });
    invariants(&mut world);
    assert_eq!(world.github_branch_reads(branch), 4);
    assert_eq!(world.github_repository_reads(), 1);
}
