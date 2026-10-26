use crate::keys::{KEYS, pem};
use skein_lib::{List, Writer, base64, bytes};
use skein_tls::{RS256_KEY_BYTES, sign_rs256};

#[test]
fn github_fake_keys() {
    const BEGIN: &[u8] = b"-----BEGIN RSA PRIVATE KEY-----\n";
    const END: &[u8] = b"-----END RSA PRIVATE KEY-----\n";
    let mut signatures = List::with_capacity(3);
    for (number, key) in KEYS.iter().enumerate() {
        assert!(key.len() <= RS256_KEY_BYTES, "key {number}: DER bound");
        let signature = sign_rs256(key, b"the same message").expect("test key signs");
        for prior in &signatures {
            assert!(prior != &signature, "key {number}: distinct signature");
        }
        signatures.push(signature).expect("three signatures");
        let text = pem(key).expect("bounded test key PEM");
        assert!(text.starts_with(BEGIN), "key {number}: opening line");
        assert!(text.ends_with(END), "key {number}: closing line");
        let body = text.strip_prefix(BEGIN).expect("opening").strip_suffix(END).expect("closing");
        let length = base64::encoded_len(base64::Alphabet::Standard, u32::try_from(key.len()).expect("key length"))
            .expect("encoded key length");
        let mut encoded = Writer::new(usize::try_from(length).expect("encoded length"));
        let mut at = 0_usize;
        while at < body.len() {
            let rest = body.get(at..).expect("body cursor");
            let end = bytes::find(rest, b"\n").expect("every body line ends in newline");
            let line = rest.get(..end).expect("line range");
            at = at.checked_add(end).expect("body offset").checked_add(1).expect("newline");
            if at < body.len() {
                assert_eq!(line.len(), 64, "key {number}: full body line");
            } else {
                assert!(!line.is_empty() && line.len() <= 64, "key {number}: last body line");
            }
            encoded.put(line).expect("body allocation holds encoded text");
        }
        let decoded = base64::decode(
            base64::Alphabet::Standard,
            &encoded.finish(),
            u32::try_from(RS256_KEY_BYTES).expect("key bound"),
        )
        .expect("standard base64 body");
        assert!(decoded.as_ref() == *key, "key {number}: DER round trip");
    }
    assert!(pem(b"").expect("empty PEM").starts_with(BEGIN), "empty encoding is bounded");
}
