use skein_lib::{Duration, Time};
use skein_playground::{native::Native, slices::Composition};
use skein_playground_world::bare::{self, END, Visit, exchange, heard};
use skein_ui_web::{
    bridge::Duty,
    fetch::Report,
    platform::{
        Platform,
        worker::{Figures, Heard, Speed},
    },
    transport::StreamDuty,
};
fn send(page: &mut Native<Visit>, id: u32, path: &str) {
    let Duty::Exchange(exchange) = exchange(id, path) else { unreachable!("exchange fixture") };
    page.send(&exchange);
}
fn finish<C: Composition>(page: &mut Native<C>) {
    page.pace(Speed(1));
    page.run();
    for _ in 0..100 {
        if page.slices().world().settled() {
            break;
        }
        let due = page.wake().expect("the fixture has its end due");
        page.advance(due);
        page.run();
    }
    assert!(page.slices().world().settled());
    assert_eq!(page.wake(), None);
    assert!(page.slices().world().processes().iter().all(skein_shell::Host::is_empty));
}
#[test]
fn an_exchange_crosses_the_bridge_in_memory_and_comes_back_under_its_id() {
    let mut page = bare::native(Speed(0));
    send(&mut page, 913, "/echo");
    page.run();
    let messages = heard(&mut page);
    assert!(messages.iter().any(|heard| matches!(heard, Heard::Stamp(_))));
    assert!(messages.iter().any(|heard| matches!(heard, Heard::Report(Report::Answered { exchange: 913, status: 200, body }) if body.starts_with(b"GET\n/echo\n"))));
    finish(&mut page);
}
#[test]
fn a_stream_is_read_one_piece_per_read_on_across_the_bridge() {
    let mut page = bare::native(Speed(0));
    page.stream(StreamDuty::Open { path: b"/stream/1x8".as_slice().into(), headers: Vec::new(), bound: 4 });
    page.run();
    let messages = heard(&mut page);
    assert!(messages.contains(&Heard::Report(Report::StreamOpened)));
    assert!(!messages.iter().any(|heard| matches!(heard, Heard::Report(Report::StreamPiece { .. }))));
    for _ in 0..2 {
        page.read_on();
        page.run();
        let messages = heard(&mut page);
        assert_eq!(
            messages
                .iter()
                .filter(
                    |heard| matches!(heard, Heard::Report(Report::StreamPiece { bytes }) if bytes.as_ref() == b"ssss")
                )
                .count(),
            1
        );
        page.run();
        assert!(heard(&mut page).is_empty());
    }
    page.read_on();
    page.run();
    assert!(heard(&mut page).contains(&Heard::Report(Report::StreamEnded)));
    finish(&mut page);
}
#[test]
fn the_clients_clock_is_the_worlds() {
    let mut page = bare::native(Speed(10));
    page.run();
    let due = page.slices().world().next_due().expect("fixture's end");
    let start_wall = page.slices().world().wall();
    page.advance(Time::from_nanos(7));
    assert_eq!(page.now().0, Time::from_nanos(70));
    assert_eq!(page.now().1.as_nanos(), start_wall.as_nanos() + 70);
    page.advance(END);
    let before = page.now().0;
    assert_eq!(before, due);
    page.run();
    assert!(page.now().0 >= before);
    assert_eq!(page.now().0, page.slices().world().now());
    finish(&mut page);
}
#[test]
fn the_clients_deadline_is_a_browser_instant_of_the_worlds_time() {
    let mut page = bare::native(Speed(10));
    page.run();
    assert_eq!(page.timer(Time::from_nanos(101)), Some(Time::from_nanos(11)));
    page.advance(Time::from_nanos(10));
    assert!(page.now().0 < Time::from_nanos(101));
    page.advance(Time::from_nanos(11));
    assert!(page.now().0 >= Time::from_nanos(101));
    assert_eq!(page.timer(END.saturating_add(skein_lib::Duration::from_nanos(1))), None);
    page.pace(Speed(0));
    assert_eq!(page.timer(Time::from_nanos(200)), None);
    finish(&mut page);
}
#[test]
fn a_line_printed_in_the_world_is_heard_on_the_page() {
    let mut page = bare::native(Speed(0));
    send(&mut page, 1, "/print");
    page.run();
    assert!(heard(&mut page).iter().any(|heard| matches!(heard, Heard::Line(line) if line.process.as_ref() == b"scripted server" && line.line.as_ref() == b"a scripted line" && line.stream == skein_ui_web::bridge::Output::Standard)));
    finish(&mut page);
}

#[test]
fn a_native_walk_has_zero_figures() {
    let mut page = bare::native(Speed(0));
    let zero = Figures { memory: 0, slice: Duration::ZERO };
    assert_eq!(page.figures(), zero);
    send(&mut page, 1, "/print");
    page.run();
    assert!(heard(&mut page).iter().any(|heard| matches!(heard, Heard::Line(_))));
    assert_eq!(page.figures(), zero);
    finish(&mut page);
    assert_eq!(page.figures(), zero);
}

#[test]
fn immediate_failures_and_stream_closes_keep_the_pages_order() {
    let mut page = bare::native(Speed(0));
    send(&mut page, 1, "/slow");
    send(&mut page, 2, "/slow");
    send(&mut page, 3, "/echo");
    page.stream(StreamDuty::Open { path: b"/stream/1x4".as_slice().into(), headers: Vec::new(), bound: 4 });
    page.stream(StreamDuty::Close);
    assert_eq!(page.heard(), Some(Heard::Report(Report::Failed { exchange: 3 })));
    assert_eq!(page.heard(), Some(Heard::Report(Report::StreamEnded)));
    page.run();
    finish(&mut page);
}
#[test]
fn an_answer_crossing_an_abandon_is_dropped_and_memory_is_local() {
    let mut page = bare::native(Speed(0));
    send(&mut page, 1, "/echo");
    page.abandon(1);
    page.run();
    assert!(
        !heard(&mut page)
            .iter()
            .any(|heard| matches!(heard, Heard::Report(Report::Answered { .. } | Report::Failed { .. })))
    );
    assert!(page.write(b"key", b"value"));
    assert_eq!(page.read_all().len(), 1);
    let mut other = bare::native(Speed(0));
    assert!(other.read_all().is_empty());
    finish(&mut other);
    page.remove(b"key");
    assert!(page.read_all().is_empty());
    finish(&mut page);
}

#[test]
fn a_duty_wakes_the_bridge_and_pace_keeps_the_instant_it_was_selected() {
    let mut page = bare::native(Speed(2));
    page.run();
    assert!(page.wake().is_some_and(|at| at > page.browser()));
    page.pace(Speed(0));
    page.advance(Time::from_nanos(50));
    send(&mut page, 1, "/echo");
    assert_eq!(page.wake(), Some(page.browser()));
    page.run();
    assert_eq!(page.slices().world().now(), Time::ZERO);
    assert_eq!(page.now().0, Time::ZERO);
    finish(&mut page);
}
