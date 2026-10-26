//! Bounded process output kept as whole lines for the world's loop (world.md, 3).

use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::cell::RefCell;
use std::io::{self, Write};

/// The most unread process output, including newlines, kept by a world.
pub const PRINTED_BYTES: usize = 64 * 1024;

/// Which of a process's outputs a line was printed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Output,
    Error,
}

/// A whole line a process printed, without its newline.
#[derive(Debug, PartialEq, Eq)]
pub struct Printed {
    pub process: Box<[u8]>,
    pub stream: Stream,
    pub line: Box<[u8]>,
}

#[derive(Debug)]
struct Source {
    process: Box<[u8]>,
    stream: Stream,
    partial: Vec<u8>,
}

#[derive(Debug)]
struct Unread {
    bytes: usize,
    sources: Vec<Source>,
    lines: Vec<Printed>,
}

#[derive(Debug)]
pub(crate) struct Printing(Rc<RefCell<Unread>>);

impl Printing {
    pub(crate) fn new() -> Self {
        Self(Rc::new(RefCell::new(Unread { bytes: 0, sources: Vec::new(), lines: Vec::new() })))
    }

    pub(crate) fn printer(&mut self, process: &[u8], stream: Stream) -> Printer {
        let mut unread = self.0.borrow_mut();
        // Printers for the same output share its unfinished line.
        let source = unread
            .sources
            .iter()
            .position(|source| &*source.process == process && source.stream == stream)
            .unwrap_or_else(|| {
                let at = unread.sources.len();
                unread.sources.push(Source { process: Box::from(process), stream, partial: Vec::new() });
                at
            });
        Printer { unread: self.0.clone(), source }
    }

    pub(crate) fn printed(&mut self) -> Vec<Printed> {
        let mut unread = self.0.borrow_mut();
        let lines = core::mem::take(&mut unread.lines);
        for line in &lines {
            let bytes = line.line.len().checked_add(1).expect("line and newline fit the bound");
            unread.bytes = unread.bytes.checked_sub(bytes).expect("taken lines were unread");
        }
        lines
    }
}

/// A process output kept by the world, at most `PRINTED_BYTES` unread.
/// A full backlog returns `WouldBlock`, as a pipe's does.
#[derive(Clone, Debug)]
pub struct Printer {
    unread: Rc<RefCell<Unread>>,
    source: usize,
}

impl Write for Printer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut unread = self.unread.borrow_mut();
        let room = PRINTED_BYTES.checked_sub(unread.bytes).expect("bounded unread output");
        let count = bytes.len().min(room);
        if count == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        for byte in bytes.iter().take(count) {
            let source = unread.sources.get_mut(self.source).expect("a printer's source");
            if *byte == b'\n' {
                let line = Printed {
                    process: source.process.clone(),
                    stream: source.stream,
                    line: core::mem::take(&mut source.partial).into_boxed_slice(),
                };
                unread.lines.push(line);
            } else {
                source.partial.push(*byte);
            }
        }
        unread.bytes = unread.bytes.checked_add(count).expect("bounded unread output");
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
