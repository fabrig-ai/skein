use skein_lib::{Duration, Rng, Time};
use skein_playground::slices::Slices;
use skein_playground_world::bare::{self, END, Visit, exchange, give, said};
use skein_ui_web::{
    bridge::{Duty, Said, Stamp, ToWorld},
    platform::worker::Speed,
};

fn drain_busy(slices: &mut Slices<Visit>, at: Time) -> Vec<Said> {
    let mut messages = Vec::new();
    for _ in 0..1000 {
        if slices.wake(at).is_none_or(|due| due > at) {
            break;
        }
        slices.slice(at);
        messages.extend(said(slices));
    }
    assert!(!slices.world().busy(), "bounded fixture settles its immediate work");
    messages
}

#[test]
fn a_world_paced_at_drawn_speeds_and_pauses_ends_as_the_world_run_whole() {
    for seed in [1, 7, 73, u64::MAX] {
        let mut rng = Rng::new(seed);
        let mut reference = bare::slices(1, Speed(u32::MAX), Some(END), 4);
        give(&mut reference, exchange(1, "/echo"), Time::ZERO);
        give(&mut reference, exchange(2, "/slow"), Time::ZERO);
        let mut expected = Vec::new();
        let mut at = Time::ZERO;
        for _ in 0..2000 {
            let Some(due) = reference.wake(at) else { break };
            at = due;
            reference.slice(at);
            expected.extend(said(&mut reference).into_iter().filter(|said| !matches!(said, Said::Slice)));
        }
        assert!(reference.world().settled());
        let whole = reference.into_world().finish();
        let budget = u32::try_from(rng.between(1, 17)).expect("tiny budget");
        let mut slices = bare::slices(budget, Speed(1), Some(END), 4);
        give(&mut slices, exchange(1, "/echo"), Time::ZERO);
        give(&mut slices, exchange(2, "/slow"), Time::ZERO);
        let mut got = Vec::new();
        let mut at = Time::ZERO;
        for turn in 0..4000 {
            // Force pauses into every replay, with the rest of the pace drawn.
            let speed = if turn % 3 == 0 { Speed(0) } else { Speed(u32::try_from(rng.between(1, 21)).expect("speed")) };
            slices.heard(ToWorld::Pace(speed), at);
            slices.slice(at);
            got.extend(said(&mut slices).into_iter().filter(|said| !matches!(said, Said::Slice)));
            if slices.wake(at).is_none() && slices.world().settled() {
                break;
            }
            at = at.saturating_add(Duration::from_millis(rng.between(1, 9)));
        }
        assert!(slices.world().settled(), "seed {seed}");
        let paced = slices.into_world().finish();
        assert_eq!(got, expected, "seed {seed}");
        assert_eq!(paced.steps, whole.steps, "seed {seed}");
        assert_eq!(paced.trace, whole.trace, "seed {seed}");
    }
}
#[test]
fn a_paused_world_answers_an_input_at_its_paused_instant() {
    let mut slices = bare::slices(16, Speed(0), None, 4);
    give(&mut slices, exchange(7, "/echo"), Time::ZERO);
    let messages = drain_busy(&mut slices, Time::ZERO);
    assert!(messages.iter().any(|said| matches!(
        said,
        Said::Report(skein_ui_web::fetch::Report::Answered { exchange: 7, status: 200, .. })
    )));
    assert_eq!(slices.world().now(), Time::ZERO);
    bare::finish(slices);
}
#[test]
fn a_paused_world_moves_no_clock_and_has_no_wake() {
    let mut slices = bare::slices(16, Speed(0), Some(END), 4);
    slices.slice(Time::from_nanos(9_000_000_000));
    assert_eq!(slices.world().now(), Time::ZERO);
    assert_eq!(slices.wake(Time::from_nanos(9_000_000_000)), None);
    assert_eq!(said(&mut slices), vec![Said::Slice]);
    bare::finish(slices);
}
#[test]
fn a_slice_steps_at_most_its_budget() {
    for budget in [1, 2, 7, 16] {
        let mut slices = bare::slices(budget, Speed(1), None, 4);
        give(&mut slices, exchange(1, "/echo"), Time::ZERO);
        let before = slices.world().steps();
        slices.slice(Time::ZERO);
        assert_eq!(slices.world().steps() - before, u64::from(budget));
        bare::finish(slices);
    }
}
#[test]
fn time_the_world_could_not_keep_up_with_is_dropped() {
    let mut slices = bare::slices(1, Speed(1), None, 4);
    give(&mut slices, exchange(1, "/slow"), Time::ZERO);
    let gap = Time::from_nanos(10_000_000_000);
    slices.slice(gap);
    assert_eq!(slices.world().now(), Time::ZERO);
    let _messages = drain_busy(&mut slices, gap);
    assert_eq!(slices.world().now(), Time::ZERO);
    assert!(slices.world().next_due().is_some_and(|due| due > Time::ZERO));
    bare::finish(slices);
}
#[test]
fn inputs_past_the_visitors_room_wait_for_the_next_slice() {
    let mut slices = bare::slices(1, Speed(0), None, 1);
    for exchange in 0..3 {
        give(&mut slices, Duty::Abandon { exchange }, Time::ZERO);
    }
    let first = slices.world().steps();
    let mut stamps = Vec::new();
    for _ in 0..3 {
        slices.slice(Time::ZERO);
        let messages = said(&mut slices);
        assert_eq!(messages.len(), 1);
        let Said::Stamp(stamp) = &messages[0] else { panic!("one entered input") };
        stamps.push(stamp.step);
    }
    assert_eq!(stamps, vec![first, first + 1, first + 2]);
    bare::finish(slices);
}
#[test]
fn each_input_is_stamped_at_its_step_and_instant() {
    let mut slices = bare::slices(1, Speed(1), None, 4);
    slices.slice(Time::from_nanos(57));
    let _messages = said(&mut slices);
    let expected = Stamp { step: slices.world().steps(), instant: slices.world().now() };
    give(&mut slices, Duty::Abandon { exchange: 1 }, Time::from_nanos(57));
    slices.slice(Time::from_nanos(57));
    assert_eq!(said(&mut slices), vec![Said::Stamp(expected)]);
    bare::finish(slices);
}
#[test]
fn a_settled_world_says_it_stopped_once() {
    let mut slices = bare::slices(16, Speed(1), Some(END), 4);
    let messages = drain_busy(&mut slices, END);
    assert_eq!(messages.iter().filter(|said| matches!(said, Said::Stopped)).count(), 1);
    assert!(slices.world().settled());
    assert_eq!(slices.wake(END), None);
    give(&mut slices, exchange(9, "/echo"), END);
    slices.slice(END);
    assert!(said(&mut slices).is_empty());
    let _finished = slices.into_world().finish();
}
#[test]
fn every_message_carries_the_clock_at_its_slice() {
    let mut slices = bare::slices(16, Speed(0), None, 4);
    give(&mut slices, exchange(1, "/echo"), Time::ZERO);
    for _ in 0..100 {
        slices.slice(Time::ZERO);
        let world = slices.world();
        let expected = skein_ui_web::platform::worker::Clock {
            step: world.steps(),
            now: world.now(),
            wall: world.wall(),
            idle: !world.busy(),
            due: world.next_due(),
        };
        while let Some(message) = slices.outgoing() {
            assert_eq!(message.clock, expected);
        }
        if !slices.world().busy() {
            break;
        }
    }
    bare::finish(slices);
}
#[test]
fn a_pace_change_banks_elapsed_time_at_the_old_speed() {
    let mut slices = bare::slices(16, Speed(2), Some(END), 4);
    slices.heard(ToWorld::Pace(Speed(0)), Time::from_nanos(50));
    slices.slice(Time::from_nanos(500));
    assert_eq!(slices.world().now(), Time::from_nanos(100));
    assert_eq!(slices.wake(Time::from_nanos(500)), None);
    bare::finish(slices);
}

#[test]
fn an_idle_world_without_a_due_instant_moves_to_the_target() {
    let mut slices = bare::slices(1, Speed(3), None, 4);
    assert_eq!(slices.world().next_due(), None);
    assert_eq!(slices.wake(Time::ZERO), None);
    let steps = slices.world().steps();
    slices.slice(Time::from_nanos(7));
    assert_eq!(slices.world().now(), Time::from_nanos(21));
    assert_eq!(slices.world().steps(), steps);
    bare::finish(slices);
}
