#![expect(
    clippy::disallowed_types,
    clippy::disallowed_macros,
    reason = "the ZIP fixture builds its local headers and central records in Vecs"
)]
#![expect(clippy::disallowed_methods, reason = "the ZIP fixture edits bounded byte records with ordinary slices")]
#![expect(clippy::arithmetic_side_effects, reason = "the ZIP fixture uses its encoded headers' fixed offsets")]
use crate::{api::Error, forgejo::zip};
// Room for the tiny archive, its headers, and a two-file refusal fixture.
const ARCHIVE_BOUND: u32 = 1024;
const FILE: &[u8] = b"a\nb\n";
const FILE_BYTES: u32 = 4;
pub(crate) fn archive(names: &[&[u8]], deflated: bool, descriptor: Option<bool>) -> Vec<u8> {
    let payload = if deflated {
        let mut gzip = vec![0; skein_http::gzip::bound(FILE.len()).unwrap()];
        let length = skein_http::gzip::encode(FILE, &mut gzip).unwrap();
        gzip[10..length - 8].to_vec()
    } else {
        FILE.to_vec()
    };
    let flags: u16 = if descriptor.is_some() { 8 } else { 0 };
    let method: u16 = if deflated { 8 } else { 0 };
    let crc = 0x1857_2a97_u32; // CRC-32 of b"a\nb\n" from zlib.
    let compressed = u32::try_from(payload.len()).unwrap();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for name in names {
        let offset = u32::try_from(out.len()).unwrap();
        out.extend(0x0403_4b50u32.to_le_bytes());
        for value in [20u16, flags, method, 0, 0] {
            out.extend(value.to_le_bytes());
        }
        for value in [
            if descriptor.is_some() { 0 } else { crc },
            if descriptor.is_some() { 0 } else { compressed },
            if descriptor.is_some() { 0 } else { FILE_BYTES },
        ] {
            out.extend(value.to_le_bytes());
        }
        out.extend(u16::try_from(name.len()).unwrap().to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(*name);
        out.extend(&payload);
        if descriptor.is_some() {
            if descriptor == Some(true) {
                out.extend(0x0807_4b50u32.to_le_bytes());
            }
            for value in [crc, compressed, FILE_BYTES] {
                out.extend(value.to_le_bytes());
            }
        }
        central.extend(0x0201_4b50u32.to_le_bytes());
        for value in [20u16, 20, flags, method, 0, 0] {
            central.extend(value.to_le_bytes());
        }
        for value in [crc, compressed, FILE_BYTES] {
            central.extend(value.to_le_bytes());
        }
        for value in [u16::try_from(name.len()).unwrap(), 0, 0, 0, 0] {
            central.extend(value.to_le_bytes());
        }
        for value in [0u32, offset] {
            central.extend(value.to_le_bytes());
        }
        central.extend(*name);
    }
    let offset = u32::try_from(out.len()).unwrap();
    let size = u32::try_from(central.len()).unwrap();
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    for value in [0u16, 0, u16::try_from(names.len()).unwrap(), u16::try_from(names.len()).unwrap()] {
        out.extend(value.to_le_bytes());
    }
    for value in [size, offset] {
        out.extend(value.to_le_bytes());
    }
    out.extend(0u16.to_le_bytes());
    out
}
#[test]
fn stored_and_deflated_artifacts_are_checked_before_cutting() {
    for (deflated, descriptor) in [(false, None), (true, None), (true, Some(true)), (true, Some(false))] {
        let bytes = archive(&[b"failed.txt"], deflated, descriptor);
        for (max_bytes, expected, truncated) in [(FILE_BYTES, FILE, false), (FILE_BYTES - 1, b"a\nb".as_slice(), true)]
        {
            assert_eq!(
                zip::read(&bytes, max_bytes, ARCHIVE_BOUND),
                Ok(zip::File { path: Box::from(*b"failed.txt"), bytes: Box::from(expected), truncated })
            );
        }
    }
}
#[test]
fn several_entries_and_other_methods_are_refused() {
    let multiple = archive(&[b"a", b"b"], false, None);
    let mut method = archive(&[b"failed.txt"], false, None);
    let central = 30 + b"failed.txt".len() + FILE.len();
    method[8..10].copy_from_slice(&12u16.to_le_bytes());
    method[central + 10..central + 12].copy_from_slice(&12u16.to_le_bytes());
    for bytes in [multiple, method] {
        assert_eq!(zip::read(&bytes, FILE_BYTES, ARCHIVE_BOUND), Err(Error::Refused));
    }
}
#[test]
fn a_bad_crc_and_malformed_archive_are_refused() {
    let intact = archive(&[b"failed.txt"], false, None);
    let mut crc = intact.clone();
    let central = 30 + b"failed.txt".len() + FILE.len();
    crc[14] ^= 1;
    crc[central + 16] ^= 1;
    for bytes in [crc, intact[..intact.len() - 1].to_vec()] {
        assert_eq!(zip::read(&bytes, FILE_BYTES, ARCHIVE_BOUND), Err(Error::InvalidAnswer));
    }
}
#[test]
fn an_archive_holds_at_its_byte_bound() {
    let bytes = archive(&[b"failed.txt"], false, None);
    let bound = u32::try_from(bytes.len()).unwrap();
    assert_eq!(
        zip::read(&bytes, FILE_BYTES, bound),
        Ok(zip::File { path: Box::from(*b"failed.txt"), bytes: Box::from(FILE), truncated: false })
    );
    assert_eq!(zip::read(&bytes, FILE_BYTES, bound - 1), Err(Error::TooLarge));
}
