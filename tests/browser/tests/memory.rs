//! Keep fixed scenarios under measured peak ceilings with a counting allocator.

use skein_browser::Limits;
use skein_browser::boundary::Request;
use skein_browser_world::world::{Fault, World};
use skein_heap::{Counting, Meter};
use skein_lib::Token;

#[global_allocator]
static HEAP: Counting = Counting;

pub fn compact_limits() -> Limits {
    Limits {
        persons: 2,
        pages: 2,
        ops: 8,
        commands: 32,
        message: 4096,
        command: 2048,
        matches: 8,
        text: 128,
        snapshot: 1024,
        screenshot: 2048,
        stderr: 128,
        ..Limits::default()
    }
}

// Whole-world peak for this scenario: 16,764 B measured, plus a quarter, rounded up
// to two significant digits.
const BROWSER_REPORTS_PEAK_CEILING: u64 = 21_000;

#[test]
fn reports_faults_and_terminals_stay_under_the_peak_ceiling() {
    let limits = compact_limits();
    let meter = Meter::new();
    meter.start();
    let mut world = World::with_limits(limits);
    world.start();
    world.open();
    world.ask(Request::Screenshot { page: Token::new(3), op: Token::new(4) });
    world.fault(b"Accessibility.getFullAXTree", Fault::Interleave);
    world.ask(Request::Snapshot { page: Token::new(3), op: Token::new(5) });
    world.disabled = true;
    world.ask(Request::Press { page: Token::new(3), op: Token::new(6), node: 7, at: None });
    let measured = meter.end();
    println!("{}: peak {} bytes", module_path!(), measured.peak());
    assert!(
        measured.peak() <= BROWSER_REPORTS_PEAK_CEILING,
        "peak {} exceeds ceiling {BROWSER_REPORTS_PEAK_CEILING}",
        measured.peak()
    );
    drop(world);
    assert_eq!(meter.held(), 0, "all storage is released");
}

// Whole-world peak for this scenario: 33,290 B measured, plus a quarter, rounded up
// to two significant digits.
const BROWSER_ATTRIBUTE_PEAK_CEILING: u64 = 42_000;

#[test]
fn selector_and_attribute_storage_stays_under_the_peak_ceiling() {
    let limits = Limits {
        text: 2048,
        message: 16384,
        command: 16384,
        matches: 0,
        snapshot: 0,
        screenshot: 0,
        ..compact_limits()
    };
    let meter = Meter::new();
    meter.start();
    let mut world = World::with_limits(limits);
    world.start();
    world.open();
    world.attributes = format!("[\"value\",\"{}\"]", "x".repeat(2048));
    world.ask(Request::Attribute {
        page: Token::new(3),
        op: Token::new(4),
        selector: vec![b'x'; 2048].into(),
        name: b"value".as_slice().into(),
    });
    assert!(
        matches!(world.above.pop(), Some(skein_browser::boundary::Event::Attribute { read: skein_browser::boundary::Read::Value(value), .. }) if value.len() == 2048)
    );
    let measured = meter.end();
    println!("{}: peak {} bytes", module_path!(), measured.peak());
    assert!(
        measured.peak() <= BROWSER_ATTRIBUTE_PEAK_CEILING,
        "peak {} exceeds ceiling {BROWSER_ATTRIBUTE_PEAK_CEILING}",
        measured.peak()
    );
    drop(world);
    assert_eq!(meter.held(), 0);
}

#[path = "support/look_memory.rs"]
mod look_memory;
// Whole-world peak for this scenario: 83,602 B measured, plus a quarter, rounded up
// to two significant digits.
const BROWSER_LOOK_PEAK_CEILING: u64 = 110_000;

#[test]
fn look_requests_and_answers_at_their_limits_stay_under_the_peak_ceiling() {
    let limits = Limits { entries: 16, properties: 8, message: 32768, command: 4096, ..compact_limits() };
    let meter = Meter::new();
    meter.start();
    let mut world = World::with_limits(limits);
    world.start();
    world.open();
    look_memory::drive(&mut world, limits);
    let measured = meter.end();
    println!("{}: peak {} bytes", module_path!(), measured.peak());
    assert!(
        measured.peak() <= BROWSER_LOOK_PEAK_CEILING,
        "peak {} exceeds ceiling {BROWSER_LOOK_PEAK_CEILING}",
        measured.peak()
    );
    drop(world);
    assert_eq!(meter.held(), 0);
}

// Whole-world peak for this scenario: 16,260 B measured, plus a quarter, rounded up
// to two significant digits.
const BROWSER_DOWNLOAD_PEAK_CEILING: u64 = 21_000;

#[test]
fn downloads_at_the_inflight_limit_stay_under_the_peak_ceiling() {
    let limits = compact_limits();
    let meter = Meter::new();
    meter.start();
    let mut world = World::with_limits(limits);
    world.start();
    world.open_with_downloads(Some(b"/downloads".as_slice().into()));
    for index in 0..limits.downloads {
        world.begin_download(&format!("guid-{index}"), &"x".repeat(usize::try_from(limits.text).unwrap()));
    }
    assert!(world.above.is_empty(), "all downloads at the bound remain in flight");
    let measured = meter.end();
    println!("{}: peak {} bytes", module_path!(), measured.peak());
    assert!(
        measured.peak() <= BROWSER_DOWNLOAD_PEAK_CEILING,
        "peak {} exceeds ceiling {BROWSER_DOWNLOAD_PEAK_CEILING}",
        measured.peak()
    );
    drop(world);
    assert_eq!(meter.held(), 0);
}
