pub mod fixtures;
use skein_forge::api::{Answer, Error, Read, Write};
use skein_forge_client::{Condition, Effect, Entry, Made, Outcome};
use skein_forge_client_world::{REPO, Settings, World};
const FINDING: &[u8] = b"finding";
const SEVERITY: &[u8] = b"severity:high";
const ISSUE: u64 = 1;
const READER: u64 = 1;
// Calm calls settle within the three seconds used throughout these stories.
const SETTLE_SECONDS: u64 = 3;
fn entry(number: u64, write: Write) -> Entry {
    Entry {
        number,
        task: 7,
        repository: REPO,
        effect: Effect { write, condition: Condition::None },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    }
}
fn definition(number: u64, name: &[u8]) -> Entry {
    entry(number, Write::CreateLabel { name: Box::from(name), color: [0x12, 0xab, 0xef] })
}
fn with_labels() -> World {
    let mut world = World::new(Settings::calm(31));
    world.make(definition(1, FINDING));
    world.make(definition(2, SEVERITY));
    world.run_for(SETTLE_SECONDS);
    world
}
fn labeled_issue() -> Entry {
    fixtures::issue_with_labels(3, Box::new([Box::from(FINDING), Box::from(SEVERITY)]))
}

fn read_issue(world: &mut World) {
    world.read(READER, Read::Item { number: ISSUE, after: 0 });
    world.run_for(SETTLE_SECONDS);
}
#[test]
fn an_issue_is_created_with_its_labels() {
    let mut world = with_labels();
    world.make(labeled_issue());
    world.run_for(SETTLE_SECONDS);
    read_issue(&mut world);
    let Some(Ok(Answer::Item { item, .. })) = world.read_result(READER) else { panic!("issue read") };
    assert_eq!(item.labels.as_ref(), &[Box::from(FINDING), Box::from(SEVERITY)]);
    world.finish();
}
#[test]
fn an_issues_labels_are_replaced_by_name() {
    let mut world = with_labels();
    world.make(labeled_issue());
    world.run_for(SETTLE_SECONDS);
    world.make(entry(4, Write::Labels { number: ISSUE, labels: Box::new([Box::from(FINDING)]) }));
    world.run_for(SETTLE_SECONDS);
    read_issue(&mut world);
    let Some(Ok(Answer::Item { item, .. })) = world.read_result(READER) else { panic!("issue read") };
    assert_eq!(item.labels.as_ref(), &[Box::from(FINDING)]);
    world.finish();
}
#[test]
fn a_label_that_exists_is_found_not_made_again() {
    let mut world = with_labels();
    world.make(definition(3, FINDING));
    world.run_for(SETTLE_SECONDS);
    assert!(world.outcomes().contains(&(3, Outcome::Made { made: Made::Set, found: true })));
    world.finish();
}
#[test]
fn an_unknown_label_is_missing_before_any_write() {
    let mut world = World::new(Settings::calm(31));
    world.make(labeled_issue());
    world.run_for(SETTLE_SECONDS);
    assert!(world.outcomes().contains(&(3, Outcome::Failed(Error::Missing))));
    read_issue(&mut world);
    assert_eq!(world.read_result(READER), Some(&Err(Error::Missing)));
    world.finish();
}

#[test]
fn an_uncertain_label_is_found_by_its_name() {
    // A one-second delayed copy has landed before the three-second restart.
    const LATE_SECONDS: u64 = 1;
    let mut world = World::new(Settings::calm(31));
    world.land_writes_late(skein_lib::Duration::from_secs(LATE_SECONDS));
    world.make(definition(1, FINDING));
    world.run_for(SETTLE_SECONDS);
    world.calm();
    world.restart();
    world.run_for(SETTLE_SECONDS);
    assert!(world.outcomes().contains(&(1, Outcome::Made { made: Made::Set, found: true })));
    world.finish();
}
