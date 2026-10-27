pub mod fixtures;
use skein_forge_client::{Made, Outcome, Resource, Watch, What};
use skein_forge_client_world::{REPO, Settings, World};

#[test]
fn an_uncertain_branch_is_found_without_retry_after_restart() {
    let mut world = World::new(Settings::calm(41));
    let head = world.branch(b"main").expect("fixture main branch");
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(fixtures::branch_entry(head));
    world.run_for(1);
    let first = world.entry(1).expect("attempt retained").attempt.expect("write was sent");
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.branch(b"kit/7"), None);
    world.calm();
    world.restart();
    world.run_for(8);
    assert_eq!(world.entry(1).expect("deadline retained").attempt, Some(first));
    assert_eq!(world.stats().writes, 1, "restart cannot renew the first deadline or send early");
    world.run_for(5);
    assert_eq!(world.stats().writes, 1, "a found branch is never sent again");
    assert_eq!(world.branch(b"kit/7"), Some(head));
    world.run_for(10);
    assert_eq!(world.stats().late_landings, 1);
    assert_eq!(world.branch(b"kit/7"), Some(head));
    assert!(
        world
            .outcomes()
            .iter()
            .any(|(entry, outcome)| { *entry == 1 && matches!(outcome, Outcome::Made { made: Made::Branch(_), .. }) })
    );
    world.finish();
}

#[test]
fn a_deleted_uncertain_branch_is_held_after_its_deadline() {
    let (mut world, _, _) = fixtures::held_branch();
    assert_eq!(world.stats().writes, 1);
    assert!(world.outcomes().contains(&(1, Outcome::Held)));
    world.finish();
}

#[test]
fn restart_reads_afresh_before_settling_the_outbox() {
    let mut world = World::new(Settings::calm(41));
    let resource = Resource { repository: REPO, what: What::Branch(Box::from(&b"main"[..])) };
    world.keep(Box::new([Watch { resource: resource.clone(), participating: false }]));
    world.writer(resource.clone(), true);
    world.run_for(1);
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    world.make(fixtures::issue(1));
    world.run_for(1);
    let attempt = world.entry(1).expect("attempt retained").attempt.expect("write was sent");
    assert_eq!(world.stats().writes, 1);

    world.calm();
    world.restart();
    world.run_for(1);
    assert_eq!(world.recovery_completions(), (1, 1));
    assert_eq!(world.entry(1).expect("attempt retained while waiting").attempt, Some(attempt));
    assert_eq!(world.stats().writes, 1);
    world.run_for(30);
    assert_eq!(world.recovery_completions(), (1, 1));
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.stats().creations, 1);
    world.writer(resource, false);
    world.finish();
}

#[test]
fn a_held_branch_is_written_again_only_after_remake() {
    let (mut world, entry, head) = fixtures::held_branch();
    world.run_for(30);
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.outcomes().last(), Some(&(entry.number, Outcome::Held)));
    world.restart();
    world.run_for(30);
    assert_eq!(world.stats().writes, 1);
    let outcomes = world.outcomes().len();
    world.remake(entry.clone());
    world.run_for(30);
    assert_eq!(world.stats().writes, 2);
    assert_eq!(world.branch(b"kit/7"), Some(head));
    assert_eq!(
        &world.outcomes()[outcomes..],
        &[(
            entry.number,
            Outcome::Made { made: Made::Branch(skein_forge_client_world::translate::commit(head)), found: false }
        )]
    );
    world.finish();
}

#[test]
fn a_writer_given_at_runtime_is_kept_across_restart() {
    let mut world = World::new(Settings { writer: false, ..Settings::calm(41) });
    world.make(fixtures::issue(1));
    assert!(world.outcomes().contains(&(1, Outcome::Failed(skein_forge::api::Error::Forbidden))));
    assert!(world.give_writer(1));
    let mut entry = fixtures::issue(2);
    let skein_forge::api::Write::CreateIssue { key, .. } = &mut entry.effect.write else {
        panic!("issue fixture creates an issue");
    };
    *key = Box::from(&b"\x01\x00\x03kitdeployment:gate"[..]);
    world.make(entry);
    assert!(world.entry(2).expect("entry committed before restart").attempt.is_none());
    world.restart();
    world.run_for(30);
    assert!(
        world
            .outcomes()
            .iter()
            .any(|(entry, outcome)| { *entry == 2 && matches!(outcome, Outcome::Made { made: Made::Created(_), .. }) })
    );
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.stats().creations, 1);
    world.finish();
}

use skein_fake_forge_domain::api as raw;
use skein_forge::api::{self, Answer, Read, Write};
use skein_lib::Duration;

fn dispatch_world() -> World {
    World::with_workflows(
        Settings::calm(47),
        Box::new([raw::Workflow {
            file: Box::from(b"gate.yml".as_slice()),
            jobs: Box::new([Box::from(b"build".as_slice())]),
            latency_min: Duration::from_secs(1),
            latency_max: Duration::from_secs(1),
            passes: 1000,
            silent: 0,
            cue: None,
        }]),
    )
}
fn dispatch_entry() -> skein_forge_client::Entry {
    let mut entry = fixtures::issue(1);
    entry.effect.write = Write::Dispatch {
        workflow: Box::from(b"gate.yml".as_slice()),
        branch: Box::from(b"main".as_slice()),
        inputs: Box::new([api::Input { name: Box::from(b"select".as_slice()), value: Box::from(b"all".as_slice()) }]),
    };
    entry
}
fn dispatched(world: &mut World) -> api::Run {
    let head = world.branch(b"main").expect("main");
    world.read(
        100,
        Read::Runs {
            workflow: Box::from(b"gate.yml".as_slice()),
            commit: skein_forge_client_world::translate::commit(head),
            page: 1,
        },
    );
    world.run_for(1);
    let Some(Ok(Answer::Runs { runs, more: false })) = world.read_result(100) else { panic!("dispatch found") };
    assert_eq!(runs.len(), 1, "no duplicate dispatch");
    let id = runs[0].id;
    world.read(101, Read::Run { run: id });
    world.run_for(1);
    let Some(Ok(Answer::Run(run))) = world.read_result(101) else { panic!("run detail") };
    run.clone()
}
#[test]
fn a_dispatch_whose_answer_is_lost_is_found_by_its_run() {
    let mut world = dispatch_world();
    world.land_writes_late(Duration::from_secs(5));
    world.make(dispatch_entry());
    world.run_for(1);
    assert_eq!(world.stats().writes, 1);
    world.calm();
    world.restart();
    world.run_for(12);
    assert_eq!(world.outcomes().last(), Some(&(1, Outcome::Made { made: Made::Set, found: true })));
    let run = dispatched(&mut world);
    assert_eq!((run.event, run.actor), (api::Event::Dispatch, 1));
    assert_eq!(world.stats().writes, 1);
    assert_eq!(world.stats().late_landings, 1);
    world.finish();
}
#[test]
fn a_rerun_whose_answer_is_lost_is_found_by_its_next_attempt() {
    let mut world = dispatch_world();
    world.make(dispatch_entry());
    world.run_for(3);
    let run = dispatched(&mut world);
    let job = &run.jobs[0];
    let attempt = api::JobAttempt { head: run.commit, run: run.id, job: job.id, attempt: job.attempt };
    let mut entry = fixtures::issue(2);
    entry.effect.write = Write::Rerun { attempt };
    world.land_writes_late(Duration::from_secs(5));
    world.make(entry);
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(12);
    assert_eq!(world.outcomes().last(), Some(&(2, Outcome::Made { made: Made::Set, found: true })));
    let rerun = dispatched(&mut world);
    assert_eq!(
        (rerun.id, rerun.commit, rerun.jobs[0].id, rerun.jobs[0].attempt),
        (run.id, run.commit, job.id, job.attempt + 1)
    );
    assert_eq!(world.stats().writes, 2, "one dispatch and one rerun");
    assert_eq!(world.stats().late_landings, 1);
    world.finish();
}
