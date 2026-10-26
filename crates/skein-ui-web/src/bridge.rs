//! The bridge between the page and a world in a worker (playground.md, 5).

/// A duty of the transport's, as data: what the page hands the world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Duty {
    Exchange(crate::fetch::Exchange),
    Abandon {
        exchange: u32,
    },
    Stream(crate::transport::StreamDuty),
    /// The next piece of the stream, after the step that took the last.
    ReadOn,
}

use crate::{
    fetch::{Exchange, Report},
    platform::worker::{Clock, Speed},
    transport::{Header, StreamDuty},
};
use skein_lib::{Reader, Time, Wall, Writer};

/// What the page sends the world.
#[derive(Debug, PartialEq, Eq)]
pub enum ToWorld {
    /// A visit's story, seed and starting wall (playground.md, 6).
    Start(Start),
    /// A duty of the transport's, for the visitor.
    Duty(Duty),
    /// The pace from now on, zero for paused.
    Pace(Speed),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Start {
    pub story: Box<[u8]>,
    pub seed: u64,
    pub start: Wall,
}

/// What the world said, with its clock when it said it. The session log
/// folds `said`, never the clock, whose messages depend on the pace.
#[derive(Debug, PartialEq, Eq)]
pub struct ToPage {
    pub clock: Clock,
    pub said: Said,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Said {
    /// A slice's end: its clock alone.
    Slice,
    /// A report of the visitor's, fetch's own.
    Report(Report),
    /// A process's line, without its line feed.
    Line(Line),
    /// The next input the page sent, in order, entered at this moment.
    Stamp(Stamp),
    /// Every process ended: the world has nothing more to step.
    Stopped,
    /// A panic in the world, with its words.
    Failed { words: Box<[u8]> },
}

#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    pub stream: Output,
    /// The process's name, the composition's.
    pub process: Box<[u8]>,
    pub line: Box<[u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Standard,
    Error,
}

/// A moment of the world (world.md, 3): its step count and instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub step: u64,
    pub instant: Time,
}

/// Bytes that are no message of this build's.
#[derive(Debug, PartialEq, Eq)]
pub struct Malformed;

/// Encode one message, counting its length before allocating one writer.
/// Integers and lengths are big-endian; enum and option tags are one byte.
#[must_use]
pub fn encode_to_world(message: &ToWorld) -> Box<[u8]> {
    let mut length = Encoding::Length(0);
    write_world(message, &mut length);
    let mut bytes = Encoding::Bytes(Writer::new(length.len()));
    write_world(message, &mut bytes);
    bytes.finish()
}

/// Validate the whole message before allocating any decoded containers.
/// Messages longer than `u32::MAX`, trailing bytes and invalid tags, lengths,
/// counts or UTF-8 are malformed. Payload allocations total at most the input
/// length; vectors have exactly the validated header count, each encoded
/// header costing at least eight bytes. The decoded heap is linear in input.
pub fn decode_to_world(bytes: &[u8]) -> Result<ToWorld, Malformed> {
    u32::try_from(bytes.len()).map_err(|_| Malformed)?;
    let mut reader = Reader::new(bytes);
    read_world(&mut reader, false).ok_or(Malformed)?;
    if !reader.is_empty() {
        return Err(Malformed);
    }
    read_world(&mut Reader::new(bytes), true).ok_or(Malformed)
}

/// Encode one message, counting its length before allocating one writer.
/// Integers and lengths are big-endian; enum and option tags are one byte.
#[must_use]
pub fn encode_to_page(message: &ToPage) -> Box<[u8]> {
    let mut length = Encoding::Length(0);
    write_page(message, &mut length);
    let mut bytes = Encoding::Bytes(Writer::new(length.len()));
    write_page(message, &mut bytes);
    bytes.finish()
}

/// Validate the whole message before allocating any decoded containers.
/// Messages longer than `u32::MAX`, trailing bytes and invalid tags, lengths,
/// counts or UTF-8 are malformed. Payload allocations total at most the input
/// length; vectors have exactly the validated header count, each encoded
/// header costing at least eight bytes. The decoded heap is linear in input.
pub fn decode_to_page(bytes: &[u8]) -> Result<ToPage, Malformed> {
    u32::try_from(bytes.len()).map_err(|_| Malformed)?;
    let mut reader = Reader::new(bytes);
    read_page(&mut reader, false).ok_or(Malformed)?;
    if !reader.is_empty() {
        return Err(Malformed);
    }
    read_page(&mut Reader::new(bytes), true).ok_or(Malformed)
}

// Both passes follow the same fields: counting cannot drift from writing.
enum Encoding {
    Length(usize),
    Bytes(Writer),
}

impl Encoding {
    fn put(&mut self, bytes: &[u8]) {
        match self {
            Self::Length(length) => {
                *length = length.checked_add(bytes.len()).expect("message length fits");
            }
            Self::Bytes(writer) => writer.put(bytes).expect("the counted message fits"),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Length(length) => {
                assert!(u32::try_from(*length).is_ok(), "a bridge message fits u32");
                *length
            }
            Self::Bytes(writer) => writer.written(),
        }
    }

    fn finish(self) -> Box<[u8]> {
        match self {
            Self::Length(_) => unreachable!("only the writing pass finishes"),
            Self::Bytes(writer) => writer.finish(),
        }
    }

    fn tag(&mut self, tag: u8) {
        self.put(&[tag]);
    }

    fn u16(&mut self, value: u16) {
        self.put(&value.to_be_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.put(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.put(&value.to_be_bytes());
    }

    fn count(&mut self, count: usize) {
        self.u32(u32::try_from(count).expect("a bridge count fits u32"));
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.put(bytes);
    }
}

fn write_world(message: &ToWorld, out: &mut Encoding) {
    match message {
        ToWorld::Start(start) => {
            out.tag(0);
            out.bytes(&start.story);
            out.u64(start.seed);
            out.u64(start.start.as_nanos());
        }
        ToWorld::Duty(duty) => {
            out.tag(1);
            write_duty(duty, out);
        }
        ToWorld::Pace(speed) => {
            out.tag(2);
            out.u32(speed.0);
        }
    }
}

fn write_duty(duty: &Duty, out: &mut Encoding) {
    match duty {
        Duty::Exchange(exchange) => {
            out.tag(0);
            out.u32(exchange.id);
            out.bytes(&exchange.path);
            out.bytes(exchange.method.as_bytes());
            out.count(exchange.headers.len());
            for (name, value) in &exchange.headers {
                out.bytes(name.as_bytes());
                out.bytes(value.as_bytes());
            }
            out.bytes(&exchange.body);
            out.u32(exchange.bound);
        }
        Duty::Abandon { exchange } => {
            out.tag(1);
            out.u32(*exchange);
        }
        Duty::Stream(stream) => {
            out.tag(2);
            match stream {
                StreamDuty::Open { path, headers, bound } => {
                    out.tag(0);
                    out.bytes(path);
                    out.count(headers.len());
                    for (name, value) in headers {
                        out.bytes(name);
                        out.bytes(value);
                    }
                    out.u32(*bound);
                }
                StreamDuty::Close => out.tag(1),
            }
        }
        Duty::ReadOn => out.tag(3),
    }
}

fn write_page(message: &ToPage, out: &mut Encoding) {
    let clock = message.clock;
    out.u64(clock.step);
    out.u64(clock.now.as_nanos());
    out.u64(clock.wall.as_nanos());
    out.tag(u8::from(clock.idle));
    match clock.due {
        None => out.tag(0),
        Some(due) => {
            out.tag(1);
            out.u64(due.as_nanos());
        }
    }
    match &message.said {
        Said::Slice => out.tag(0),
        Said::Report(report) => {
            out.tag(1);
            write_report(report, out);
        }
        Said::Line(line) => {
            out.tag(2);
            match line.stream {
                Output::Standard => out.tag(0),
                Output::Error => out.tag(1),
            }
            out.bytes(&line.process);
            out.bytes(&line.line);
        }
        Said::Stamp(stamp) => {
            out.tag(3);
            out.u64(stamp.step);
            out.u64(stamp.instant.as_nanos());
        }
        Said::Stopped => out.tag(4),
        Said::Failed { words } => {
            out.tag(5);
            out.bytes(words);
        }
    }
}

fn write_report(report: &Report, out: &mut Encoding) {
    match report {
        Report::Answered { exchange, status, body } => {
            out.tag(0);
            out.u32(*exchange);
            out.u16(*status);
            out.bytes(body);
        }
        Report::Failed { exchange } => {
            out.tag(1);
            out.u32(*exchange);
        }
        Report::StreamOpened => out.tag(2),
        Report::StreamRefused { status } => {
            out.tag(3);
            out.u16(*status);
        }
        Report::StreamPiece { bytes } => {
            out.tag(4);
            out.bytes(bytes);
        }
        Report::StreamEnded => out.tag(5),
    }
}

// The validation pass constructs empty containers without allocating. Only
// after it consumes the whole message does the same parser copy payloads.
fn read_bytes(reader: &mut Reader<'_>, allocate: bool) -> Option<Box<[u8]>> {
    let length = reader.u32()?;
    let bytes = reader.bytes(length)?;
    Some(if allocate { bytes.into() } else { Box::from([]) })
}

fn read_string(reader: &mut Reader<'_>, allocate: bool) -> Option<String> {
    let length = reader.u32()?;
    let text = core::str::from_utf8(reader.bytes(length)?).ok()?;
    Some(if allocate { text.into() } else { String::new() })
}

fn header_count(reader: &mut Reader<'_>) -> Option<usize> {
    let count = reader.u32()?;
    // Even empty names and values each carry their four-byte length.
    if count > reader.remaining() / 8 {
        return None;
    }
    usize::try_from(count).ok()
}

fn read_string_headers(reader: &mut Reader<'_>, allocate: bool) -> Option<Vec<(String, String)>> {
    let count = header_count(reader)?;
    let mut headers = Vec::with_capacity(if allocate { count } else { 0 });
    for _ in 0..count {
        let name = read_string(reader, allocate)?;
        let value = read_string(reader, allocate)?;
        if allocate {
            headers.push((name, value));
        }
    }
    Some(headers)
}

fn read_headers(reader: &mut Reader<'_>, allocate: bool) -> Option<Vec<Header>> {
    let count = header_count(reader)?;
    let mut headers = Vec::with_capacity(if allocate { count } else { 0 });
    for _ in 0..count {
        let name = read_bytes(reader, allocate)?;
        let value = read_bytes(reader, allocate)?;
        if allocate {
            headers.push((name, value));
        }
    }
    Some(headers)
}

fn read_world(reader: &mut Reader<'_>, allocate: bool) -> Option<ToWorld> {
    match reader.u8()? {
        0 => Some(ToWorld::Start(Start {
            story: read_bytes(reader, allocate)?,
            seed: reader.u64()?,
            start: Wall::from_nanos(reader.u64()?),
        })),
        1 => Some(ToWorld::Duty(read_duty(reader, allocate)?)),
        2 => Some(ToWorld::Pace(Speed(reader.u32()?))),
        _ => None,
    }
}

fn read_duty(reader: &mut Reader<'_>, allocate: bool) -> Option<Duty> {
    match reader.u8()? {
        0 => Some(Duty::Exchange(Exchange {
            id: reader.u32()?,
            path: read_bytes(reader, allocate)?,
            method: read_string(reader, allocate)?,
            headers: read_string_headers(reader, allocate)?,
            body: read_bytes(reader, allocate)?,
            bound: reader.u32()?,
        })),
        1 => Some(Duty::Abandon { exchange: reader.u32()? }),
        2 => Some(Duty::Stream(match reader.u8()? {
            0 => StreamDuty::Open {
                path: read_bytes(reader, allocate)?,
                headers: read_headers(reader, allocate)?,
                bound: reader.u32()?,
            },
            1 => StreamDuty::Close,
            _ => return None,
        })),
        3 => Some(Duty::ReadOn),
        _ => None,
    }
}

fn read_page(reader: &mut Reader<'_>, allocate: bool) -> Option<ToPage> {
    let clock = Clock {
        step: reader.u64()?,
        now: Time::from_nanos(reader.u64()?),
        wall: Wall::from_nanos(reader.u64()?),
        idle: match reader.u8()? {
            0 => false,
            1 => true,
            _ => return None,
        },
        due: match reader.u8()? {
            0 => None,
            1 => Some(Time::from_nanos(reader.u64()?)),
            _ => return None,
        },
    };
    let said = match reader.u8()? {
        0 => Said::Slice,
        1 => Said::Report(read_report(reader, allocate)?),
        2 => Said::Line(Line {
            stream: match reader.u8()? {
                0 => Output::Standard,
                1 => Output::Error,
                _ => return None,
            },
            process: read_bytes(reader, allocate)?,
            line: read_bytes(reader, allocate)?,
        }),
        3 => Said::Stamp(Stamp { step: reader.u64()?, instant: Time::from_nanos(reader.u64()?) }),
        4 => Said::Stopped,
        5 => Said::Failed { words: read_bytes(reader, allocate)? },
        _ => return None,
    };
    Some(ToPage { clock, said })
}

fn read_report(reader: &mut Reader<'_>, allocate: bool) -> Option<Report> {
    match reader.u8()? {
        0 => Some(Report::Answered {
            exchange: reader.u32()?,
            status: reader.u16()?,
            body: read_bytes(reader, allocate)?,
        }),
        1 => Some(Report::Failed { exchange: reader.u32()? }),
        2 => Some(Report::StreamOpened),
        3 => Some(Report::StreamRefused { status: reader.u16()? }),
        4 => Some(Report::StreamPiece { bytes: read_bytes(reader, allocate)? }),
        5 => Some(Report::StreamEnded),
        _ => None,
    }
}
