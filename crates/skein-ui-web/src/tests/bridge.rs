use crate::{
    bridge::{
        Duty, Line, Malformed, Output, Said, Stamp, Start, ToPage, ToWorld, decode_to_page, decode_to_world,
        encode_to_page, encode_to_world,
    },
    fetch::{Exchange, Report},
    platform::worker::{Clock, Speed},
    transport::StreamDuty,
};
use skein_lib::{Duration, Time, Wall};

fn world_messages() -> Vec<ToWorld> {
    vec![
        ToWorld::Start(Start {
            story: b"story-a".as_slice().into(),
            seed: 0x0123_4567_89ab_cdef,
            start: Wall::from_nanos(0xfedc_ba98_7654_3210),
        }),
        ToWorld::Start(Start { story: Box::from([]), seed: 0, start: Wall::from_nanos(0) }),
        ToWorld::Duty(Duty::Exchange(Exchange {
            id: 0x0123_4567,
            path: b"/visit?x=1".as_slice().into(),
            method: "POST".into(),
            headers: vec![("content-type".into(), "text/plain; café".into()), (String::new(), String::new())],
            body: Box::from([0, 255, 42]),
            bound: 0x89ab_cdef,
        })),
        ToWorld::Duty(Duty::Exchange(Exchange {
            id: u32::MAX,
            path: Box::from([]),
            method: "GET".into(),
            headers: vec![],
            body: Box::from([]),
            bound: 0,
        })),
        ToWorld::Duty(Duty::Abandon { exchange: 0x7654_3210 }),
        ToWorld::Duty(Duty::Stream(StreamDuty::Open {
            path: b"/events".as_slice().into(),
            headers: vec![(b"last-event-id".as_slice().into(), Box::from([0, 255])), (Box::from([]), Box::from([]))],
            bound: 0x1234_5678,
        })),
        ToWorld::Duty(Duty::Stream(StreamDuty::Open { path: Box::from([]), headers: vec![], bound: 0 })),
        ToWorld::Duty(Duty::Stream(StreamDuty::Close)),
        ToWorld::Duty(Duty::ReadOn),
        ToWorld::Pace(Speed(0)),
        ToWorld::Pace(Speed(u32::MAX)),
    ]
}

fn clock() -> Clock {
    Clock {
        step: 0x0123_4567_89ab_cdef,
        now: Time::from_nanos(0x1234_5678_9abc_def0),
        wall: Wall::from_nanos(0x2345_6789_abcd_ef01),
        idle: true,
        due: Some(Time::from_nanos(u64::MAX)),
        memory: 0x3456_789a_bcde_f012,
        slice: Duration::from_nanos(u64::MAX),
    }
}

fn page_messages() -> Vec<ToPage> {
    let said = vec![
        Said::Slice,
        Said::Report(Report::Answered { exchange: 0x1234_5678, status: 0x1234, body: Box::from([0, 255, 13]) }),
        Said::Report(Report::Answered { exchange: 0, status: 0, body: Box::from([]) }),
        Said::Report(Report::Failed { exchange: u32::MAX }),
        Said::Report(Report::StreamOpened),
        Said::Report(Report::StreamRefused { status: u16::MAX }),
        Said::Report(Report::StreamPiece { bytes: Box::from([255, 0, 7]) }),
        Said::Report(Report::StreamPiece { bytes: Box::from([]) }),
        Said::Report(Report::StreamEnded),
        Said::Line(Line { stream: Output::Standard, process: b"server".as_slice().into(), line: Box::from([0, 255]) }),
        Said::Line(Line { stream: Output::Error, process: b"visitor".as_slice().into(), line: Box::from([]) }),
        Said::Stamp(Stamp { step: 0xfedc_ba98_7654_3210, instant: Time::from_nanos(0xedcb_a987_6543_210f) }),
        Said::Stopped,
        Said::Failed { words: b"the world panicked".as_slice().into() },
        Said::Failed { words: Box::from([]) },
    ];
    let mut messages: Vec<_> = said.into_iter().map(|said| ToPage { clock: clock(), said }).collect();
    messages.push(ToPage {
        clock: Clock {
            step: 0,
            now: Time::from_nanos(0),
            wall: Wall::from_nanos(0),
            idle: false,
            due: None,
            memory: 0,
            slice: Duration::ZERO,
        },
        said: Said::Slice,
    });
    messages
}

#[test]
fn every_message_to_the_world_round_trips() {
    for message in world_messages() {
        assert_eq!(decode_to_world(&encode_to_world(&message)), Ok(message));
    }
}

#[test]
fn every_message_to_the_page_round_trips() {
    for message in page_messages() {
        assert_eq!(decode_to_page(&encode_to_page(&message)), Ok(message));
    }
}

#[test]
fn trailing_bytes_are_malformed() {
    for message in world_messages() {
        let mut bytes = encode_to_world(&message).into_vec();
        bytes.push(0);
        assert_eq!(decode_to_world(&bytes), Err(Malformed), "{message:?}");
    }
    for message in page_messages() {
        let mut bytes = encode_to_page(&message).into_vec();
        bytes.push(0);
        assert_eq!(decode_to_page(&bytes), Err(Malformed), "{message:?}");
    }
}

#[test]
fn a_short_message_is_malformed() {
    for message in world_messages() {
        let bytes = encode_to_world(&message);
        for length in 0..bytes.len() {
            assert_eq!(decode_to_world(&bytes[..length]), Err(Malformed), "{message:?}, prefix {length}");
        }
    }
    for message in page_messages() {
        let bytes = encode_to_page(&message);
        for length in 0..bytes.len() {
            assert_eq!(decode_to_page(&bytes[..length]), Err(Malformed), "{message:?}, prefix {length}");
        }
    }
}

fn clock_bytes(idle: u8, due: &[u8]) -> Vec<u8> {
    let clock = clock();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&clock.step.to_be_bytes());
    bytes.extend_from_slice(&clock.now.as_nanos().to_be_bytes());
    bytes.extend_from_slice(&clock.wall.as_nanos().to_be_bytes());
    bytes.push(idle);
    bytes.extend_from_slice(due);
    bytes.extend_from_slice(&clock.memory.to_be_bytes());
    bytes.extend_from_slice(&clock.slice.as_nanos().to_be_bytes());
    bytes
}

fn page_bytes(said: &[u8]) -> Vec<u8> {
    let mut bytes = clock_bytes(0, &[0]);
    bytes.extend_from_slice(said);
    bytes
}

#[test]
fn an_unknown_tag_is_malformed() {
    for bytes in [vec![255], vec![1, 255], vec![1, 2, 255]] {
        assert_eq!(decode_to_world(&bytes), Err(Malformed));
    }
    for said in [vec![255], vec![1, 255], vec![2, 255]] {
        assert_eq!(decode_to_page(&page_bytes(&said)), Err(Malformed));
    }
    for mut bytes in [clock_bytes(2, &[0]), clock_bytes(0, &[2])] {
        bytes.push(0); // Slice, after the invalid boolean or option tag.
        assert_eq!(decode_to_page(&bytes), Err(Malformed));
    }
}

fn blob(bytes: &[u8]) -> Vec<u8> {
    let mut encoded = u32::try_from(bytes.len()).expect("fixture length fits").to_be_bytes().to_vec();
    encoded.extend_from_slice(bytes);
    encoded
}

fn exchange_bytes(method: &[u8], headers: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut bytes = vec![1, 0]; // ToWorld::Duty(Duty::Exchange).
    bytes.extend_from_slice(&7_u32.to_be_bytes());
    bytes.extend_from_slice(&blob(b"/"));
    bytes.extend_from_slice(&blob(method));
    bytes.extend_from_slice(&u32::try_from(headers.len()).expect("fixture count fits").to_be_bytes());
    for (name, value) in headers {
        bytes.extend_from_slice(&blob(name));
        bytes.extend_from_slice(&blob(value));
    }
    bytes.extend_from_slice(&blob(b"body"));
    bytes.extend_from_slice(&99_u32.to_be_bytes());
    bytes
}

#[test]
fn a_length_past_the_end_is_malformed() {
    for length in [1_u32, u32::MAX] {
        let mut start = vec![0];
        start.extend_from_slice(&length.to_be_bytes());
        assert_eq!(decode_to_world(&start), Err(Malformed));
        let mut failed = vec![5];
        failed.extend_from_slice(&length.to_be_bytes());
        assert_eq!(decode_to_page(&page_bytes(&failed)), Err(Malformed));
    }
}

#[test]
fn a_header_count_past_the_end_is_malformed() {
    for count in [1_u32, u32::MAX] {
        let mut exchange = vec![1, 0];
        exchange.extend_from_slice(&7_u32.to_be_bytes());
        exchange.extend_from_slice(&blob(b"/"));
        exchange.extend_from_slice(&blob(b"GET"));
        exchange.extend_from_slice(&count.to_be_bytes());
        assert_eq!(decode_to_world(&exchange), Err(Malformed));
        let mut stream = vec![1, 2, 0];
        stream.extend_from_slice(&blob(b"/events"));
        stream.extend_from_slice(&count.to_be_bytes());
        assert_eq!(decode_to_world(&stream), Err(Malformed));
    }
}

#[test]
fn a_method_not_utf8_is_malformed() {
    assert_eq!(decode_to_world(&exchange_bytes(&[255], &[])), Err(Malformed));
}

#[test]
fn a_header_not_utf8_is_malformed() {
    for headers in [vec![(&[255][..], &b"value"[..])], vec![(&b"name"[..], &[255][..])]] {
        assert_eq!(decode_to_world(&exchange_bytes(b"GET", &headers)), Err(Malformed));
    }
}

#[test]
fn integers_and_lengths_are_big_endian() {
    assert_eq!(encode_to_world(&ToWorld::Pace(Speed(0x0123_4567))).as_ref(), &[2, 1, 35, 69, 103]);
    let mut expected = vec![0];
    expected.extend_from_slice(&[0, 0, 0, 1, b'a']);
    expected.extend_from_slice(&[1, 35, 69, 103, 137, 171, 205, 239]);
    expected.extend_from_slice(&[254, 220, 186, 152, 118, 84, 50, 16]);
    let start = ToWorld::Start(Start {
        story: b"a".as_slice().into(),
        seed: 0x0123_4567_89ab_cdef,
        start: Wall::from_nanos(0xfedc_ba98_7654_3210),
    });
    assert_eq!(encode_to_world(&start).as_ref(), expected);
    assert_eq!(decode_to_world(&expected), Ok(start));

    let expected = page_bytes(&[1, 0, 1, 35, 69, 103, 137, 171, 0, 0, 0, 1, 255]);
    let page = ToPage {
        clock: Clock { idle: false, due: None, ..clock() },
        said: Said::Report(Report::Answered { exchange: 0x0123_4567, status: 0x89ab, body: Box::from([255]) }),
    };
    assert_eq!(encode_to_page(&page).as_ref(), expected);
    assert_eq!(decode_to_page(&expected), Ok(page));
}

#[test]
fn empty_headers_have_exact_container_counts() {
    const COUNT: usize = 100;
    let message = ToWorld::Duty(Duty::Exchange(Exchange {
        id: 7,
        path: Box::from([]),
        method: String::new(),
        headers: vec![(String::new(), String::new()); COUNT],
        body: Box::from([]),
        bound: 9,
    }));
    let ToWorld::Duty(Duty::Exchange(exchange)) = decode_to_world(&encode_to_world(&message)).expect("valid") else {
        panic!("an exchange round trips");
    };
    assert_eq!(exchange.headers.len(), COUNT);
    assert_eq!(exchange.headers.capacity(), COUNT);

    let message = ToWorld::Duty(Duty::Stream(StreamDuty::Open {
        path: Box::from([]),
        headers: vec![(Box::from([]), Box::from([])); COUNT],
        bound: 9,
    }));
    let ToWorld::Duty(Duty::Stream(StreamDuty::Open { headers, .. })) =
        decode_to_world(&encode_to_world(&message)).expect("valid")
    else {
        panic!("an open round trips");
    };
    assert_eq!(headers.len(), COUNT);
    assert_eq!(headers.capacity(), COUNT);
}
