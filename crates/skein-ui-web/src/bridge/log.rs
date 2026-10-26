//! The page's versioned session log (playground.md, 6).
//!
//! Records end in LF and fields have one space between them. Integers are
//! canonical decimal, digests lowercase hex. Nonempty byte fields use
//! canonical padded standard base64; `=` alone denotes every empty byte
//! field, including an empty header list. Blank fields are refused.
//! Headers encode `name: value\n` records; names contain neither colon nor
//! LF, values contain no LF. Exchange headers are UTF-8.
//!
//! The fold hashes `report answered <id> <status> <body>\n`,
//! `report failed <id>\n`, `report stream-opened\n`,
//! `report stream-refused <status>\n`, `report stream-piece <bytes>\n`,
//! `report stream-ended\n`, `line <out|err> <process> <line>\n`, and
//! `stopped\n`, in order. Byte fields follow the same base64 rule.

use super::{Duty, Line, Output, Said, Stamp, Start};
use crate::{fetch::Exchange, fetch::Report, transport::Header, transport::StreamDuty};
use skein_lib::{Decimal, Time, Wall, Writer as BytesWriter, base64, base64::Alphabet, sha256::Sha256};
use std::collections::VecDeque;

/// The format's name and version, its first line's first two words.
pub const FORMAT: &str = "skein-playground-log";
pub const VERSION: u32 = 1;

// The longer terminal, `end <64 hex digits>\n`; `cut\n` fits within it.
const TERMINAL: usize = 69;
// The shortest stamp prefix and the digest suffix: `0 0 ` + ` <digest>\n`.
const INPUT_OVERHEAD: usize = 70;

/// What the page writes as the visit goes (playground.md, 6).
pub struct Writer {
    lines: Vec<u8>,
    pending: VecDeque<Box<[u8]>>,
    pending_bytes: usize,
    pending_cut: bool,
    fold: Fold,
    bound: usize,
    cut: bool,
}

impl Writer {
    /// A visit's start and the bundle's visible ASCII digest.
    ///
    /// Panics unless `bound` holds the start line plus the 69-byte terminal
    /// reserve. Methods and build names must be nonempty visible ASCII;
    /// headers must have the representation described by this module.
    #[must_use]
    pub fn new(build: &[u8], start: &Start, bound: u32) -> Self {
        assert!(visible(build), "the bundle digest is nonempty visible ASCII");
        let mut length = Text::Length(0);
        write_start(build, start, &mut length);
        let size = length.len();
        let bound = usize::try_from(bound).expect("a log bound fits usize");
        assert!(
            size.checked_add(TERMINAL).expect("the start and terminal fit usize") <= bound,
            "the log bound holds the start line and the terminal reserve"
        );
        let mut text = Text::Bytes(BytesWriter::new(size));
        write_start(build, start, &mut text);
        Self {
            lines: text.finish().into(),
            pending: VecDeque::new(),
            pending_bytes: 0,
            pending_cut: false,
            fold: Fold::default(),
            bound,
            cut: false,
        }
    }

    /// A duty sent to the world; its line is written at its stamp.
    pub fn sent(&mut self, duty: &Duty) {
        if self.cut || self.pending_cut {
            return;
        }
        let mut length = Text::Length(0);
        write_duty(duty, &mut length);
        let size = length.len();
        let cost = size.checked_add(INPUT_OVERHEAD).expect("an input's length fits usize");
        let room = (self.bound - TERMINAL - self.lines.len()).saturating_sub(self.pending_bytes);
        if cost > room {
            // Keep the cut at this duty's stamp, after all earlier stamps.
            // This duty will cut the log, so later duties need no storage.
            self.pending_cut = true;
            return;
        }
        let mut text = Text::Bytes(BytesWriter::new(size));
        write_duty(duty, &mut text);
        self.pending.push_back(text.finish());
        self.pending_bytes += cost;
    }

    /// Fold the world's reports, lines and stop. A stamp writes the oldest
    /// unstamped duty with the digest since the previous input. Slice ends
    /// and failures are ignored. A stamp without a sent duty is a caller bug.
    pub fn said(&mut self, said: &Said) {
        if self.cut {
            return;
        }
        match said {
            Said::Stamp(stamp) => {
                let Some(duty) = self.pending.pop_front() else {
                    assert!(self.pending_cut, "a stamp pairs with a duty sent");
                    self.cut();
                    return;
                };
                self.pending_bytes -= duty.len() + INPUT_OVERHEAD;
                let mut length = Text::Length(0);
                write_input(*stamp, &duty, &[0; 32], &mut length);
                let size = length.len();
                if size > self.bound - TERMINAL - self.lines.len() {
                    self.cut();
                    return;
                }
                let digest = self.fold.take();
                let mut text = Text::Bytes(BytesWriter::new(size));
                write_input(*stamp, &duty, &digest, &mut text);
                self.lines.extend_from_slice(&text.finish());
                // The stamp may have consumed more than its shortest form.
                // Pending payloads remain bounded by the configured bound.
            }
            Said::Report(_) | Said::Line(_) | Said::Stopped | Said::Slice | Said::Failed { words: _ } => {
                self.fold.said(said);
            }
        }
    }

    /// The recorded lines followed by `end` and the current digest, or `cut`.
    /// The terminal is included in the bound. Taking a file changes no state.
    #[must_use]
    pub fn file(&self) -> Box<[u8]> {
        let terminal = if self.cut { 4 } else { TERMINAL };
        let mut out = BytesWriter::new(self.lines.len() + terminal);
        out.put(&self.lines).expect("the retained lines fit");
        if self.cut {
            out.put(b"cut\n").expect("the cut fits its terminal reserve");
        } else {
            let mut text = Text::Bytes(out);
            text.put(b"end ");
            text.hex(&self.fold.hash.clone().finish());
            text.put(b"\n");
            return text.finish();
        }
        out.finish()
    }

    fn cut(&mut self) {
        self.cut = true;
        self.pending.clear();
        self.pending_bytes = 0;
        self.pending_cut = false;
        self.fold = Fold::default();
    }
}

/// A log read back. Its build is evidence, not a condition of reading it.
#[derive(Debug, PartialEq, Eq)]
pub struct Log {
    pub build: Box<[u8]>,
    pub start: Start,
    pub inputs: Vec<Input>,
    pub end: End,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Input {
    pub stamp: Stamp,
    pub duty: Duty,
    /// What the world said since the input before (the start, for the first).
    pub digest: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// What the world said after the last input.
    Said([u8; 32]),
    /// The log stopped at its bound: a replay runs to here.
    Cut,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not this format or version: what its first line names.
    Format { named: Box<[u8]> },
    /// Line `line` (from 1) is not the format's.
    Malformed { line: u32 },
}

/// The digest of what was said, folded as the writer folds it.
pub struct Fold {
    hash: Sha256,
}

impl Default for Fold {
    fn default() -> Self {
        Self { hash: Sha256::new() }
    }
}

impl Fold {
    /// A report, a line or the stop folded; anything else is not.
    pub fn said(&mut self, said: &Said) {
        let mut out = Text::Hash(&mut self.hash);
        match said {
            Said::Report(report) => {
                out.put(b"report ");
                match report {
                    Report::Answered { exchange, status, body } => {
                        out.put(b"answered ");
                        out.number(u64::from(*exchange));
                        out.put(b" ");
                        out.number(u64::from(*status));
                        out.put(b" ");
                        out.encoded(body);
                    }
                    Report::Failed { exchange } => {
                        out.put(b"failed ");
                        out.number(u64::from(*exchange));
                    }
                    Report::StreamOpened => out.put(b"stream-opened"),
                    Report::StreamRefused { status } => {
                        out.put(b"stream-refused ");
                        out.number(u64::from(*status));
                    }
                    Report::StreamPiece { bytes } => {
                        out.put(b"stream-piece ");
                        out.encoded(bytes);
                    }
                    Report::StreamEnded => out.put(b"stream-ended"),
                }
            }
            Said::Line(Line { stream, process, line }) => {
                out.put(match stream {
                    Output::Standard => b"line out ",
                    Output::Error => b"line err ",
                });
                out.encoded(process);
                out.put(b" ");
                out.encoded(line);
            }
            Said::Stopped => out.put(b"stopped"),
            Said::Slice | Said::Stamp(_) | Said::Failed { words: _ } => return,
        }
        out.put(b"\n");
    }

    /// The digest so far, and a new fold from here.
    pub fn take(&mut self) -> [u8; 32] {
        core::mem::replace(&mut self.hash, Sha256::new()).finish()
    }
}

// The same serialization measures, writes, or folds a canonical record.
enum Text<'a> {
    Length(usize),
    Bytes(BytesWriter),
    Hash(&'a mut Sha256),
}

impl Text<'_> {
    fn put(&mut self, bytes: &[u8]) {
        match self {
            Self::Length(length) => *length = length.checked_add(bytes.len()).expect("text length fits usize"),
            Self::Bytes(writer) => writer.put(bytes).expect("the measured text fits"),
            Self::Hash(hash) => hash.update(bytes),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Length(length) => *length,
            Self::Bytes(_) | Self::Hash(_) => unreachable!("only a measuring pass asks for its length"),
        }
    }

    fn finish(self) -> Box<[u8]> {
        match self {
            Self::Bytes(writer) => writer.finish(),
            Self::Length(_) | Self::Hash(_) => unreachable!("only a writing pass finishes"),
        }
    }

    fn number(&mut self, value: u64) {
        self.put(Decimal::of(value).as_bytes());
    }

    fn encoded_size(&mut self, length: usize) -> bool {
        match self {
            Self::Length(size) => {
                let length =
                    if length == 0 { 1 } else { length.div_ceil(3).checked_mul(4).expect("base64 length fits usize") };
                *size = size.checked_add(length).expect("text length fits usize");
                true
            }
            Self::Bytes(_) | Self::Hash(_) => false,
        }
    }

    fn encoded(&mut self, bytes: &[u8]) {
        if self.encoded_size(bytes.len()) {
            return;
        }
        if bytes.is_empty() {
            self.put(b"=");
            return;
        }
        // Multiples of three leave padding only at the final piece. Folding
        // needs at most 1024 temporary bytes, even for a large report body.
        for piece in bytes.chunks(768) {
            let length =
                base64::encoded_len(Alphabet::Standard, u32::try_from(piece.len()).expect("a base64 piece fits u32"))
                    .expect("an encoded piece fits u32");
            let mut writer = BytesWriter::new(usize::try_from(length).expect("a piece fits usize"));
            base64::encode(Alphabet::Standard, piece, &mut writer).expect("the measured base64 fits");
            self.put(&writer.finish());
        }
    }

    fn hex(&mut self, digest: &[u8; 32]) {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in digest {
            self.put(&[DIGITS[usize::from(byte >> 4)], DIGITS[usize::from(byte & 15)]]);
        }
    }

    fn headers<'a>(&mut self, headers: impl Iterator<Item = (&'a [u8], &'a [u8])> + Clone) {
        let mut size = 0_usize;
        for (name, value) in headers.clone() {
            assert!(!name.contains(&b':') && !name.contains(&b'\n'), "header names contain neither colon nor LF");
            assert!(!value.contains(&b'\n'), "header values contain no LF");
            size = size
                .checked_add(name.len())
                .and_then(|size| size.checked_add(value.len()))
                .and_then(|size| size.checked_add(3))
                .expect("header text fits usize");
        }
        if self.encoded_size(size) {
            return;
        }
        let mut writer = BytesWriter::new(size);
        for (name, value) in headers {
            writer.put(name).expect("measured header fits");
            writer.put(b": ").expect("measured separator fits");
            writer.put(value).expect("measured value fits");
            writer.put(b"\n").expect("measured line ending fits");
        }
        self.encoded(&writer.finish());
    }
}

fn write_start(build: &[u8], start: &Start, out: &mut Text<'_>) {
    out.put(FORMAT.as_bytes());
    out.put(b" ");
    out.number(u64::from(VERSION));
    out.put(b" ");
    out.put(build);
    out.put(b" ");
    out.encoded(&start.story);
    out.put(b" ");
    out.number(start.seed);
    out.put(b" ");
    out.number(start.start.as_nanos());
    out.put(b"\n");
}

fn write_duty(duty: &Duty, out: &mut Text<'_>) {
    match duty {
        Duty::Exchange(exchange) => {
            assert!(visible(exchange.method.as_bytes()), "an exchange method is nonempty visible ASCII");
            out.put(b"exchange ");
            out.number(u64::from(exchange.id));
            out.put(b" ");
            out.put(exchange.method.as_bytes());
            out.put(b" ");
            out.encoded(&exchange.path);
            out.put(b" ");
            out.headers(exchange.headers.iter().map(|(name, value)| (name.as_bytes(), value.as_bytes())));
            out.put(b" ");
            out.encoded(&exchange.body);
            out.put(b" ");
            out.number(u64::from(exchange.bound));
        }
        Duty::Abandon { exchange } => {
            out.put(b"abandon ");
            out.number(u64::from(*exchange));
        }
        Duty::Stream(stream) => match stream {
            StreamDuty::Open { path, headers, bound } => {
                out.put(b"open ");
                out.encoded(path);
                out.put(b" ");
                out.headers(headers.iter().map(|(name, value)| (name.as_ref(), value.as_ref())));
                out.put(b" ");
                out.number(u64::from(*bound));
            }
            StreamDuty::Close => out.put(b"close"),
        },
        Duty::ReadOn => out.put(b"read-on"),
    }
}

fn write_input(stamp: Stamp, duty: &[u8], digest: &[u8; 32], out: &mut Text<'_>) {
    out.number(stamp.step);
    out.put(b" ");
    out.number(stamp.instant.as_nanos());
    out.put(b" ");
    out.put(duty);
    out.put(b" ");
    out.hex(digest);
    out.put(b"\n");
}

fn visible(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(|byte| (b'!'..=b'~').contains(byte))
}

/// Read a complete log, refusing the first malformed line. The build is
/// preserved without comparison: native replays use their digest checks.
pub fn read(bytes: &[u8]) -> Result<Log, Refused> {
    u32::try_from(bytes.len()).map_err(|_| Refused::Malformed { line: 1 })?;
    // Validate the entire file before allocating decoded containers, then
    // size each vector to its validated count. Heap use is linear in input.
    let (_, count) = parse(bytes, false, 0)?;
    parse(bytes, true, count).map(|(log, _)| log)
}

fn parse(bytes: &[u8], allocate: bool, count: usize) -> Result<(Log, usize), Refused> {
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
    let first = lines.next().unwrap_or(b"");
    let first_text = first.strip_suffix(b"\n").unwrap_or(first);
    let mut words = first_text.split(u8::is_ascii_whitespace).filter(|word| !word.is_empty());
    let name = words.next().unwrap_or(b"");
    let version = words.next().unwrap_or(b"");
    if name != FORMAT.as_bytes() || version != Decimal::of(u64::from(VERSION)).as_bytes() {
        let mut named = name.to_vec();
        if !version.is_empty() {
            named.push(b' ');
            named.extend_from_slice(version);
        }
        return Err(Refused::Format { named: named.into_boxed_slice() });
    }
    let mut first_fields = fields(first).ok_or(Refused::Malformed { line: 1 })?;
    let (build, start) = start_fields(&mut first_fields, allocate).ok_or(Refused::Malformed { line: 1 })?;
    let mut inputs = Vec::with_capacity(if allocate { count } else { 0 });
    let mut count = 0;
    let mut end = None;
    let mut line = 1_u32;
    for record in lines {
        line = line.checked_add(1).expect("line count fits the input's u32 length");
        let malformed = Refused::Malformed { line };
        if end.is_some() {
            return Err(malformed);
        }
        let mut fields = fields(record).ok_or(Refused::Malformed { line })?;
        match fields.next() {
            Some(b"end") => {
                let digest = digest(fields.next()).ok_or(Refused::Malformed { line })?;
                if fields.next().is_some() {
                    return Err(malformed);
                }
                end = Some(End::Said(digest));
            }
            Some(b"cut") => {
                if fields.next().is_some() {
                    return Err(malformed);
                }
                end = Some(End::Cut);
            }
            Some(step) => {
                let input = input_fields(step, &mut fields, allocate).ok_or(malformed)?;
                count += 1;
                if allocate {
                    inputs.push(input);
                }
            }
            None => return Err(malformed),
        }
    }
    let end = end.ok_or(Refused::Malformed { line: line.saturating_add(1) })?;
    Ok((Log { build, start, inputs, end }, count))
}

fn fields(record: &[u8]) -> Option<std::slice::Split<'_, u8, impl FnMut(&u8) -> bool>> {
    let line = record.strip_suffix(b"\n")?;
    if line.is_empty() || line.split(|byte| *byte == b' ').any(|field| !visible(field)) {
        return None;
    }
    Some(line.split(|byte| *byte == b' '))
}

fn start_fields<'a>(fields: &mut impl Iterator<Item = &'a [u8]>, allocate: bool) -> Option<(Box<[u8]>, Start)> {
    if fields.next()? != FORMAT.as_bytes() || fields.next()? != b"1" {
        return None;
    }
    let build = fields.next()?;
    let story = decoded(fields.next()?, allocate)?;
    let seed = integer(fields.next()?)?;
    let start = Wall::from_nanos(integer(fields.next()?)?);
    if fields.next().is_some() {
        return None;
    }
    Some((if allocate { build.into() } else { Box::from([]) }, Start { story, seed, start }))
}

fn input_fields<'a>(step: &[u8], fields: &mut impl Iterator<Item = &'a [u8]>, allocate: bool) -> Option<Input> {
    let step = integer(step)?;
    let instant = Time::from_nanos(integer(fields.next()?)?);
    let duty = match fields.next()? {
        b"exchange" => {
            let id = u32::try_from(integer(fields.next()?)?).ok()?;
            let method = fields.next()?;
            let path = decoded(fields.next()?, allocate)?;
            let headers = read_headers(fields.next()?, true, allocate)?;
            let body = decoded(fields.next()?, allocate)?;
            let bound = u32::try_from(integer(fields.next()?)?).ok()?;
            Duty::Exchange(Exchange {
                id,
                method: if allocate { String::from_utf8(method.into()).ok()? } else { String::new() },
                path,
                headers: {
                    let mut strings = Vec::with_capacity(headers.len());
                    for (name, value) in headers {
                        strings.push((String::from_utf8(name.into()).ok()?, String::from_utf8(value.into()).ok()?));
                    }
                    strings
                },
                body,
                bound,
            })
        }
        b"abandon" => Duty::Abandon { exchange: u32::try_from(integer(fields.next()?)?).ok()? },
        b"open" => Duty::Stream(StreamDuty::Open {
            path: decoded(fields.next()?, allocate)?,
            headers: read_headers(fields.next()?, false, allocate)?,
            bound: u32::try_from(integer(fields.next()?)?).ok()?,
        }),
        b"close" => Duty::Stream(StreamDuty::Close),
        b"read-on" => Duty::ReadOn,
        _ => return None,
    };
    let digest = digest(fields.next())?;
    if fields.next().is_some() {
        return None;
    }
    Some(Input { stamp: Stamp { step, instant }, duty, digest })
}

fn integer(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || (bytes.len() > 1 && bytes.first() == Some(&b'0')) {
        return None;
    }
    let mut number = 0_u64;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number.checked_mul(10)?.checked_add(u64::from(byte.checked_sub(b'0')?))?;
    }
    Some(number)
}

fn decoded(text: &[u8], allocate: bool) -> Option<Box<[u8]>> {
    if text == b"=" {
        return Some(Box::from([]));
    }
    if text.is_empty() {
        return None;
    }
    let length = base64::decoded_len(Alphabet::Standard, text).ok()?;
    if allocate { base64::decode(Alphabet::Standard, text, length).ok() } else { Some(Box::from([])) }
}

fn read_headers(text: &[u8], utf8: bool, allocate: bool) -> Option<Vec<Header>> {
    let count = header_count(text, utf8)?;
    let mut headers = Vec::with_capacity(if allocate { count } else { 0 });
    if allocate {
        let bytes = decoded(text, true)?;
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            let (name, value) = header(line).expect("the header records are validated");
            headers.push((name.into(), value.into()));
        }
    }
    Some(headers)
}

// The library validates base64's alphabet, padding and trailing bits. Walk
// its decoded bytes without a buffer to validate the header envelope and
// UTF-8 before the allocating pass, including incomplete code points at a
// field boundary. There is no allocation based on an encoded count.
fn header_count(text: &[u8], utf8: bool) -> Option<usize> {
    if text == b"=" {
        return Some(0);
    }
    if text.is_empty() {
        return None;
    }
    base64::decoded_len(Alphabet::Standard, text).ok()?;
    let mut check = HeaderCheck { field: HeaderField::Name, started: false, count: 0, character: [0; 4], used: 0 };
    for [a, b, c, d] in text.as_chunks::<4>().0 {
        check.byte((sextet(*a) << 2) | (sextet(*b) >> 4), utf8)?;
        if *c != b'=' {
            check.byte(((sextet(*b) & 15) << 4) | (sextet(*c) >> 2), utf8)?;
        }
        if *d != b'=' {
            check.byte(((sextet(*c) & 3) << 6) | sextet(*d), utf8)?;
        }
    }
    if check.started { None } else { Some(check.count) }
}

fn sextet(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => unreachable!("the base64 alphabet is validated before decoding a sextet"),
    }
}

enum HeaderField {
    Name,
    Space,
    Value,
}

struct HeaderCheck {
    field: HeaderField,
    started: bool,
    count: usize,
    character: [u8; 4],
    used: usize,
}

impl HeaderCheck {
    fn byte(&mut self, byte: u8, utf8: bool) -> Option<()> {
        self.started = true;
        match self.field {
            HeaderField::Name => match byte {
                b':' => {
                    if self.used != 0 {
                        return None;
                    }
                    self.field = HeaderField::Space;
                }
                b'\n' => return None,
                _ => self.character(byte, utf8)?,
            },
            HeaderField::Space => {
                if byte != b' ' {
                    return None;
                }
                self.field = HeaderField::Value;
            }
            HeaderField::Value => {
                if byte == b'\n' {
                    if self.used != 0 {
                        return None;
                    }
                    self.field = HeaderField::Name;
                    self.started = false;
                    self.count += 1;
                } else {
                    self.character(byte, utf8)?;
                }
            }
        }
        Some(())
    }

    fn character(&mut self, byte: u8, utf8: bool) -> Option<()> {
        if utf8 {
            *self.character.get_mut(self.used)? = byte;
            self.used += 1;
            match core::str::from_utf8(self.character.get(..self.used)?) {
                Ok(_) => self.used = 0,
                Err(error) => {
                    if error.error_len().is_some() {
                        return None;
                    }
                }
            }
        }
        Some(())
    }
}

fn header(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let line = line.strip_suffix(b"\n")?;
    let colon = line.iter().position(|byte| *byte == b':')?;
    let (name, rest) = line.split_at_checked(colon)?;
    let value = rest.strip_prefix(b": ")?;
    Some((name, value))
}

fn digest(text: Option<&[u8]>) -> Option<[u8; 32]> {
    let text = text?;
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0; 32];
    for (slot, pair) in digest.iter_mut().zip(text.chunks_exact(2)) {
        *slot = nibble(pair[0])?.checked_mul(16)?.checked_add(nibble(pair[1])?)?;
    }
    Some(digest)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte.checked_sub(b'a')?.checked_add(10),
        _ => None,
    }
}
