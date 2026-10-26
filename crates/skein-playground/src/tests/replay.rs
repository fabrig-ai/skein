use crate::replay::{Differs, Feed};
use skein_lib::{Time, Wall, sha256::Sha256};
use skein_ui_web::bridge::{
    Duty, Line, Output, Said, Stamp, Start,
    log::{self, Writer},
};

const BUILD: &[u8] = b"the-recorded-build";
const ROOM: u32 = 4096;

fn stamp(step: u64) -> Stamp {
    Stamp { step, instant: Time::from_nanos(step.saturating_mul(100)) }
}
fn line(bytes: &[u8]) -> Said {
    Said::Line(Line { stream: Output::Standard, process: b"server".as_slice().into(), line: bytes.into() })
}
fn writer(bound: u32) -> Writer {
    Writer::new(BUILD, &Start { story: b"a".as_slice().into(), seed: 42, start: Wall::from_nanos(500) }, bound)
}
fn recorded() -> log::Log {
    let mut writer = writer(ROOM);
    writer.said(&line(b"ready"));
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stamp(stamp(2)));
    writer.said(&line(b"reading"));
    writer.sent(&Duty::Abandon { exchange: 7 });
    writer.said(&Said::Stamp(stamp(5)));
    writer.said(&Said::Stopped);
    log::read(&writer.file()).unwrap()
}

#[test]
fn a_feed_gives_each_input_at_its_stamp() {
    let mut feed = Feed::new(recorded());
    assert_eq!(feed.next(), Some(stamp(2)));
    assert_eq!(feed.next(), Some(stamp(2)));
    feed.said(&line(b"ready"));
    assert_eq!(feed.take(stamp(2)), Ok(Duty::ReadOn));
    assert_eq!(feed.next(), Some(stamp(5)));
    feed.said(&line(b"reading"));
    assert_eq!(feed.take(stamp(5)), Ok(Duty::Abandon { exchange: 7 }));
    assert_eq!(feed.next(), None);
}

#[test]
fn a_replay_that_says_the_same_finishes() {
    let mut feed = Feed::new(recorded());
    feed.said(&Said::Slice);
    feed.said(&line(b"ready"));
    feed.take(stamp(2)).unwrap();
    feed.said(&Said::Failed { words: b"ignored".as_slice().into() });
    feed.said(&line(b"reading"));
    feed.take(stamp(5)).unwrap();
    feed.said(&Said::Stopped);
    assert_eq!(feed.finish(), Ok(()));
}

#[test]
fn a_replay_that_differs_fails_at_that_input() {
    let log = recorded();
    let logged = log.inputs[1].digest;
    let mut feed = Feed::new(log);
    feed.said(&line(b"ready"));
    feed.take(stamp(2)).unwrap();
    feed.said(&line(b"different"));
    let mut expected = log::Fold::default();
    expected.said(&line(b"different"));
    let differs = Differs { input: 1, logged, replayed: expected.take(), build: BUILD.into() };
    assert_eq!(feed.take(stamp(5)), Err(differs));
    assert_eq!(feed.next(), Some(stamp(5)));
}

#[test]
fn a_cut_log_replays_to_its_cut() {
    let mut whole = writer(ROOM);
    whole.said(&line(b"ready"));
    whole.sent(&Duty::ReadOn);
    whole.said(&Said::Stamp(stamp(2)));
    let bound = u32::try_from(whole.file().len()).unwrap();
    let mut writer = writer(bound);
    writer.said(&line(b"ready"));
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stamp(stamp(2)));
    writer.said(&line(b"reading"));
    writer.sent(&Duty::Abandon { exchange: 7 });
    writer.said(&Said::Stamp(stamp(5)));
    let log = log::read(&writer.file()).unwrap();
    assert_eq!(log.end, log::End::Cut);
    assert_eq!(log.inputs.len(), 1);
    let mut feed = Feed::new(log);
    feed.said(&line(b"ready"));
    assert_eq!(feed.take(stamp(2)), Ok(Duty::ReadOn));
    assert_eq!(feed.next(), None);
    assert_eq!(feed.finish(), Ok(()));
}

#[test]
fn a_differing_end_fails_at_the_end() {
    let log = recorded();
    let logged = match log.end {
        log::End::Said(digest) => digest,
        log::End::Cut => panic!("the fixture is whole"),
    };
    let mut feed = Feed::new(log);
    feed.said(&line(b"ready"));
    feed.take(stamp(2)).unwrap();
    feed.said(&line(b"reading"));
    feed.take(stamp(5)).unwrap();
    assert_eq!(feed.finish(), Err(Differs { input: 2, logged, replayed: Sha256::new().finish(), build: BUILD.into() }));
}

#[test]
fn the_first_difference_survives_a_retry_or_finish() {
    let mut feed = Feed::new(recorded());
    let first = feed.take(stamp(2)).unwrap_err();
    feed.said(&line(b"ready"));
    assert_eq!(feed.take(stamp(2)), Err(first));
    let differs = feed.take(stamp(2)).unwrap_err();
    assert_eq!(feed.finish(), Err(differs));
}

#[test]
#[should_panic(expected = "the replay takes the next input at its stamp")]
fn a_wrong_stamp_breaks_the_replay_loops_contract() {
    Feed::new(recorded()).take(stamp(3)).unwrap();
}

#[test]
#[should_panic(expected = "the replay takes every input before finishing")]
fn an_early_finish_breaks_the_replay_loops_contract() {
    Feed::new(recorded()).finish().unwrap();
}

#[test]
fn an_empty_log_checks_the_start_to_end_digest() {
    let mut writer = writer(ROOM);
    writer.said(&Said::Stopped);
    let mut feed = Feed::new(log::read(&writer.file()).unwrap());
    assert_eq!(feed.next(), None);
    feed.said(&Said::Stopped);
    assert_eq!(feed.finish(), Ok(()));
}
