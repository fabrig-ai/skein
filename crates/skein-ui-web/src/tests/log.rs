use crate::{
    bridge::{
        Duty, Line, Output, Said, Stamp, Start,
        log::{self, End, Fold, Refused, Writer},
    },
    fetch::{Exchange, Report},
    transport::StreamDuty,
};
use skein_lib::{Time, Wall, Writer as BytesWriter, base64, base64::Alphabet, sha256::Sha256};

const BUILD: &[u8] = b"bundle-digest";
const ROOM: u32 = 16_384;
const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn start() -> Start {
    Start { story: b"story-a".as_slice().into(), seed: u64::MAX, start: Wall::from_nanos(u64::MAX) }
}

fn stamp(step: u64) -> Stamp {
    Stamp { step, instant: Time::from_nanos(step.saturating_mul(10)) }
}

fn duties() -> Vec<Duty> {
    vec![
        Duty::Exchange(Exchange {
            id: u32::MAX,
            method: "POST".into(),
            path: b"/visit".as_slice().into(),
            headers: vec![("content-type".into(), "text/plain; café".into()), (String::new(), String::new())],
            body: b"a body\n".as_slice().into(),
            bound: u32::MAX,
        }),
        Duty::Abandon { exchange: u32::MAX },
        Duty::Stream(StreamDuty::Open {
            path: b"/events".as_slice().into(),
            headers: vec![(b"last-event-id".as_slice().into(), Box::from([0, 255, b':']))],
            bound: 64,
        }),
        Duty::Stream(StreamDuty::Close),
        Duty::ReadOn,
        Duty::Exchange(Exchange {
            id: 0,
            method: "GET".into(),
            path: Box::from([]),
            headers: vec![],
            body: Box::from([]),
            bound: 0,
        }),
        Duty::Stream(StreamDuty::Open { path: Box::from([]), headers: vec![], bound: 0 }),
    ]
}

fn reports() -> Vec<Said> {
    vec![
        Said::Report(Report::Answered { exchange: 7, status: 201, body: Box::from([0, 255]) }),
        Said::Report(Report::Failed { exchange: 8 }),
        Said::Report(Report::StreamOpened),
        Said::Report(Report::StreamRefused { status: 403 }),
        Said::Report(Report::StreamPiece { bytes: b"piece".as_slice().into() }),
        Said::Report(Report::StreamEnded),
        Said::Line(Line { stream: Output::Standard, process: b"server".as_slice().into(), line: Box::from([0, 255]) }),
        Said::Line(Line { stream: Output::Error, process: Box::from([]), line: Box::from([]) }),
    ]
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.finish()
}

fn empty_file() -> String {
    String::from_utf8(Writer::new(BUILD, &start(), ROOM).file().into()).unwrap()
}

fn one_input(bound: u32) -> Writer {
    let mut writer = Writer::new(BUILD, &start(), bound);
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stamp(stamp(1)));
    writer
}

#[test]
fn a_log_reads_back_as_written() {
    let mut writer = Writer::new(BUILD, &start(), ROOM);
    let mut expected = Vec::new();
    let mut fold = Fold::default();
    for (index, duty) in duties().into_iter().enumerate() {
        for report in reports() {
            writer.said(&report);
            fold.said(&report);
        }
        let stamp = stamp(u64::try_from(index).unwrap());
        writer.sent(&duty);
        writer.said(&Said::Stamp(stamp));
        expected.push(log::Input { stamp, duty, digest: fold.take() });
    }
    writer.said(&Said::Stopped);
    fold.said(&Said::Stopped);
    assert_eq!(
        log::read(&writer.file()),
        Ok(log::Log { build: BUILD.into(), start: start(), inputs: expected, end: End::Said(fold.take()) })
    );
    let text = String::from_utf8(writer.file().into()).unwrap();
    assert!(text.contains("exchange 0 GET = = = 0 "));
    assert!(text.contains("open = = 0 "));
}

#[test]
fn the_digest_folds_reports_lines_and_the_stop_alone() {
    let mut alone = Fold::default();
    let mut interleaved = Fold::default();
    for said in reports().into_iter().chain([Said::Stopped]) {
        alone.said(&said);
        interleaved.said(&Said::Slice);
        interleaved.said(&Said::Stamp(stamp(17)));
        interleaved.said(&Said::Failed { words: b"failed".as_slice().into() });
        interleaved.said(&said);
    }
    let canonical = b"report answered 7 201 AP8=\nreport failed 8\nreport stream-opened\nreport stream-refused 403\nreport stream-piece cGllY2U=\nreport stream-ended\nline out c2VydmVy AP8=\nline err = =\nstopped\n";
    assert_eq!(alone.take(), hash(canonical));
    assert_eq!(interleaved.take(), hash(canonical));
    assert_eq!(alone.take(), hash(b""));
    assert_eq!(interleaved.take(), hash(b""));
}

#[test]
fn a_stamp_pairs_with_the_oldest_duty_sent() {
    let mut writer = Writer::new(BUILD, &start(), ROOM);
    writer.sent(&Duty::Abandon { exchange: 42 });
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Report(Report::StreamOpened));
    writer.said(&Said::Stamp(stamp(3)));
    writer.said(&Said::Report(Report::StreamEnded));
    writer.said(&Said::Stamp(stamp(4)));
    let log = log::read(&writer.file()).unwrap();
    assert_eq!(log.inputs[0].duty, Duty::Abandon { exchange: 42 });
    assert_eq!(log.inputs[0].stamp, stamp(3));
    assert_eq!(log.inputs[0].digest, hash(b"report stream-opened\n"));
    assert_eq!(log.inputs[1].duty, Duty::ReadOn);
    assert_eq!(log.inputs[1].stamp, stamp(4));
    assert_eq!(log.inputs[1].digest, hash(b"report stream-ended\n"));
}

#[test]
fn a_log_at_its_bound_is_whole() {
    let bound = u32::try_from(one_input(ROOM).file().len()).unwrap();
    let file = one_input(bound).file();
    assert_eq!(u32::try_from(file.len()).unwrap(), bound);
    let log = log::read(&file).unwrap();
    assert_eq!(log.inputs.len(), 1);
    assert_eq!(log.end, End::Said(hash(b"")));
    let minimum = u32::try_from(empty_file().len()).unwrap();
    assert_eq!(Writer::new(BUILD, &start(), minimum).file().len(), usize::try_from(minimum).unwrap());
}

#[test]
fn a_log_past_its_bound_ends_with_its_cut() {
    let bound = u32::try_from(one_input(ROOM).file().len()).unwrap() - 1;
    let mut writer = one_input(bound);
    let file = writer.file();
    assert!(file.ends_with(b"cut\n"));
    assert!(file.len() <= usize::try_from(bound).unwrap());
    let log = log::read(&file).unwrap();
    assert!(log.inputs.is_empty());
    assert_eq!(log.end, End::Cut);
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stamp(stamp(2)));
    writer.said(&Said::Stopped);
    assert_eq!(writer.file(), file);
}

#[test]
#[should_panic(expected = "the log bound holds the start line and the terminal reserve")]
fn a_bound_short_of_the_start_and_terminal_breaks_the_callers_contract() {
    let bound = u32::try_from(empty_file().len()).unwrap() - 1;
    let _writer = Writer::new(BUILD, &start(), bound);
}

#[test]
fn a_queued_cut_waits_for_its_stamp() {
    // Two one-digit stamps cost the same; only the first input fits.
    let mut first = Writer::new(BUILD, &start(), ROOM);
    first.sent(&Duty::ReadOn);
    first.said(&Said::Stamp(stamp(0)));
    let bound = u32::try_from(first.file().len()).unwrap();
    let mut writer = Writer::new(BUILD, &start(), bound);
    writer.sent(&Duty::ReadOn);
    writer.sent(&Duty::ReadOn);
    writer.sent(&Duty::ReadOn);
    assert_eq!(log::read(&writer.file()).unwrap().end, End::Said(hash(b"")));
    writer.said(&Said::Stamp(stamp(0)));
    assert_eq!(writer.file(), first.file());
    writer.said(&Said::Stamp(stamp(0)));
    let log = log::read(&writer.file()).unwrap();
    assert_eq!(log.inputs.len(), 1);
    assert_eq!(log.end, End::Cut);
}

#[test]
fn a_long_stamp_cuts_before_its_input() {
    let mut first = Writer::new(BUILD, &start(), ROOM);
    first.sent(&Duty::ReadOn);
    first.said(&Said::Stamp(stamp(0)));
    let bound = u32::try_from(first.file().len()).unwrap();
    let mut writer = Writer::new(BUILD, &start(), bound);
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stamp(Stamp { step: u64::MAX, instant: Time::from_nanos(u64::MAX) }));
    let log = log::read(&writer.file()).unwrap();
    assert!(log.inputs.is_empty());
    assert_eq!(log.end, End::Cut);
}

#[test]
fn another_format_or_version_is_refused_naming_it() {
    for (file, named) in [
        (b"other-log 1 build = 0 0\n".as_slice(), b"other-log 1".as_slice()),
        (b"skein-playground-log 2 build = 0 0\n", b"skein-playground-log 2"),
        (b"skein-playground-log 01 build = 0 0\n", b"skein-playground-log 01"),
        (b"other\n", b"other"),
        (b"", b""),
    ] {
        assert_eq!(log::read(file), Err(Refused::Format { named: named.into() }));
    }
}

#[test]
fn each_malformed_line_is_refused_with_its_number() {
    let whole = empty_file();
    let header = whole.lines().next().unwrap();
    let end = whole.lines().nth(1).unwrap();
    let digest = end.strip_prefix("end ").unwrap();
    let cases = [
        (format!("{header}\n0 0 abandon\n{end}\n"), 2),
        (format!("{header}\n0 0 read-on {digest} extra\n{end}\n"), 2),
        (format!("{header}\nx 0 read-on {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 open !!! = 10 {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 open Zh== = 10 {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 read-on BAD\n{end}\n"), 2),
        (format!("{whole}cut\n"), 3),
        (format!("{header}\n"), 2),
        (format!("{header}\n\n{end}\n"), 2),
        (format!("{header}\ncut\n{end}\n"), 3),
        (format!("{header}\n0  0 close {digest}\n{end}\n"), 2),
        (format!("{header}\n+1 0 close {digest}\n{end}\n"), 2),
        (format!("{header}\n01 0 close {digest}\n{end}\n"), 2),
        (format!("{header}\n18446744073709551616 0 close {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 abandon 4294967296 {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 open = = -1 {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 close {digest}\r\n{end}\n"), 2),
        (format!("{header}\n0 0 close {}\n{end}\n", "A".repeat(64)), 2),
        (format!("{header}\n{end}"), 2),
        (format!("{header}\ncut"), 2),
        (format!("{header}\ncut extra\n"), 2),
        (format!("{header}\n{end} extra\n"), 2),
        (format!("{header}\n0 0 unknown {digest}\n{end}\n"), 2),
        (format!("{header} extra\n{end}\n"), 1),
        (format!("skein-playground-log 1 build = 0\n{end}\n"), 1),
        (format!("skein-playground-log 1 build = bad 0\n{end}\n"), 1),
        (format!("skein-playground-log 1 build !! 0 0\n{end}\n"), 1),
        (header.to_owned(), 1),
        (format!("skein-playground-log  1 build = 0 0\n{end}\n"), 1),
        (format!("skein-playground-log\t1 build = 0 0\n{end}\n"), 1),
        (format!("skein-playground-log 1 build  0 0\n{end}\n"), 1),
        (format!("{header}\n0 0 exchange 1 GET = eA== = 1 {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 exchange 1 GET = eDog/wo= = 1 {digest}\n{end}\n"), 2),
        (format!("{header}\n0 0 open = == 1 {digest}\n{end}\n"), 2),
    ];
    for (file, line) in cases {
        assert_eq!(log::read(file.as_bytes()), Err(Refused::Malformed { line }), "{file:?}");
    }
    let mut binary = format!("{header}\n").into_bytes();
    binary.extend_from_slice(b"\xff\n");
    assert_eq!(log::read(&binary), Err(Refused::Malformed { line: 2 }));
}

#[test]
fn a_bodys_bytes_survive_as_base64() {
    let bytes: Box<[u8]> = (0..=255).collect();
    let duty = Duty::Exchange(Exchange {
        id: 7,
        method: "POST".into(),
        path: bytes.clone(),
        headers: vec![],
        body: bytes,
        bound: 256,
    });
    let mut writer = Writer::new(BUILD, &start(), ROOM);
    writer.sent(&duty);
    writer.said(&Said::Stamp(stamp(1)));
    let log = log::read(&writer.file()).unwrap();
    assert_eq!(log.inputs[0].duty, duty);
    assert!(writer.file().is_ascii());
}

#[test]
fn an_unstamped_duty_is_not_written() {
    let mut writer = Writer::new(BUILD, &start(), ROOM);
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stopped);
    let log = log::read(&writer.file()).unwrap();
    assert!(log.inputs.is_empty());
    assert_eq!(log.end, End::Said(hash(b"stopped\n")));
}

#[test]
fn reading_a_different_build_keeps_its_name() {
    let file = empty_file().replace("bundle-digest", "another-build");
    assert_eq!(log::read(file.as_bytes()).unwrap().build.as_ref(), b"another-build");
}

#[test]
fn the_text_form_stands_alone() {
    let start = Start { story: b"a".as_slice().into(), seed: 42, start: Wall::from_nanos(500) };
    let mut writer = Writer::new(b"build", &start, ROOM);
    writer.sent(&Duty::Abandon { exchange: 7 });
    writer.said(&Said::Stamp(stamp(0)));
    let expected = format!("skein-playground-log 1 build YQ== 42 500\n0 0 abandon 7 {EMPTY}\nend {EMPTY}\n");
    assert_eq!(writer.file().as_ref(), expected.as_bytes());
    assert_eq!(log::read(expected.as_bytes()).unwrap().start, start);
}

#[test]
fn taking_a_file_does_not_reset_the_fold() {
    let mut writer = Writer::new(BUILD, &start(), ROOM);
    writer.said(&Said::Stopped);
    let snapshot = writer.file();
    assert_eq!(writer.file(), snapshot);
    writer.sent(&Duty::ReadOn);
    writer.said(&Said::Stamp(stamp(0)));
    let log = log::read(&writer.file()).unwrap();
    assert_eq!(log.inputs[0].digest, hash(b"stopped\n"));
    assert_eq!(log.end, End::Said(hash(b"")));
}

#[test]
fn a_large_fold_has_only_its_final_base64_padding() {
    for size in [767, 768, 769, 1536, 1537] {
        let bytes = vec![255; size];
        let len = base64::encoded_len(Alphabet::Standard, u32::try_from(size).unwrap()).unwrap();
        let mut out = BytesWriter::new(usize::try_from(len).unwrap());
        base64::encode(Alphabet::Standard, &bytes, &mut out).unwrap();
        let mut canonical = b"report stream-piece ".to_vec();
        canonical.extend_from_slice(&out.finish());
        canonical.push(b'\n');
        let mut fold = Fold::default();
        fold.said(&Said::Report(Report::StreamPiece { bytes: bytes.into() }));
        assert_eq!(fold.take(), hash(&canonical));
    }
}
