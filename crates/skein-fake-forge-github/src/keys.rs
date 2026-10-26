//! The App's public test keys (fake-forge.md, 10.1).
use alloc::boxed::Box;
use skein_lib::{Writer, base64};

/// Fixed 2048-bit RSA PKCS#1 `RSAPrivateKey` DER, public and made for the
/// fake. The conversion hands them out in turn.
#[expect(clippy::disallowed_macros, reason = "fake-forge.md, 10.1 fixes these public test keys in the layer")]
pub const KEYS: [&[u8]; 3] =
    [include_bytes!("../keys/0.der"), include_bytes!("../keys/1.der"), include_bytes!("../keys/2.der")];

/// Standard base64 at 64 columns within an `RSA PRIVATE KEY` PEM block.
/// Each line, including the closing line, ends with a newline.
#[must_use]
pub fn pem(der: &[u8]) -> Option<Box<[u8]>> {
    const BEGIN: &[u8] = b"-----BEGIN RSA PRIVATE KEY-----\n";
    const END: &[u8] = b"-----END RSA PRIVATE KEY-----\n";
    let encoded =
        usize::try_from(base64::encoded_len(base64::Alphabet::Standard, u32::try_from(der.len()).ok()?)?).ok()?;
    let length = BEGIN.len().checked_add(encoded)?.checked_add(encoded.div_ceil(64))?.checked_add(END.len())?;
    let mut body = Writer::new(encoded);
    base64::encode(base64::Alphabet::Standard, der, &mut body).ok()?;
    let body = body.finish();
    let mut out = Writer::new(length);
    out.put(BEGIN).ok()?;
    for line in body.chunks(64) {
        out.put(line).ok()?;
        out.put(b"\n").ok()?;
    }
    out.put(END).ok()?;
    Some(out.finish())
}
