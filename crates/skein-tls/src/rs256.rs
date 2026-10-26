//! RS256 through the TLS ring provider: OAuth's check (oauth.md, 2.2) and a GitHub App's signature (tls.md, 4).

use alloc::boxed::Box;

use rustls::pki_types::PrivateKeyDer;
use rustls::sign::SigningKey;
use rustls::{SignatureScheme, crypto::ring};
use skein_lib::Writer;

/// The largest PKCS#1 `RSAPrivateKey` DER of a 2048-bit RSA key that ring
/// signs with: a 4-byte sequence header; version 0 (3 bytes); `n` and `d`,
/// 256 bytes with a sign byte (261 each); `e` at most ring's 2^33 - 1
/// (7); `p`, `q`, `dP`, `dQ` and `qInv`, 128 bytes with a sign byte (132
/// each). 4 + 3 + 261 + 7 + 261 + 5 * 132.
pub const RS256_KEY_BYTES: usize = 1196;

/// An RS256 signature of `message` (RSASSA-PKCS1-v1_5, SHA-256) by a
/// 2048-bit RSA key, PKCS#1 `RSAPrivateKey` DER of at most
/// [`RS256_KEY_BYTES`]; none for any other key, or one ring refuses.
/// It draws ring's blinding randomness from the kernel; the signature
/// does not depend on it. The key is parsed at each call, never kept.
#[must_use]
pub fn sign_rs256(key: &[u8], message: &[u8]) -> Option<[u8; 256]> {
    if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
        // TLS is not available on the portable target: a world runs in plaintext
        // (testing.md, 10).
        return None;
    }
    if key.len() > RS256_KEY_BYTES {
        return None;
    }
    let key = PrivateKeyDer::Pkcs1(key.into());
    let key = ring::sign::RsaSigningKey::new(&key).ok()?;
    let signer = key.choose_scheme(&[SignatureScheme::RSA_PKCS1_SHA256])?;
    let signature = signer.sign(message).ok()?;
    <[u8; 256]>::try_from(signature.as_slice()).ok()
}

/// The largest 4096-bit two-prime RSA PKCS#8 key ring accepts: the
/// PKCS#1 sequence is 4 + 3 + 517 + 7 + 517 + 5 * 261 = 2353 bytes
/// (version, n, e <= 2^33 - 1, d, and five half-size CRT integers).
/// `PrivateKeyInfo` adds its sequence (4), version (3), rsaEncryption
/// `AlgorithmIdentifier` (15), and OCTET STRING header (4): 2379 bytes.
pub const RS256_PKCS8_KEY_BYTES: usize = 2379;

/// An issuer's unsigned RSA public components (tls.md, 4).
#[derive(Debug)]
pub struct Rs256Public {
    pub modulus: Box<[u8]>,
    pub exponent: Box<[u8]>,
}

/// Signs with PKCS#8 RSA of 2048 to 4096 bits, through rustls's wrapper
/// of ring's `RsaKeyPair::from_pkcs8`. Blinding does not change the bytes.
#[must_use]
pub fn sign_rs256_pkcs8(key: &[u8], message: &[u8]) -> Option<Box<[u8]>> {
    if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
        // TLS is not available on the portable target: a world runs in plaintext
        // (testing.md, 10).
        return None;
    }
    let parsed = issuer_key(key)?;
    let signer = parsed.choose_scheme(&[SignatureScheme::RSA_PKCS1_SHA256])?;
    let signature = signer.sign(message).ok()?;
    Some(signature.into_boxed_slice())
}

/// Reads a key only after ring accepts it; never accepts another key
/// form or size than `sign_rs256_pkcs8` (tls.md, 4).
#[must_use]
pub fn rs256_public(key: &[u8]) -> Option<Rs256Public> {
    let _parsed = issuer_key(key)?;
    let (modulus, exponent) = issuer_components(key)?;
    Some(Rs256Public { modulus: Box::from(modulus), exponent: Box::from(exponent) })
}

fn issuer_key(key: &[u8]) -> Option<ring::sign::RsaSigningKey> {
    if key.len() > RS256_PKCS8_KEY_BYTES {
        return None;
    }
    let der = PrivateKeyDer::Pkcs8(key.into());
    let parsed = ring::sign::RsaSigningKey::new(&der).ok()?;
    let (modulus, exponent) = issuer_components(key)?;
    if !(256..=512).contains(&modulus.len()) || !(1..=5).contains(&exponent.len()) {
        return None;
    }
    Some(parsed)
}

fn issuer_components(key: &[u8]) -> Option<(&[u8], &[u8])> {
    let (sequence, _) = der_element(key, 0x30)?;
    let (_version, rest) = der_element(sequence, 0x02)?;
    let (_algorithm, rest) = der_element(rest, 0x30)?;
    let (private, _) = der_element(rest, 0x04)?;
    let (sequence, _) = der_element(private, 0x30)?;
    let (version, rest) = der_element(sequence, 0x02)?;
    if version != [0] {
        return None;
    }
    let (modulus, rest) = der_element(rest, 0x02)?;
    let (exponent, _) = der_element(rest, 0x02)?;
    Some((unsigned(modulus), unsigned(exponent)))
}

// ring has already checked DER's canonical spelling and the whole key.
fn der_element(bytes: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = bytes.split_first()?;
    if found != tag {
        return None;
    }
    let (&first, mut rest) = rest.split_first()?;
    let mut len = usize::from(first);
    if first & 0x80 != 0 {
        len = 0;
        for _ in 0..(first & 0x7f) {
            let (&byte, tail) = rest.split_first()?;
            len = len.checked_mul(256)?.checked_add(usize::from(byte))?;
            rest = tail;
        }
    }
    rest.split_at_checked(len)
}

/// An RS256 signature check by an RSA key of 2048 to 8192 bits.
#[must_use]
pub fn verify_rs256(modulus: &[u8], exponent: &[u8], message: &[u8], signature: &[u8]) -> bool {
    if modulus.is_empty() || modulus.len() > 1024 || exponent.is_empty() {
        return false;
    }
    let Some(key) = public_key(modulus, exponent) else {
        return false;
    };
    let provider = ring::default_provider();
    for (scheme, algorithms) in provider.signature_verification_algorithms.mapping {
        if *scheme == SignatureScheme::RSA_PKCS1_SHA256 {
            for algorithm in *algorithms {
                if algorithm.verify_signature(&key, message, signature).is_ok() {
                    return true;
                }
            }
        }
    }
    false
}

fn public_key(modulus: &[u8], exponent: &[u8]) -> Option<Box<[u8]>> {
    integers(&[modulus, exponent])
}

pub(crate) fn integers(values: &[&[u8]]) -> Option<Box<[u8]>> {
    let mut contents = 0_usize;
    for value in values {
        if value.is_empty() {
            return None;
        }
        contents = contents.checked_add(element_len(integer_len(unsigned(value))?)?)?;
    }
    let mut writer = Writer::new(element_len(contents)?);
    header(&mut writer, 0x30, contents);
    for value in values {
        let bytes = unsigned(value);
        integer(&mut writer, bytes, integer_len(bytes)?);
    }
    Some(writer.finish())
}

// DER INTEGER is signed: remove redundant zeroes, then protect a high bit.
fn unsigned(bytes: &[u8]) -> &[u8] {
    for (offset, byte) in bytes.iter().enumerate() {
        if *byte != 0 {
            return bytes.get(offset..).expect("offset belongs to bytes");
        }
    }
    bytes.get(..1).expect("nonempty input")
}

fn integer_len(bytes: &[u8]) -> Option<usize> {
    let first = bytes.first()?;
    bytes.len().checked_add(usize::from(first & 0x80 != 0))
}

fn length_bytes(len: usize) -> usize {
    if len < 128 {
        return 0;
    }
    let bits = usize::BITS.checked_sub(len.leading_zeros()).expect("bit count");
    usize::try_from(bits.div_ceil(8)).expect("usize's byte count")
}

fn element_len(len: usize) -> Option<usize> {
    len.checked_add(2)?.checked_add(length_bytes(len))
}

fn header(writer: &mut Writer, tag: u8, len: usize) {
    writer.put(&[tag]).expect("measured tag");
    let count = length_bytes(len);
    if count == 0 {
        writer.put(&[u8::try_from(len).expect("short DER length")]).expect("measured length");
    } else {
        let marker = 0x80 | u8::try_from(count).expect("usize byte count fits u8");
        writer.put(&[marker]).expect("measured length marker");
        let bytes = len.to_be_bytes();
        let start = bytes.len().checked_sub(count).expect("length fits usize");
        writer.put(bytes.get(start..).expect("length bytes")).expect("measured long length");
    }
}

fn integer(writer: &mut Writer, bytes: &[u8], len: usize) {
    header(writer, 0x02, len);
    if len > bytes.len() {
        writer.put(&[0]).expect("measured sign byte");
    }
    writer.put(bytes).expect("measured integer");
}
