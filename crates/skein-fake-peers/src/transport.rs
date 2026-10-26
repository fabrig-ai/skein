//! The peers' transport: plaintext everywhere, buffered TLS on Unix (testing.md, 10).

use crate::{Error, Limits};

#[cfg(unix)]
mod native;
#[cfg(unix)]
pub(crate) use native::Wire;
#[cfg(not(unix))]
mod plaintext;
#[cfg(not(unix))]
pub(crate) use plaintext::Wire;

/// The DER root signing the peers' fixed chain for `skein.test` and `127.0.0.1`.
/// Trust it only in tests: its key is in the tree.
pub const ROOT: &[u8] = include_bytes!("../../../tests/tls/fixtures/root.der");

/// Transport selected by a world; TLS presents Skein's fixed test certificates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// A replayable local byte stream selected by a simulated world.
    Plaintext,
    /// Real rustls cryptography selected by a TLS or real-loop world.
    Tls,
}

pub(crate) fn worst_case(limits: &Limits, transport: Transport) -> Option<u64> {
    let base = u64::from(limits.plaintext).checked_add(u64::from(limits.ciphertext).checked_mul(4)?)?;
    match transport {
        Transport::Plaintext => Some(base),
        // rustls limits a handshake message to 64 KiB, plaintext records to
        // 16 KiB, and buffers to ciphertext. The fixed unauthenticated server
        // chain/configuration, decoded hello extensions and crypto workspace
        // share this conservative 1 MiB envelope. The worst-case tests meter
        // construction, handshake and full buffers (testing-strategy.md, 6).
        Transport::Tls => base.checked_add(1_048_576),
    }
}

pub(crate) fn validate(limits: &Limits, transport: Transport) -> Result<(), Error> {
    if transport == Transport::Tls && cfg!(not(unix)) {
        // TLS is not available on the portable target: a world runs in plaintext
        // (testing.md, 10).
        return Err(Error::Limits);
    }
    if transport == Transport::Tls && limits.plaintext < 16_384 {
        return Err(Error::Limits);
    }
    Ok(())
}
