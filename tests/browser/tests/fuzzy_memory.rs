//! Seeded loads at varied limits, checked against measured peak ceilings.

use skein_browser::Limits;
use skein_browser::boundary::Request;
use skein_browser_world::world::{Fault, World};
use skein_heap::{Counting, Meter};
use skein_lib::{Time, Token};

#[global_allocator]
static HEAP: Counting = Counting;

// Whole-world ceiling across seeds 0..96, using the largest measured peak
// (16,987 B), plus a quarter rounded up to two significant digits.
const BROWSER_SEEDED_LOAD_PEAK_CEILING: u64 = 22_000;

#[test]
fn random_load_stays_under_the_peak_ceiling() {
    for seed in 0..96_u64 {
        let limits = Limits {
            persons: 2,
            pages: 2,
            ops: 4 + u32::try_from(seed % 8).expect("ops"),
            commands: 24,
            message: 1024 + u32::try_from(seed % 5).expect("message") * 256,
            command: 2048,
            matches: 4 + u32::try_from(seed % 8).expect("matches"),
            text: 64 + u32::try_from(seed % 4).expect("text") * 32,
            snapshot: 512,
            screenshot: 1024,
            stderr: 64,
            ..Limits::default()
        };
        let meter = Meter::new();
        meter.start();
        let mut world = World::with_limits(limits);
        world.start();
        world.open();
        world.fault(
            b"Accessibility.getFullAXTree",
            match seed % 4 {
                0 => Fault::Drop,
                1 => Fault::Delay,
                2 => Fault::Error,
                _ => Fault::Interleave,
            },
        );
        world.ask(Request::Snapshot { page: Token::new(3), op: Token::new(4) });
        if seed % 4 <= 1 {
            world.tick(Time::from_nanos(limits.answer.as_nanos()));
            world.release_delayed();
        }
        let measured = meter.end();
        println!("{}: seed {seed}: peak {} bytes", module_path!(), measured.peak());
        assert!(
            measured.peak() <= BROWSER_SEEDED_LOAD_PEAK_CEILING,
            "seed {seed}: peak {} exceeds ceiling {BROWSER_SEEDED_LOAD_PEAK_CEILING}",
            measured.peak()
        );
        drop(world);
        assert_eq!(meter.held(), 0, "seed {seed}: all storage released");
    }
}

#[path = "support/look_memory.rs"]
mod look_memory;
// Whole-world ceiling across seeds 0..48, using the largest measured peak
// (58,372 B), plus a quarter rounded up to two significant digits.
const BROWSER_SEEDED_LOOK_PEAK_CEILING: u64 = 73_000;

#[test]
fn seeded_look_answers_reach_each_limit_under_the_peak_ceiling() {
    for seed in 0..48_u32 {
        let limits = Limits {
            persons: 1,
            pages: 1,
            ops: 3,
            commands: 16,
            message: 32768,
            command: 4096,
            matches: 0,
            text: 32 + seed % 4 * 16,
            snapshot: 0,
            screenshot: 128 + seed * 8,
            entries: 4 + seed % 16,
            properties: 1 + seed % 8,
            stderr: 0,
            ..Limits::default()
        };
        let meter = Meter::new();
        meter.start();
        let mut world = World::with_limits(limits);
        world.start();
        world.open();
        look_memory::drive(&mut world, limits);
        let measured = meter.end();
        println!("{}: seed {seed}: peak {} bytes", module_path!(), measured.peak());
        assert!(
            measured.peak() <= BROWSER_SEEDED_LOOK_PEAK_CEILING,
            "seed {seed}: peak {} exceeds ceiling {BROWSER_SEEDED_LOOK_PEAK_CEILING}",
            measured.peak()
        );
        drop(world);
        assert_eq!(meter.held(), 0, "seed {seed}");
    }
}
