use super::fixtures::{Call, FakePlatform};
use crate::{
    platform::{
        Platform,
        worker::{Clock, Memory, Speed, WorldClock, navigable},
    },
    store::{Record, Refused, Store},
};
use skein_lib::{Rng, Time, Wall};

const WORLD_START: u64 = 100;
const BROWSER_START: u64 = 1_000;
const WALL_START: u64 = 10_000;
const ELAPSED: u64 = 7;

fn instant(nanos: u64) -> Time {
    Time::from_nanos(nanos)
}
fn report(idle: bool, due: Option<u64>) -> Clock {
    Clock { step: 0, now: instant(WORLD_START), wall: Wall::from_nanos(WALL_START), idle, due: due.map(instant) }
}
fn clock(speed: u32) -> WorldClock {
    WorldClock::new(report(true, None), instant(BROWSER_START), Speed(speed))
}

#[test]
fn an_idle_world_moves_at_its_speed() {
    for speed in [1, 10] {
        let mut clock = clock(speed);
        let expected = WORLD_START + ELAPSED * u64::from(speed);
        assert_eq!(clock.now(instant(BROWSER_START + ELAPSED)).0, instant(expected));
    }
}

#[test]
fn an_idle_world_stops_at_its_next_due_instant() {
    const DUE: u64 = WORLD_START + 5;
    let mut clock = WorldClock::new(report(true, Some(DUE)), instant(BROWSER_START), Speed(10));
    assert_eq!(clock.now(instant(BROWSER_START)).0, instant(WORLD_START));
    assert_eq!(clock.now(instant(BROWSER_START + ELAPSED)).0, instant(DUE));
    assert_eq!(clock.now(instant(BROWSER_START + 2 * ELAPSED)).0, instant(DUE));
}

#[test]
fn a_stepping_world_holds_its_clock() {
    let mut clock = WorldClock::new(report(false, None), instant(BROWSER_START), Speed(10));
    assert_eq!(clock.now(instant(BROWSER_START + ELAPSED)).0, instant(WORLD_START));
}

#[test]
fn a_paused_world_holds_its_clock() {
    let mut clock = clock(10);
    let paused_at = instant(BROWSER_START + ELAPSED);
    let paused_now = clock.now(paused_at);
    clock.pace(Speed(0), paused_at);
    assert_eq!(clock.now(instant(BROWSER_START + 2 * ELAPSED)), paused_now);
}

#[test]
fn the_clock_never_goes_back() {
    let mut clock = clock(10);
    let at = instant(BROWSER_START + ELAPSED);
    let given = clock.now(at).0;
    // A stepping report and an idle report both lag behind the client's projection.
    clock.report(report(false, None), at);
    assert_eq!(clock.now(at).0, given);
    clock.report(report(true, Some(WORLD_START + 1)), at);
    assert_eq!(clock.now(instant(BROWSER_START + 2 * ELAPSED)).0, given);
    clock.report(report(true, None), at);
    assert_eq!(clock.now(instant(BROWSER_START + 2 * ELAPSED - 1)).0, given);
    assert_eq!(clock.now(instant(BROWSER_START + 2 * ELAPSED + 1)).0, instant(given.as_nanos() + 10));
}

#[test]
fn a_pace_change_keeps_the_instant() {
    let mut clock = clock(1);
    let mut at = BROWSER_START + ELAPSED;
    for speed in [10, 0, 2, 1] {
        let before = clock.now(instant(at));
        clock.pace(Speed(speed), instant(at));
        assert_eq!(clock.now(instant(at)), before);
        at += ELAPSED;
        assert_eq!(clock.now(instant(at)).0, instant(before.0.as_nanos() + ELAPSED * u64::from(speed)));
    }
}

#[test]
fn the_wall_follows_the_instant() {
    let mut clock = clock(10);
    let at = instant(BROWSER_START + ELAPSED);
    let expected_wall = Wall::from_nanos(WALL_START + ELAPSED * 10);
    assert_eq!(clock.now(at).1, expected_wall);
    clock.report(report(false, None), at);
    assert_eq!(clock.now(at).1, expected_wall);
    let next = Clock { now: instant(200), wall: Wall::from_nanos(20_000), ..report(true, None) };
    clock.report(next, at);
    assert_eq!(clock.now(instant(BROWSER_START + ELAPSED + 1)), (instant(210), Wall::from_nanos(20_010)));
}

#[test]
fn the_timer_fires_where_the_clock_reaches_its_deadline() {
    const DISTANCE: u64 = 21; // Not divisible by speed ten: the inverse must round up.
    for speed in [1, 10] {
        let mut clock = clock(speed);
        let deadline = instant(WORLD_START + DISTANCE);
        let expected = BROWSER_START + DISTANCE.div_ceil(u64::from(speed));
        assert_eq!(clock.timer(deadline), Some(instant(expected)));
        assert!(clock.now(instant(expected - 1)).0 < deadline);
        assert!(clock.now(instant(expected)).0 >= deadline);
    }
    let mut clock = clock(10);
    assert_eq!(clock.timer(instant(WORLD_START - 1)), Some(instant(BROWSER_START)));
    let at = instant(BROWSER_START + ELAPSED);
    clock.pace(Speed(2), at);
    let reached = WORLD_START + ELAPSED * 10;
    assert_eq!(clock.timer(instant(reached)), Some(at));
    assert_eq!(clock.timer(instant(reached + 3)), Some(instant(at.as_nanos() + 2)));
    // A lagging report must catch up before the timer's deadline can be reached.
    clock.report(report(true, None), at);
    assert_eq!(clock.timer(instant(reached + 2)), Some(instant(at.as_nanos() + (reached + 2 - WORLD_START) / 2)));
}

#[test]
fn no_timer_while_paused_stepping_or_past_due() {
    const DUE: u64 = WORLD_START + 5;
    let deadline = instant(DUE + 1);
    for (report, speed) in [(report(true, None), 0), (report(false, None), 10), (report(true, Some(DUE)), 10)] {
        let clock = WorldClock::new(report, instant(BROWSER_START), Speed(speed));
        assert_eq!(clock.timer(deadline), None);
    }
    let mut bounded = WorldClock::new(report(true, Some(DUE)), instant(BROWSER_START), Speed(10));
    let timer = bounded.timer(instant(DUE)).expect("due is reachable");
    assert_eq!(bounded.now(timer).0, instant(DUE));
    for clock in [clock(0), WorldClock::new(report(false, None), instant(BROWSER_START), Speed(1))] {
        assert_eq!(clock.timer(instant(WORLD_START)), None);
    }
}

fn drawn_paces(seed: u64) -> Vec<(Time, Wall)> {
    const OPERATIONS: usize = 256;
    const MAX_BROWSER_ADVANCE: u64 = 20;
    const MAX_WORLD_ADVANCE: u64 = 10;
    const MAX_DUE_DISTANCE: u64 = 200;
    let mut rng = Rng::new(seed);
    let mut report = report(true, None);
    let mut clock = clock(1);
    let mut browser = BROWSER_START;
    let mut last = instant(WORLD_START);
    let mut readings = Vec::new();
    for step in 0..OPERATIONS {
        browser += rng.between(0, MAX_BROWSER_ADVANCE);
        match rng.below(3) {
            0 => {
                // Reports model a world that has caught up to the client's last read.
                report.now = instant(last.as_nanos() + rng.between(0, MAX_WORLD_ADVANCE));
                report.wall = Wall::from_nanos(WALL_START + report.now.as_nanos() - WORLD_START);
                report.step += 1;
                report.idle = rng.chance(500);
                report.due = if rng.chance(750) {
                    Some(instant(report.now.as_nanos() + rng.between(0, MAX_DUE_DISTANCE)))
                } else {
                    None
                };
                clock.report(report, instant(browser));
            }
            1 => {
                let pace = u32::try_from(rng.between(0, 100)).expect("drawn speed fits");
                clock.pace(Speed(pace), instant(browser));
            }
            2 => {}
            _ => unreachable!("draw is below three"),
        }
        let read = clock.now(instant(browser));
        assert!(read.0 >= last, "seed={seed}, step={step}");
        if report.idle {
            if let Some(due) = report.due {
                assert!(read.0 <= due, "seed={seed}, step={step}");
            }
        } else {
            assert_eq!(read.0, report.now, "seed={seed}, step={step}");
        }
        assert_eq!(read.1.as_nanos() - WALL_START, read.0.as_nanos() - WORLD_START);
        last = read.0;
        readings.push(read);
    }
    readings
}

#[test]
fn the_clock_over_drawn_paces_is_monotonic_and_capped() {
    const SEEDS: &[u64] = &[0, 1, 7, 42, 0xdead_beef, u64::MAX];
    for &seed in SEEDS {
        assert_eq!(drawn_paces(seed), drawn_paces(seed), "seed={seed} replays");
    }
}

#[derive(Default)]
struct MemoryPlatform(Memory);
impl Platform for MemoryPlatform {
    fn read_all(&self) -> Vec<Record> {
        self.0.read_all()
    }
    fn write(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.0.write(key, value)
    }
    fn remove(&mut self, key: &[u8]) {
        self.0.remove(key);
    }
    fn send(&mut self, _: &crate::fetch::Exchange) {}
    fn abandon(&mut self, _: u32) {}
    fn read_on(&mut self) {}
    fn navigate(&mut self, _: &[u8]) {}
    fn stream(&mut self, _: crate::transport::StreamDuty) {}
}

#[test]
fn the_memory_store_keeps_records_under_the_stores_bound() {
    const PREFIX: &[u8] = b"page-";
    const KEY: &[u8] = b"key";
    const VALUE: &[u8] = b"record";
    const ENCODED_BYTES: u64 = 5 + 4 + 8; // Prefix, base64 key, base64 value.
    let mut platform = MemoryPlatform::default();
    let mut store = Store::new(PREFIX, Some(ENCODED_BYTES));
    assert_eq!(Store::new(PREFIX, Some(ENCODED_BYTES - 1)).write(KEY, VALUE, &mut platform), Err(Refused));
    assert!(platform.read_all().is_empty());
    assert_eq!(store.write(KEY, VALUE, &mut platform), Ok(()));
    let expected = vec![Record { key: KEY.into(), value: VALUE.into() }];
    assert_eq!(store.read_all(&platform), expected);
    assert_eq!(store.write(b"two", b"value", &mut platform), Err(Refused));
    assert_eq!(store.read_all(&platform), expected);
    assert_eq!(store.write(KEY, b"new", &mut platform), Ok(()));
    assert_eq!(store.read_all(&platform), vec![Record { key: KEY.into(), value: b"new".as_slice().into() }]);
    store.remove(KEY, &mut platform);
    store.remove(KEY, &mut platform);
    assert!(platform.read_all().is_empty());
    assert_eq!(store.write(KEY, VALUE, &mut platform), Ok(()));
    assert!(store.read_all(&MemoryPlatform::default()).is_empty());
}

#[test]
fn the_worker_follows_only_a_path_on_its_origin() {
    let rows: &[(&[u8], bool)] = &[
        (b"/", true),
        (b"/notes?next=https://example.com/#here", true),
        (b"//example.com/", false),
        (b"\\example.com", false),
        (b"/\\example.com", false),
        (b"https://example.com/", false),
        (b"http://127.0.0.1:8080/", false),
        (b"http://[::1]:8080/", false),
        (b"", false),
        (b"relative", false),
        (b"/space here", false),
        (b"/\xff", false),
        (b"/\n", false),
    ];
    for &(target, expected) in rows {
        assert_eq!(navigable(target), expected, "{target:?}");
    }
}

#[test]
fn the_platform_carries_abandon_and_read_on() {
    const EXCHANGE: u32 = 7;
    let mut platform = FakePlatform::default();
    platform.abandon(EXCHANGE);
    platform.read_on();
    assert_eq!(platform.calls, vec![Call::Abandon(EXCHANGE), Call::ReadOn]);
}

#[test]
fn clock_arithmetic_saturates_and_unrepresentable_timers_are_absent() {
    const LAST: u64 = u64::MAX;
    let mut clock = WorldClock::new(report(true, None), Time::ZERO, Speed(u32::MAX));
    assert_eq!(clock.now(instant(LAST)), (instant(LAST), Wall::from_nanos(LAST)));
    let clock = WorldClock::new(report(true, None), instant(LAST), Speed(1));
    assert_eq!(clock.timer(instant(WORLD_START + 1)), None);
    let mut clock = WorldClock::new(report(true, None), instant(BROWSER_START), Speed(1));
    assert_eq!(clock.now(Time::ZERO).0, instant(WORLD_START));
}
