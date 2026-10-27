#![expect(clippy::disallowed_types, clippy::disallowed_macros, reason = "tests build bounded members in Vecs")]
#![expect(clippy::disallowed_methods, reason = "fixtures use ordinary slices and iterators")]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::default_numeric_fallback,
    reason = "fixture sizes use ordinary test arithmetic"
)]
use crate::inflate::{deflate, gzip as inflate};
use skein_lib::List;
const HEADER: usize = 10;
use alloc::vec::Vec;

/// `body` through the client's encoder, as the Codex route sends it.
fn encoded(body: &[u8]) -> Vec<u8> {
    let mut member = vec![0; crate::gzip::bound(body.len()).expect("a bounded body")];
    let length = crate::gzip::encode(body, &mut member).expect("room for the member");
    member.truncate(length);
    member
}

fn inflated(member: &[u8], room: u32) -> Option<Vec<u8>> {
    let mut out = List::with_capacity(room);
    inflate(member, &mut out).map(|()| out.as_slice().to_vec())
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex")).collect()
}

#[test]
fn what_the_encoder_sends_inflates_to_the_body() {
    let mut state = 0x2545_f491_u32;
    let noise: Vec<u8> = (0..70_000)
        .map(|_| {
            state ^= state << 13_u32;
            state ^= state >> 17_u32;
            state ^= state << 5_u32;
            state.to_le_bytes()[0]
        })
        .collect();
    let text = br#"{"model":"fixture","input":[{"type":"message","content":"Hello"}]}"#.repeat(900);
    let run = vec![7; 300_000];
    for body in [&b""[..], b"x", &text, &noise, &run] {
        let member = encoded(body);
        let room = u32::try_from(body.len()).expect("a bounded body");
        assert_eq!(inflated(&member, room).as_deref(), Some(body));
    }
}

#[test]
fn stored_and_fixed_blocks_inflate_too() {
    // Python's zlib at level 0, and with Z_FIXED.
    let stored = hex("1f8b0800000000000403010f00f0ff73746f7265642061732069742069736578d15a0f000000");
    assert_eq!(inflated(&stored, 15).as_deref(), Some(&b"stored as it is"[..]));
    let fixed = hex("1f8b08000000000002034b5448cbac484d5148ce4f49d55148c4c903005ef3d59928000000");
    assert_eq!(inflated(&fixed, 40).as_deref(), Some(&b"a fixed code, a fixed code, a fixed code"[..]));
}

#[test]
fn a_member_that_is_not_whole_and_checked_is_refused() {
    let body = b"a body the server holds room for, a body the server holds room for";
    let member = encoded(body);
    let room = u32::try_from(body.len()).expect("a bounded body");
    assert!(inflated(&member, room).is_some());
    let changed = |at: usize, bits: u8| {
        let mut member = member.clone();
        member[at] ^= bits;
        member
    };
    let crc = member.len() - 8;
    let size = member.len() - 4;
    for refused in [
        changed(0, 1),    // not gzip
        changed(2, 1),    // not DEFLATE
        changed(3, 8),    // a file name follows
        changed(crc, 1),  // the CRC-32 fails
        changed(size, 1), // ISIZE fails
        member[..member.len() - 1].to_vec(),
        member[..HEADER + 2].to_vec(),
        [&member[..], &[0]].concat(), // a byte trails the member
    ] {
        assert_eq!(inflated(&refused, room), None);
    }
    assert_eq!(inflated(&member, room - 1), None, "past the room");
}

#[test]
fn raw_deflate_round_trip_holds_at_output_limit() {
    const BODY: &[u8] = b"a repeated body, a repeated body, a repeated body";
    let member = encoded(BODY);
    let data = &member[HEADER..member.len() - 8];
    for (room, expected) in [(BODY.len(), Some(BODY)), (BODY.len() - 1, None)] {
        let mut out = List::with_capacity(u32::try_from(room).expect("fixture fits"));
        let result = deflate(data, &mut out);
        assert_eq!(result.map(|()| out.as_slice()), expected);
    }
}
