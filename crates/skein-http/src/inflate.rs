//! Bounded gzip and raw DEFLATE decoding (RFC 1951 and RFC 1952).
//! Malformed streams and output past the caller's capacity are refused.

use skein_lib::List;

/// A member's first four bytes: gzip, DEFLATE, no flags. Ten bytes of
/// header in all; the encoder sends no optional fields.
const MAGIC: [u8; 4] = [0x1f, 0x8b, 8, 0];
const HEADER: usize = 10;

/// The order in which a block's header sends the widths of its width code
/// (RFC 1951, 3.2.7).
const ORDER: [u8; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Each length symbol from 257: its base and extra bits (RFC 1951, 3.2.5).
const LENGTHS: [(u16, u8); 29] = [
    (3, 0),
    (4, 0),
    (5, 0),
    (6, 0),
    (7, 0),
    (8, 0),
    (9, 0),
    (10, 0),
    (11, 1),
    (13, 1),
    (15, 1),
    (17, 1),
    (19, 2),
    (23, 2),
    (27, 2),
    (31, 2),
    (35, 3),
    (43, 3),
    (51, 3),
    (59, 3),
    (67, 4),
    (83, 4),
    (99, 4),
    (115, 4),
    (131, 5),
    (163, 5),
    (195, 5),
    (227, 5),
    (258, 0),
];

/// Each distance symbol: its base and extra bits (RFC 1951, 3.2.5).
const DISTANCES: [(u16, u8); 30] = [
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 6),
    (193, 6),
    (257, 7),
    (385, 7),
    (513, 8),
    (769, 8),
    (1025, 9),
    (1537, 9),
    (2049, 10),
    (3073, 10),
    (4097, 11),
    (6145, 11),
    (8193, 12),
    (12289, 12),
    (16385, 13),
    (24577, 13),
];

/// The most symbols one code holds: 286 literal/length and 30 distance
/// widths are sent together, and a width code has 19.
const SYMBOLS: usize = 320;

/// Inflates one gzip member into `out`, clearing it first. Flags, trailing
/// bytes, a bad CRC or size, and output past its capacity return `None`.
pub fn gzip(member: &[u8], out: &mut List<u8>) -> Option<()> {
    out.clear();
    if member.get(..4)? != MAGIC.as_slice() {
        return None;
    }
    let data = member.get(HEADER..)?;
    let at = blocks(data, out)?;
    let trailer = data.get(at..)?;
    let check = <[u8; 4]>::try_from(trailer.get(..4)?).ok()?;
    let size = <[u8; 4]>::try_from(trailer.get(4..)?).ok()?;
    if u32::from_le_bytes(check) != crc(out.as_slice()) || u32::from_le_bytes(size) != out.len() {
        return None;
    }
    Some(())
}

/// Inflates exactly one raw DEFLATE stream, clearing `out` first.
/// Its fixed capacity is the output limit; no trailing bytes are accepted.
pub fn deflate(data: &[u8], out: &mut List<u8>) -> Option<()> {
    out.clear();
    let end = blocks(data, out)?;
    if end != data.len() {
        return None;
    }
    Some(())
}

fn blocks(data: &[u8], out: &mut List<u8>) -> Option<usize> {
    let mut input = Input { bytes: data, bit: 0 };
    let mut last = false;
    // Each block takes at least three bits.
    for _block in 0..=data.len() {
        if last {
            break;
        }
        last = input.bits(1)? == 1;
        match input.bits(2)? {
            0 => stored(&mut input, out)?,
            1 => {
                let (literals, distances) = fixed()?;
                codes(&mut input, out, &literals, &distances)?;
            }
            2 => {
                let (literals, distances) = header(&mut input)?;
                codes(&mut input, out, &literals, &distances)?;
            }
            _ => return None,
        }
    }
    if !last {
        return None;
    }
    input.align()
}

/// A member's DEFLATE data read bit by bit, each byte's lowest bit first
/// (RFC 1951, 3.1.1).
struct Input<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl Input<'_> {
    /// `width` bits, at most 16, the first lowest.
    fn bits(&mut self, width: u32) -> Option<u32> {
        let mut value = 0_u32;
        for index in 0..width.min(16) {
            let byte = *self.bytes.get(self.bit >> 3_u32)?;
            let bit = u32::from(byte.checked_shr(u32::try_from(self.bit & 7).ok()?)? & 1);
            value |= bit.checked_shl(index)?;
            self.bit = self.bit.checked_add(1)?;
        }
        Some(value)
    }

    /// Skips to the next byte's start, and returns where that is.
    fn align(&mut self) -> Option<usize> {
        let at = self.bit.div_ceil(8);
        self.bit = at.checked_mul(8)?;
        Some(at)
    }
}

/// A canonical Huffman code (RFC 1951, 3.2.2), for decoding: how many codes
/// there are of each width, and the symbols in the codes' order.
struct Code {
    counts: [u16; 16],
    symbols: [u16; SYMBOLS],
}

/// The code whose symbols have these widths, 0 for a symbol unused; an
/// oversubscribed code is refused, and an incomplete one fails only on a
/// string that starts no code.
fn code(widths: &[u8]) -> Option<Code> {
    let mut code = Code { counts: [0; 16], symbols: [0; SYMBOLS] };
    for width in widths {
        let count = code.counts.get_mut(usize::from(*width))?;
        *count = count.checked_add(1)?;
    }
    *code.counts.get_mut(0)? = 0;
    let mut left = 1_i32;
    let mut offsets = [0_u16; 16];
    for width in 1..16_usize {
        let count = *code.counts.get(width)?;
        left = left.checked_mul(2)?.checked_sub(i32::from(count))?;
        if left < 0_i32 {
            return None;
        }
        if let Some(next) = offsets.get(width.checked_add(1)?).copied() {
            *offsets.get_mut(width.checked_add(1)?)? = next.checked_add(offsets.get(width)?.checked_add(count)?)?;
        }
    }
    for (symbol, width) in widths.iter().enumerate() {
        if *width != 0 {
            let offset = offsets.get_mut(usize::from(*width))?;
            *code.symbols.get_mut(usize::from(*offset))? = u16::try_from(symbol).ok()?;
            *offset = offset.checked_add(1)?;
        }
    }
    Some(code)
}

impl Code {
    /// The next symbol, its code read one bit at a time, most significant
    /// first.
    fn decode(&self, input: &mut Input<'_>) -> Option<u16> {
        let mut value = 0_u32;
        let mut first = 0_u32;
        let mut index = 0_u32;
        for count in self.counts.get(1..)? {
            value |= input.bits(1)?;
            let count = u32::from(*count);
            if value < first.checked_add(count)? {
                let at = index.checked_add(value.checked_sub(first)?)?;
                return self.symbols.get(usize::try_from(at).ok()?).copied();
            }
            index = index.checked_add(count)?;
            first = first.checked_add(count)?.checked_shl(1)?;
            value = value.checked_shl(1)?;
        }
        None
    }
}

/// A stored block (RFC 1951, 3.2.4): its length, that length's complement,
/// and its bytes.
fn stored(input: &mut Input<'_>, out: &mut List<u8>) -> Option<()> {
    let at = input.align()?;
    let [a, b, c, d] = <[u8; 4]>::try_from(input.bytes.get(at..at.checked_add(4)?)?).ok()?;
    let length = u16::from_le_bytes([a, b]);
    if length != !u16::from_le_bytes([c, d]) {
        return None;
    }
    let start = at.checked_add(4)?;
    let end = start.checked_add(usize::from(length))?;
    for byte in input.bytes.get(start..end)? {
        out.push(*byte).ok()?;
    }
    input.bit = end.checked_mul(8)?;
    Some(())
}

/// The fixed literal/length and distance codes (RFC 1951, 3.2.6).
fn fixed() -> Option<(Code, Code)> {
    let mut widths = [0_u8; 288];
    for (symbol, width) in widths.iter_mut().enumerate() {
        *width = match symbol {
            0..=143 | 280.. => 8,
            144..=255 => 9,
            _ => 7,
        };
    }
    Some((code(&widths)?, code(&[5; 30])?))
}

/// A block's header in its own codes (RFC 1951, 3.2.7): its literal/length
/// and distance codes.
fn header(input: &mut Input<'_>) -> Option<(Code, Code)> {
    let literals = usize::try_from(input.bits(5)?.checked_add(257)?).ok()?;
    let distances = usize::try_from(input.bits(5)?.checked_add(1)?).ok()?;
    let sent = usize::try_from(input.bits(4)?.checked_add(4)?).ok()?;
    if literals > 286 || distances > 30 {
        return None;
    }
    let mut widths = [0_u8; 19];
    for symbol in ORDER.get(..sent)? {
        *widths.get_mut(usize::from(*symbol))? = u8::try_from(input.bits(3)?).ok()?;
    }
    let width_code = code(&widths)?;
    let total = literals.checked_add(distances)?;
    let mut all = [0_u8; SYMBOLS];
    let mut filled = 0_usize;
    for _run in 0..total {
        if filled == total {
            break;
        }
        let (width, repeats) = match width_code.decode(input)? {
            width @ 0..=15 => (u8::try_from(width).ok()?, 1),
            16 => (*all.get(filled.checked_sub(1)?)?, input.bits(2)?.checked_add(3)?),
            17 => (0, input.bits(3)?.checked_add(3)?),
            18 => (0, input.bits(7)?.checked_add(11)?),
            _ => return None,
        };
        let end = filled.checked_add(usize::try_from(repeats).ok()?)?;
        if end > total {
            return None;
        }
        for slot in all.get_mut(filled..end)? {
            *slot = width;
        }
        filled = end;
    }
    if filled != total || *all.get(256)? == 0 {
        return None;
    }
    Some((code(all.get(..literals)?)?, code(all.get(literals..total)?)?))
}

/// Reads a block's symbols in these codes to its end.
fn codes(input: &mut Input<'_>, out: &mut List<u8>, literals: &Code, distances: &Code) -> Option<()> {
    // Each symbol takes a bit at least, so the member's bits bound them.
    let bound = input.bytes.len().checked_mul(8)?;
    for _symbol in 0..=bound {
        let symbol = literals.decode(input)?;
        match symbol {
            0..=255 => out.push(u8::try_from(symbol).ok()?).ok()?,
            256 => return Some(()),
            _ => {
                let (base, extra) = *LENGTHS.get(usize::from(symbol.checked_sub(257)?))?;
                let length = u32::from(base).checked_add(input.bits(u32::from(extra))?)?;
                let (base, extra) = *DISTANCES.get(usize::from(distances.decode(input)?))?;
                let distance = u32::from(base).checked_add(input.bits(u32::from(extra))?)?;
                let mut from = out.len().checked_sub(distance)?;
                for _byte in 0..length {
                    let byte = *out.get(from)?;
                    out.push(byte).ok()?;
                    from = from.checked_add(1)?;
                }
            }
        }
    }
    None
}

/// The CRC-32 of the body (RFC 1952, 8), bit by bit with the
/// reflected polynomial: the encoder's table is not consulted.
#[must_use]
pub fn crc(body: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in body {
        crc ^= u32::from(*byte);
        for _bit in 0..8_u32 {
            crc = if crc & 1 == 1 { (crc >> 1_u32) ^ 0xEDB8_8320 } else { crc >> 1_u32 };
        }
    }
    !crc
}
