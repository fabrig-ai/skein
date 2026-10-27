//! Startup's bounded file and trust-root reads (shell.md, 6.1 and 6.2).
//! It keeps no environment or service state: main supplies `SSL_CERT_FILE`,
//! bounds and the trust choice. The result owns DER certificates and their
//! source path; TLS decides which parsed roots it supports.
//!
//! Without an explicit bundle, distribution paths are tried in this order:
//! Debian's /etc/ssl/certs/ca-certificates.crt, Red Hat's
//! /etc/pki/tls/certs/ca-bundle.crt, SUSE's /etc/ssl/ca-bundle.pem,
//! /etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem, then /etc/ssl/cert.pem.
//! Only an absent file moves to the next path; an existing unreadable or
//! oversized bundle refuses startup. PEM follows RFC 7468's lax form and
//! recovers at the next BEGIN line after a malformed certificate block.

use std::path::{Path, PathBuf};

use skein_io::kernel;
use skein_lib::Secret;
use skein_lib::base64::{self, Alphabet};

pub use crate::system::{
    directory_access, effective_user, make_root, open_append, open_root, prepare_output, process_id,
};

/// Opens an independently owned launch directory beneath `root`, refusing
/// paths that escape it (world.md, 3). Released by close or process exit.
pub fn open_beneath(root: kernel::Fd, path: &[u8]) -> Result<kernel::Fd, kernel::Error> {
    if path.contains(&0) {
        return Err(kernel::Error::InvalidArgument);
    }
    crate::system::files::open(root, path, kernel::OpenHow::Directory).map(kernel::Fd::new).map_err(|errno| {
        crate::calls::file_error(
            &kernel::Op::Open { root, path: Box::from(path), how: kernel::OpenHow::Directory },
            errno,
        )
    })
}

/// How startup adopts a standard output descriptor (io.md, section 5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    /// A pipe or terminal, sent through the inherited write-pipe face.
    Pipe,
    /// A regular file, sent through io's append-stream face.
    Append,
}

/// The kernel's startup answer on whether the effective user may write and
/// search a directory (shell.md, section 6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum DirectoryAccess {
    /// The path names a directory the effective user may write and search.
    Writable,
    /// The path or a component of it is absent.
    Absent,
    /// The path or a parent component is not a directory.
    NotDirectory,
    /// The check failed with this kernel error number.
    Denied(i32),
}

/// Why startup's regular-file read returned no bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unread {
    /// The requested path was absent.
    Absent,
    /// The opened entry was not a regular file.
    NotAFile,
    /// The file exceeded startup's byte bound.
    TooLarge { max: u32 },
    /// The adapter's kernel call failed.
    Kernel(kernel::Error),
    /// The private-file checks refused what was opened, before reading bytes.
    Unsafe(skein_io::file::Unsafe),
}

/// Reads a regular file whole within `max` bytes, without blocking on an
/// entry such as a FIFO or taking a controlling terminal (shell.md, 6.1).
pub fn read_file(path: &Path, max: u32) -> Result<Box<[u8]>, Unread> {
    crate::system::read_file(path, max)
}

/// Reads a secret within `max` bytes after io's private-file checks,
/// using the expected owner supplied by startup (shell.md, 6.1).
pub fn read_private_file(path: &Path, max: u32, user: u32) -> Result<Secret, Unread> {
    crate::system::read_private_file(path, max, user)
}

/// The trust source startup's configuration asks the shell to read.
#[derive(Clone, Debug)]
pub enum Trust {
    /// The explicit `SSL_CERT_FILE` bundle, or the documented distribution paths.
    Machine { ssl_cert_file: Option<PathBuf> },
    /// One DER certificate at the supplied path.
    Der(PathBuf),
}

/// Configuration's bounds for reading trust roots at startup (shell.md, 6.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RootBounds {
    pub bundle_bytes: u32,
    pub certificates: u32,
    pub certificate_bytes: u32,
}

/// Startup's DER certificates in file order, their source and skipped blocks.
#[derive(Debug)]
pub struct Roots {
    pub certificates: Box<[Box<[u8]>]>,
    pub path: PathBuf,
    pub other_blocks: u32,
    pub malformed: u32,
}

/// Why startup could not read trust roots within its configured bounds.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RootsRefusal {
    /// No explicit or distribution bundle exists.
    NoBundle,
    /// The source file was not read.
    Unread { path: PathBuf, why: Unread },
    /// More DER certificates were taken than configuration permits.
    TooMany { bound: u32 },
    /// A certificate exceeded its byte bound while decoding.
    /// `index` counts certificate blocks from zero, including malformed blocks.
    TooLong { index: u32, bound: u32 },
}

/// Reads DER roots within `bounds`, decoding certificate PEM blocks and
/// counting malformed and other blocks (shell.md, 6.2). Empty results are
/// handed to TLS, whose `Config::from_der` refuses roots of none.
pub fn read_trust_roots(trust: &Trust, bounds: &RootBounds) -> Result<Roots, RootsRefusal> {
    match trust {
        Trust::Der(path) => {
            let certificate = source(path, bounds.bundle_bytes)?;
            if u64::try_from(certificate.len()).expect("length fits") > u64::from(bounds.certificate_bytes) {
                return Err(RootsRefusal::TooLong { index: 0, bound: bounds.certificate_bytes });
            }
            if bounds.certificates == 0 {
                return Err(RootsRefusal::TooMany { bound: 0 });
            }
            Ok(Roots { certificates: Box::from([certificate]), path: path.clone(), other_blocks: 0, malformed: 0 })
        }
        Trust::Machine { ssl_cert_file: Some(path) } => {
            let bytes = source(path, bounds.bundle_bytes)?;
            pem(&bytes, path, bounds)
        }
        Trust::Machine { ssl_cert_file: None } => {
            let paths = [
                "/etc/ssl/certs/ca-certificates.crt",
                "/etc/pki/tls/certs/ca-bundle.crt",
                "/etc/ssl/ca-bundle.pem",
                "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
                "/etc/ssl/cert.pem",
            ]
            .map(Path::new);
            machine(&paths, bounds)
        }
    }
}

fn machine(paths: &[&Path], bounds: &RootBounds) -> Result<Roots, RootsRefusal> {
    for path in paths {
        match read_file(path, bounds.bundle_bytes) {
            Ok(bytes) => return pem(&bytes, path, bounds),
            Err(Unread::Absent) => {}
            Err(why) => return Err(RootsRefusal::Unread { path: path.to_path_buf(), why }),
        }
    }
    Err(RootsRefusal::NoBundle)
}

fn source(path: &Path, max: u32) -> Result<Box<[u8]>, RootsRefusal> {
    read_file(path, max).map_err(|why| RootsRefusal::Unread { path: path.to_path_buf(), why })
}

fn pem(bundle: &[u8], path: &Path, bounds: &RootBounds) -> Result<Roots, RootsRefusal> {
    let mut certificates = Vec::new();
    let mut current: Option<(u32, base64::Decoder, u32, bool)> = None;
    let mut certificate_blocks: u32 = 0;
    let mut other_blocks: u32 = 0;
    let mut malformed: u32 = 0;
    for line in bundle.split(|byte| *byte == b'\n' || *byte == b'\r') {
        let line = trim_whitespace(line);
        if let Some(label) = boundary(line, b"-----BEGIN ") {
            finish(current.take(), false, bounds, &mut certificates, &mut malformed)?;
            if label == b"CERTIFICATE" {
                current = Some((
                    certificate_blocks,
                    base64::Decoder::new(Alphabet::Standard, bounds.certificate_bytes),
                    0,
                    false,
                ));
                certificate_blocks = certificate_blocks.checked_add(1).expect("blocks fit bounded bundle bytes");
            } else {
                other_blocks = other_blocks.checked_add(1).expect("blocks fit bounded bundle bytes");
            }
        } else if let Some(label) = boundary(line, b"-----END ") {
            finish(current.take(), label == b"CERTIFICATE", bounds, &mut certificates, &mut malformed)?;
        } else if let Some(decoder) = &mut current {
            for run in line.split(|byte| whitespace(*byte)) {
                decoder.3 |= run.contains(&b'=');
                decoder.2 =
                    decoder.2.checked_add(u32::try_from(run.len()).expect("bounded bundle")).expect("bounded bundle");
                if let Err(base64::Error::TooLong { max }) = decoder.1.push(run) {
                    return Err(RootsRefusal::TooLong { index: decoder.0, bound: max });
                }
            }
        }
    }
    finish(current, false, bounds, &mut certificates, &mut malformed)?;
    Ok(Roots { certificates: certificates.into_boxed_slice(), path: path.to_path_buf(), other_blocks, malformed })
}

fn boundary<'a>(line: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    line.strip_prefix(prefix)?.strip_suffix(b"-----")
}

fn whitespace(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == 0x0b
}

fn trim_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let Some(byte) = bytes.first() {
        if !whitespace(*byte) {
            break;
        }
        bytes = bytes.get(1..).expect("a leading byte exists");
    }
    while let Some(byte) = bytes.last() {
        if !whitespace(*byte) {
            break;
        }
        let end = bytes.len().checked_sub(1).expect("a trailing byte exists");
        bytes = bytes.get(..end).expect("within bytes");
    }
    bytes
}

fn finish(
    decoder: Option<(u32, base64::Decoder, u32, bool)>,
    ended: bool,
    bounds: &RootBounds,
    certificates: &mut Vec<Box<[u8]>>,
    malformed: &mut u32,
) -> Result<(), RootsRefusal> {
    if let Some((index, mut decoder, symbols, padded)) = decoder {
        // RFC 7468's lax PEM accepts an unpadded final group as well.
        if !padded && symbols % 4 != 0 {
            for _ in symbols % 4..4 {
                if let Err(base64::Error::TooLong { max }) = decoder.push(b"=") {
                    return Err(RootsRefusal::TooLong { index, bound: max });
                }
            }
        }
        match decoder.finish() {
            Ok(bytes) if ended => {
                if u64::try_from(certificates.len()).expect("count fits") >= u64::from(bounds.certificates) {
                    return Err(RootsRefusal::TooMany { bound: bounds.certificates });
                }
                certificates.push(bytes);
            }
            Ok(_) | Err(_) => *malformed = malformed.checked_add(1).expect("blocks fit bounded bundle bytes"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use skein_scratch::Scratch;

    use super::{RootBounds, RootsRefusal, Unread, machine};

    #[test]
    fn distribution_bundles_choose_the_first_existing_path_and_refuse_its_errors() {
        let scratch = Scratch::new("startup");
        let absent = scratch.path().join("absent");
        let first = scratch.path().join("first");
        let second = scratch.path().join("second");
        let bounds = RootBounds { bundle_bytes: 1_024, certificates: 1, certificate_bytes: 3 };
        fs::write(&first, b"-----BEGIN CERTIFICATE-----\nTWFu\n-----END CERTIFICATE-----\n").expect("first bundle");
        fs::write(&second, b"-----BEGIN CERTIFICATE-----\nTWE=\n-----END CERTIFICATE-----\n").expect("second bundle");
        let roots = machine(&[&absent, &first, &second], &bounds).expect("first existing distribution bundle");
        assert_eq!(roots.path, first, "documented path order");
        assert_eq!(machine(&[&absent], &bounds).expect_err("none exist"), RootsRefusal::NoBundle, "no fallback source");
        assert_eq!(
            machine(&[scratch.path(), &second], &bounds).expect_err("first source is a directory"),
            RootsRefusal::Unread { path: scratch.path().to_path_buf(), why: Unread::NotAFile },
            "an existing refused source never falls through"
        );
    }
}
