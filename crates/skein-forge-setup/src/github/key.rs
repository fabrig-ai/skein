//! Bounded PEM admission shared by conversion and paste (forge-setup.md, 5.2 and 5.8).
use crate::item;
use skein_lib::{
    Secret, Writer,
    base64::{self, Alphabet},
};

/// One RSA PRIVATE KEY block, within Pasted's input bound and DER's key bound
/// (forge-setup.md, 5.2 and 6.4). The host proves the RSA structure by signing.
pub(super) fn pem(pem: &Secret, key_bytes: u32) -> Option<Secret> {
    if u64::try_from(pem.len()).ok()? > u64::from(key_bytes).checked_mul(2)? {
        return None;
    }
    let mut block = pem.expose();
    for byte in pem.expose() {
        if !whitespace(*byte) {
            break;
        }
        block = block.get(1..)?;
    }
    for byte in pem.expose().iter().rev() {
        if !whitespace(*byte) {
            break;
        }
        block = block.get(..block.len().checked_sub(1)?)?;
    }
    let payload = block.strip_prefix(b"-----BEGIN RSA PRIVATE KEY-----")?;
    let payload = match payload.strip_prefix(b"\r\n") {
        Some(rest) => rest,
        None => payload.strip_prefix(b"\n")?,
    };
    let payload = payload.strip_suffix(b"-----END RSA PRIVATE KEY-----")?;
    if !payload.ends_with(b"\n") {
        return None;
    }
    let bound = usize::try_from(base64::encoded_len(Alphabet::Standard, key_bytes)?).ok()?;
    let mut length = 0_usize;
    let mut cr = false;
    for byte in payload {
        if cr && *byte != b'\n' {
            return None;
        }
        cr = *byte == b'\r';
        if *byte != b'\r' && *byte != b'\n' {
            length = length.checked_add(1)?;
            if length > bound {
                return None;
            }
        }
    }
    let mut text = Writer::new(length);
    for byte in payload {
        if *byte != b'\r' && *byte != b'\n' {
            text.put(&[*byte]).ok()?;
        }
    }
    let text = Secret::new(text.finish());
    let der = Secret::new(base64::decode(Alphabet::Standard, text.expose(), key_bytes).ok()?);
    item::app_key(der.expose(), key_bytes)?;
    Some(der)
}
fn whitespace(byte: u8) -> bool {
    byte == b' ' || (b'\t'..=b'\r').contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::pem;
    use skein_lib::Secret;

    // Local fake App 1: the whole source key and the parser input captured from
    // hand's insertText, which changed its line endings to spaces before restore.
    const SOURCE: &[u8] = b"-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA1lD64NYODaseRc3unWmUv9A8TreUpVTFrWoabEyyEamol4JC\nBo+Jkt/XZc0Mpi1SkuEb315hZVk9Sw15FoBcIBdTEcW7RQkuqDWSCLN5sJ4mcQnG\nEa9X6DhVBs8WVPHf057rZuBgD+xsjv5GXNX5Wfdbde/2sCda69QL+SzTZyX5h+xo\no8btQw/cJfmJVFvGp3S3S95yxbR0bGCvzeHLJ+QdgY0bQmYGSvsqaWD+oKHt8d+m\nkcLZNM7n14BdpZ+X4t50cejBAXptxOqy9Si0qmKSRy6ZwpyaY1YwMLe4xWR3O9TY\ngUMPXteIwpkzJmY5mdopt0InZtKWxaqyl2VVtwIDAQABAoIBAA93WZuHJw16d11P\nPrSSbX8oLmn52prR0tAwc4a0RDd9GGMSreQeUK3ORJOeqBaH98kMwL+uZnXzajZz\n2vSyP7O7Ky4BqIGTZ5ARCaBiH1qNFzYaH/afwkbKZexvXmuz5tS1R8StVZFgnPb2\nx6okcPG6pZmNvy44MNoK+/A3+WuckqBzSYzQoJ+N7xa3gGO53n/sCs6NM+/+U7un\nZ3SaYsMNpHcx06xyrEiiIhLu4UNqZNLS0JXoqMLHdgn1y/Cu0Y/RzKaYjziiBM7i\n2zdrhRJ59GiIu348EszBS10nqmJBcAGldvUAp0KDtDgD+fdoBLb4Q6OhhrIpQ6Tu\nVjozj40CgYEA8uzvx8ymoAfdJyV83NbiQ7/3wLQbU550f4nu49EhIld/XnmKNhPd\nnfeQlbRf1l+jj1Atx2Gzgn9vt1agMahbfESG+DMBJ7U90sV9F3sUR4u+tRWoczPL\nqm/Z4vQksR6wtf/ujjWrmqcxQN6AIlD/ObDtFGnrBNOKTtSi2DrjNlsCgYEA4dnc\njqXOPxgJ4vLebH+EfLfFkGS3+MryWfF1LIZa/JS440CBZnrA/HVi6Rbs2TXjiT71\nDM5KdvjpeFzxf5CJBu8b1mbywlf/JVrefIcOzB15kahvtoOUt7iWvKmdQg9XiHLl\nQZUXiVTJPC/5rBtrDX8pFB3xKqhJSv081gduFNUCgYEA58pmnKlMUCRQw5qEgp/S\n87xHJSt1a5slI9C0DdKtH/yG/tlPpHveGW1wGzgpvf4xrXxewgEabKk2KqTQY7F1\n4aPteQQJ1R6dp/BihLYeyomv5U+OFKZWbgEd0O0N2ackC/TxZeYWIJHpcDEd6CCK\n9zw3ayQJaOiD7uX7oADIbkMCgYEAmsVAduS6/NPNWqASuol8UjdhhPzwcecpw2dL\nY/LCgVqthNYclwzCGX+cA525O3TJc7EYHulzcCpNXDVdLYzvyjerfPbZn0LMT14B\nH5cyFcCRIbcqrwlmrawqaClX4gA/fPs3jgTMt0jKCpvZ3g6AAyTEEsweC5x9dY0q\n1oUK8+0CgYBZEy2VRSOffRuYdo0wik9Brk+al0iQTP8xSRSJ0b25SugKD1QyUCTL\n5qpeHhPq4kfcYDQXcRTivvVhy0+lYTm4MLLng7kwxszTY6/+oWwDuQB51i7+Mvv1\ngH55npdN+ZyT9sWUXXw+OCjF9as+oIq5MWP+DCO5+ORGg6FwVIdQQg==\n-----END RSA PRIVATE KEY-----\n";
    const RECEIVED: &[u8] = b"-----BEGIN RSA PRIVATE KEY-----\n MIIEpAIBAAKCAQEA1lD64NYODaseRc3unWmUv9A8TreUpVTFrWoabEyyEamol4JC Bo+Jkt/XZc0Mpi1SkuEb315hZVk9Sw15FoBcIBdTEcW7RQkuqDWSCLN5sJ4mcQnG Ea9X6DhVBs8WVPHf057rZuBgD+xsjv5GXNX5Wfdbde/2sCda69QL+SzTZyX5h+xo o8btQw/cJfmJVFvGp3S3S95yxbR0bGCvzeHLJ+QdgY0bQmYGSvsqaWD+oKHt8d+m kcLZNM7n14BdpZ+X4t50cejBAXptxOqy9Si0qmKSRy6ZwpyaY1YwMLe4xWR3O9TY gUMPXteIwpkzJmY5mdopt0InZtKWxaqyl2VVtwIDAQABAoIBAA93WZuHJw16d11P PrSSbX8oLmn52prR0tAwc4a0RDd9GGMSreQeUK3ORJOeqBaH98kMwL+uZnXzajZz 2vSyP7O7Ky4BqIGTZ5ARCaBiH1qNFzYaH/afwkbKZexvXmuz5tS1R8StVZFgnPb2 x6okcPG6pZmNvy44MNoK+/A3+WuckqBzSYzQoJ+N7xa3gGO53n/sCs6NM+/+U7un Z3SaYsMNpHcx06xyrEiiIhLu4UNqZNLS0JXoqMLHdgn1y/Cu0Y/RzKaYjziiBM7i 2zdrhRJ59GiIu348EszBS10nqmJBcAGldvUAp0KDtDgD+fdoBLb4Q6OhhrIpQ6Tu Vjozj40CgYEA8uzvx8ymoAfdJyV83NbiQ7/3wLQbU550f4nu49EhIld/XnmKNhPd nfeQlbRf1l+jj1Atx2Gzgn9vt1agMahbfESG+DMBJ7U90sV9F3sUR4u+tRWoczPL qm/Z4vQksR6wtf/ujjWrmqcxQN6AIlD/ObDtFGnrBNOKTtSi2DrjNlsCgYEA4dnc jqXOPxgJ4vLebH+EfLfFkGS3+MryWfF1LIZa/JS440CBZnrA/HVi6Rbs2TXjiT71 DM5KdvjpeFzxf5CJBu8b1mbywlf/JVrefIcOzB15kahvtoOUt7iWvKmdQg9XiHLl QZUXiVTJPC/5rBtrDX8pFB3xKqhJSv081gduFNUCgYEA58pmnKlMUCRQw5qEgp/S 87xHJSt1a5slI9C0DdKtH/yG/tlPpHveGW1wGzgpvf4xrXxewgEabKk2KqTQY7F1 4aPteQQJ1R6dp/BihLYeyomv5U+OFKZWbgEd0O0N2ackC/TxZeYWIJHpcDEd6CCK 9zw3ayQJaOiD7uX7oADIbkMCgYEAmsVAduS6/NPNWqASuol8UjdhhPzwcecpw2dL Y/LCgVqthNYclwzCGX+cA525O3TJc7EYHulzcCpNXDVdLYzvyjerfPbZn0LMT14B H5cyFcCRIbcqrwlmrawqaClX4gA/fPs3jgTMt0jKCpvZ3g6AAyTEEsweC5x9dY0q 1oUK8+0CgYBZEy2VRSOffRuYdo0wik9Brk+al0iQTP8xSRSJ0b25SugKD1QyUCTL 5qpeHhPq4kfcYDQXcRTivvVhy0+lYTm4MLLng7kwxszTY6/+oWwDuQB51i7+Mvv1 gH55npdN+ZyT9sWUXXw+OCjF9as+oIq5MWP+DCO5+ORGg6FwVIdQQg== \n-----END RSA PRIVATE KEY-----";

    #[test]
    fn the_recorded_password_paste_refuses_spaces_while_the_source_key_fits() {
        const KEY_BYTES: u32 = 1196;
        let source = pem(&Secret::new(SOURCE.into()), KEY_BYTES).expect("the box's complete 2048-bit key");
        assert_eq!(source.len(), 1192);
        assert!(pem(&Secret::new(RECEIVED.into()), KEY_BYTES).is_none());
    }
}
