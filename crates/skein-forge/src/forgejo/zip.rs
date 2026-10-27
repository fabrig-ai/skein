//! One bounded ZIP entry, stored or deflated, checked against its central directory.
use crate::api::Error;
use alloc::boxed::Box;
use skein_lib::{List, bytes::copy_of};

/// The archive's single file, cut only after its CRC has been checked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    pub bytes: Box<[u8]>,
    pub truncated: bool,
}
fn short(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk::<2>()?))
}
fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk::<4>()?))
}
fn end(bytes: &[u8]) -> Option<usize> {
    // The end record has 22 fixed bytes and at most u16::MAX comment bytes.
    let last = bytes.len().checked_sub(22)?;
    let first = last.saturating_sub(usize::from(u16::MAX));
    for at in (first..=last).rev() {
        if word(bytes, at)? == 0x0605_4b50 {
            let comment = usize::from(short(bytes, at.checked_add(20)?)?);
            if at.checked_add(22)?.checked_add(comment)? == bytes.len() {
                return Some(at);
            }
        }
    }
    None
}
struct Entry {
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u32,
    inflated: u32,
    local: usize,
    name: Box<[u8]>,
    central: usize,
}
fn entry(bytes: &[u8]) -> Result<Entry, Error> {
    let end = end(bytes).ok_or(Error::InvalidAnswer)?;
    let tail = bytes.get(end..).ok_or(Error::InvalidAnswer)?;
    if short(tail, 4) != Some(0) || short(tail, 6) != Some(0) {
        return Err(Error::Refused);
    }
    if short(tail, 8) != Some(1) || short(tail, 10) != Some(1) {
        return Err(Error::Refused);
    }
    let size = usize::try_from(word(tail, 12).ok_or(Error::InvalidAnswer)?).or(Err(Error::TooLarge))?;
    let central = usize::try_from(word(tail, 16).ok_or(Error::InvalidAnswer)?).or(Err(Error::TooLarge))?;
    if central.checked_add(size) != Some(end) {
        return Err(Error::InvalidAnswer);
    }
    let directory = bytes.get(central..end).ok_or(Error::InvalidAnswer)?;
    if word(directory, 0) != Some(0x0201_4b50) || short(directory, 34) != Some(0) {
        return Err(Error::InvalidAnswer);
    }
    let name = usize::from(short(directory, 28).ok_or(Error::InvalidAnswer)?);
    let extra = usize::from(short(directory, 30).ok_or(Error::InvalidAnswer)?);
    let comment = usize::from(short(directory, 32).ok_or(Error::InvalidAnswer)?);
    let name_end = 46usize.checked_add(name).ok_or(Error::TooLarge)?;
    let length = name_end.checked_add(extra).ok_or(Error::TooLarge)?.checked_add(comment).ok_or(Error::TooLarge)?;
    if length != size {
        return Err(Error::InvalidAnswer);
    }
    let name = directory.get(46..name_end).ok_or(Error::InvalidAnswer)?;
    if name.is_empty() || name.ends_with(b"/") {
        return Err(Error::Refused);
    }
    let entry = Entry {
        flags: short(directory, 8).ok_or(Error::InvalidAnswer)?,
        method: short(directory, 10).ok_or(Error::InvalidAnswer)?,
        crc: word(directory, 16).ok_or(Error::InvalidAnswer)?,
        compressed: word(directory, 20).ok_or(Error::InvalidAnswer)?,
        inflated: word(directory, 24).ok_or(Error::InvalidAnswer)?,
        local: usize::try_from(word(directory, 42).ok_or(Error::InvalidAnswer)?).or(Err(Error::TooLarge))?,
        name: copy_of(name),
        central,
    };
    if entry.flags & !0x080e != 0 || (entry.method != 0 && entry.method != 8) {
        return Err(Error::Refused);
    }
    Ok(entry)
}
fn data(bytes: &[u8], entry: &Entry) -> Result<Box<[u8]>, Error> {
    let local = bytes.get(entry.local..entry.central).ok_or(Error::InvalidAnswer)?;
    if word(local, 0) != Some(0x0403_4b50)
        || short(local, 6) != Some(entry.flags)
        || short(local, 8) != Some(entry.method)
    {
        return Err(Error::InvalidAnswer);
    }
    let name = usize::from(short(local, 26).ok_or(Error::InvalidAnswer)?);
    let extra = usize::from(short(local, 28).ok_or(Error::InvalidAnswer)?);
    let name_end = 30usize.checked_add(name).ok_or(Error::TooLarge)?;
    if local.get(30..name_end) != Some(entry.name.as_ref()) {
        return Err(Error::InvalidAnswer);
    }
    let start = name_end.checked_add(extra).ok_or(Error::TooLarge)?;
    let end = start.checked_add(usize::try_from(entry.compressed).or(Err(Error::TooLarge))?).ok_or(Error::TooLarge)?;
    let payload = local.get(start..end).ok_or(Error::InvalidAnswer)?;
    if entry.flags & 8 == 0 {
        if word(local, 14) != Some(entry.crc)
            || word(local, 18) != Some(entry.compressed)
            || word(local, 22) != Some(entry.inflated)
            || end != local.len()
        {
            return Err(Error::InvalidAnswer);
        }
    } else {
        let descriptor = local.get(end..).ok_or(Error::InvalidAnswer)?;
        let descriptor = if descriptor.len() == 16 && word(descriptor, 0) == Some(0x0807_4b50) {
            descriptor.get(4..).ok_or(Error::InvalidAnswer)?
        } else {
            descriptor
        };
        if descriptor.len() != 12
            || word(descriptor, 0) != Some(entry.crc)
            || word(descriptor, 4) != Some(entry.compressed)
            || word(descriptor, 8) != Some(entry.inflated)
        {
            return Err(Error::InvalidAnswer);
        }
    }
    Ok(copy_of(payload))
}
/// Reads a whole archive and checks its single entry before cutting the file.
/// Both the archive and its inflated entry must fit `bound`, the owner's existing answer bound.
pub fn read(archive: &[u8], max_bytes: u32, bound: u32) -> Result<File, Error> {
    if archive.len() > usize::try_from(bound).expect("u32 fits") || max_bytes > bound {
        return Err(Error::TooLarge);
    }
    let entry = entry(archive)?;
    if entry.inflated > bound {
        return Err(Error::TooLarge);
    }
    let payload = data(archive, &entry)?;
    let mut inflated = List::with_capacity(entry.inflated);
    match entry.method {
        0 => {
            if entry.compressed != entry.inflated {
                return Err(Error::InvalidAnswer);
            }
            for byte in payload {
                inflated.push(byte).or(Err(Error::InvalidAnswer))?;
            }
        }
        8 => {
            skein_http::inflate::deflate(&payload, &mut inflated).ok_or(Error::InvalidAnswer)?;
        }
        _ => return Err(Error::Refused),
    }
    if inflated.len() != entry.inflated || skein_http::inflate::crc(inflated.as_slice()) != entry.crc {
        return Err(Error::InvalidAnswer);
    }
    let kept = usize::try_from(max_bytes.min(entry.inflated)).expect("u32 fits");
    Ok(File {
        path: entry.name,
        bytes: copy_of(inflated.as_slice().get(..kept).expect("bounded output")),
        truncated: entry.inflated > max_bytes,
    })
}
