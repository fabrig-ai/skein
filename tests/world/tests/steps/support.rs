//! The same bare loop paced whole or in seeded slices.
use skein_lib::{Rng, Time};
use skein_sim::Config;
use skein_sim_world::{Digests, Finished, World};
use skein_world_tests::{Script, client, ms, server};

pub fn exchange(seed: u64) -> World<Script> {
    let mut world = World::new(seed, Config::calm());
    world.digests(Digests::Traced);
    world.spawn(server);
    world.spawn(|| client(ms(10), ms(20)));
    world
}

/// Move an idle clock in one jump or in seeded pieces, without stepping early.
pub fn advance(world: &mut World<Script>, until: Time, pacing: &mut Option<Rng>) {
    while world.now() < until {
        let to = match pacing {
            Some(random) => {
                let remaining = until.as_nanos() - world.now().as_nanos();
                let piece = 1 + random.next_u64() % remaining;
                Time::from_nanos(world.now().as_nanos() + piece)
            }
            None => until,
        };
        world.advance(to);
    }
}

/// One loop; its caller chooses only the pace, which never adds a step.
pub fn run(mut world: World<Script>, mut pacing: Option<Rng>) -> Finished<Script> {
    loop {
        let slice = pacing.as_mut().map_or(1, |random| 1 + random.next_u64() % 9);
        for _ in 0..slice {
            world.step();
            if !world.busy() {
                break;
            }
        }
        if world.busy() {
            continue;
        }
        if world.settled() {
            return world.finish();
        }
        let due = world.next_due().expect("the scripts have a next instant");
        advance(&mut world, due, &mut pacing);
    }
}

pub fn compare(seed: u64) {
    let whole = run(exchange(seed), None);
    let sliced = run(exchange(seed), Some(Rng::new(seed ^ 0x5a17)));
    assert_eq!(whole.trace, sliced.trace, "seed {seed}: pace preserves every trace entry");
    assert_eq!(whole.steps, sliced.steps, "seed {seed}: pace adds no step");
    assert_eq!(whole.end, ms(20));
    assert_eq!(whole.procs[0].received[0].1.as_ref(), b"hello");
}
