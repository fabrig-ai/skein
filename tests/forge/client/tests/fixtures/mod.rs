use skein_forge::api;
use skein_forge_client::{Condition, Effect, Entry};
use skein_forge_client_world::{REPO, Settings, World};

#[must_use]
pub fn issue(entry: u64) -> Entry {
    issue_with_labels(entry, Box::new([]))
}
#[must_use]
pub fn issue_with_labels(entry: u64, labels: Box<[Box<[u8]>]>) -> Entry {
    Entry {
        number: entry,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::CreateIssue {
                key: Box::from(&b"\x01\x00\x03kitdeployment:goal"[..]),
                title: Box::from(&b"goal"[..]),
                body: Box::from(&b"plan"[..]),
                labels,
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    }
}

#[must_use]
pub fn branch_entry(head: u64) -> Entry {
    Entry {
        number: 1,
        task: 7,
        repository: REPO,
        effect: Effect {
            write: api::Write::CreateBranch {
                branch: Box::from(&b"kit/7"[..]),
                commit: skein_forge_client_world::translate::commit(head),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    }
}

#[must_use]
pub fn held_branch() -> (World, Entry, u64) {
    let mut world = World::new(Settings::calm(50));
    let head = world.branch(b"main").expect("fixture main branch");
    world.land_writes_late(skein_lib::Duration::from_secs(5));
    let entry = branch_entry(head);
    world.make(entry.clone());
    world.run_for(1);
    world.calm();
    world.restart();
    world.run_for(6);
    assert_eq!(world.branch(b"kit/7"), Some(head));
    world.outside_delete_branch(b"kit/7");
    world.run_for(6);
    (world, entry, head)
}

#[must_use]
pub fn merge_entry(pull: u64, head: u64) -> Entry {
    Entry {
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
    }
}
