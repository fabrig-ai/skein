//! Small client and component limits, and a creation with its answer lost.
use crate::World;
use skein_forge::api::Write;
use skein_forge_client::{Condition, Effect, Entry, effect_key};
use skein_forge_connection::{Limits, Settings, Shared};
use skein_forge_machine_world::{fixtures, joint::REPO};
use skein_lib::Duration;

pub const LIMITS: skein_forge_client::Limits = skein_forge_client_world::LIMITS;

#[derive(Clone, Copy)]
pub(crate) enum Shape {
    Plain,
    Github { log_poll: Duration },
    Hooked,
}

impl Shape {
    pub(crate) fn author(self) -> u64 {
        match self {
            Self::Plain | Self::Hooked => 1,
            Self::Github { .. } => crate::github_fixture::USER,
        }
    }

    pub(crate) fn limits(self) -> Limits {
        match self {
            Self::Plain => limits(),
            Self::Github { log_poll } => crate::github_fixture::limits(log_poll),
            Self::Hooked => Limits { repositories: 1, ..fixtures::hook_limits() },
        }
    }

    pub(crate) fn settings(self) -> Settings {
        let mut settings = match self {
            Self::Plain => settings(),
            Self::Github { .. } => return crate::github_fixture::settings(),
            Self::Hooked => fixtures::hook_settings(),
        };
        settings.forges.get_mut(0).expect("fixture forge").access.as_mut().expect("fixture access").user = 1;
        settings
    }
}

/// The world over the fake's signed deliveries to the component's listener.
#[must_use]
pub fn hooked(seed: u64) -> World {
    World::with(seed, 0, Shape::Hooked)
}

pub(crate) fn limits() -> Limits {
    Limits { repositories: 1, ..fixtures::limits() }
}

pub(crate) fn shared() -> Shared {
    Shared {
        calls: LIMITS.calls,
        rows: LIMITS.rows,
        answer_bytes: LIMITS.answer_bytes,
        op_bytes: LIMITS.op_bytes,
        lifetime: LIMITS.lifetime,
        repositories: 1,
        ..fixtures::shared()
    }
}

pub(crate) fn settings() -> Settings {
    let mut settings = fixtures::settings();
    settings.forges.get_mut(0).expect("fixture forge").access.as_mut().expect("fixture access").user = 1;
    settings
}

/// Stops with the creation's error terminal queued, before client delivery.
#[must_use]
pub fn lost_creation(seed: u64) -> World {
    let mut world = World::new(seed, 0);
    world.run_for(Duration::from_secs(1));
    let entry = Entry {
        number: 1,
        task: 1,
        repository: REPO,
        effect: Effect {
            write: Write::CreateIssue {
                key: effect_key(b"kit", b"lost", LIMITS.op_bytes).expect("fixture effect key"),
                title: Box::from(b"kit".as_slice()),
                body: Box::from(b"kit".as_slice()),
                labels: Box::new([]),
            },
            condition: Condition::None,
        },
        start: None,
        attempt: None,
        failures: 0,
        queued: false,
    };
    world.lose_creation_answer(entry);
    world
}

/// Bounded owner reads and durable writes through cuts, reconnect and restart.
#[must_use]
pub fn mixed_calls(seed: u64) -> World {
    use skein_forge::api::Read;
    use skein_forge_client::Event;
    use skein_lib::Time;

    let mut world = World::with(seed, 300, Shape::Hooked);
    world.startup();
    world.reconnect();
    let number = world.person_issue(b"mixed");
    for page in 1..=2 {
        let _ = world.read(Read::Items { since: Time::ZERO, page, kind: None });
    }
    world.event(Event::Make {
        entry: Entry {
            number: 1,
            task: 1,
            repository: REPO,
            effect: Effect { write: Write::Close { number }, condition: Condition::None },
            start: None,
            attempt: None,
            failures: 0,
            queued: false,
        },
    });
    world.checkpoint();
    world.run_for(Duration::from_secs(1));
    let _ = world.read(Read::Item { number, after: 0 });
    world
}

/// A durable owner decision supplies records to the credential referee.
pub fn github_close(world: &mut World, number: u64) {
    world.event(skein_forge_client::Event::Make {
        entry: Entry {
            number: 2,
            task: 1,
            repository: REPO,
            effect: Effect { write: Write::Close { number }, condition: Condition::None },
            start: None,
            attempt: None,
            failures: 0,
            queued: false,
        },
    });
    world.run_for(Duration::from_secs(20));
}
