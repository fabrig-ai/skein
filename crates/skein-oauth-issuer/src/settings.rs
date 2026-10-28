use crate::{Client, Error, Issuer, Settings, documents, form};
use skein_lib::{List, bytes};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub clients: u32,
    pub uri_bytes: u32,
    pub client_bytes: u32,
    pub request_bytes: u32,
    pub state_bytes: u32,
    pub token_bytes: u32,
    pub claim_bytes: u32,
}

/// Whether these limits are representable: every edge bound they size
/// is finite and every capacity they derive fits (programming-model.md, 6.3).
/// Answers and discovery fit the encoder's u32 lengths; form slots fit u32.
#[must_use]
pub fn valid(limits: &Limits) -> bool {
    document_bounds(limits).is_some()
}

fn document_bounds(limits: &Limits) -> Option<()> {
    let uri = u64::from(limits.uri_bytes);
    let client = u64::from(limits.client_bytes);
    let state = u64::from(limits.state_bytes);
    let token = u64::from(limits.token_bytes);
    // JSON's longest spelling is six bytes per input byte. Include all four
    // person strings, issuer, audience, nonce, member names and two u64s.
    let claims = u64::from(limits.claim_bytes)
        .checked_mul(4)?
        .checked_add(uri)?
        .checked_add(client)?
        .checked_add(state)?
        .checked_mul(6)?
        .checked_add(1024)?;
    let jwt = claims.checked_add(2)?.checked_div(3)?.checked_mul(4)?.checked_add(1024)?;
    let answer = jwt.checked_add(token.checked_mul(6)?)?.checked_add(128)?;
    // The encoder's document length is u32; reject settings whose maximum
    // documents could not be represented before any call is admitted.
    u32::try_from(answer).ok()?;
    u32::try_from(uri.checked_mul(30)?.checked_add(2048)?).ok()?;
    limits.request_bytes.checked_add(1)?;
    Some(())
}

/// Settings held plus the largest call, including its output and temporaries.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !valid(limits) {
        return None;
    }
    let uri = u64::from(limits.uri_bytes);
    let client = u64::from(limits.client_bytes);
    let state = u64::from(limits.state_bytes);
    let token = u64::from(limits.token_bytes);
    // JSON's longest spelling is six bytes per input byte. Include all four
    // person strings, issuer, audience, nonce, member names and two u64s.
    let claims = u64::from(limits.claim_bytes)
        .checked_mul(4)?
        .checked_add(uri)?
        .checked_add(client)?
        .checked_add(state)?
        .checked_mul(6)?
        .checked_add(1024)?;
    let jwt = claims.checked_add(2)?.checked_div(3)?.checked_mul(4)?.checked_add(1024)?;
    let answer = jwt.checked_add(token.checked_mul(6)?)?.checked_add(128)?;
    // The encoder's document length is u32; reject settings whose maximum
    // documents could not be represented before any call is admitted.
    u32::try_from(answer).ok()?;
    u32::try_from(uri.checked_mul(30)?.checked_add(2048)?).ok()?;
    let held = uri
        .checked_mul(5)?
        .checked_add(u64::from(limits.clients).checked_mul(client.checked_mul(2)?.checked_add(uri)?)?)?
        .checked_add(List::<Client>::worst_case(limits.clients)?)?
        .checked_add(u64::try_from(skein_tls::RS256_PKCS8_KEY_BYTES).ok()?)?
        .checked_add(683 + 7 + 43)?;
    // Compact signing input, encoded payload, raw claims, JWT and answer
    // coexist at most in this sum. ring's fixed-size RSA work and the
    // encoder stacks fit 32 KiB, measured in the bound tests.
    let signing = claims.checked_add(jwt.checked_mul(3)?)?.checked_add(answer)?.checked_add(32_768)?;
    // Every form field owns its decoded name and value. Empty fields are
    // ignored; at most request_bytes + 1 slots, and two copies for Basic.
    let parsing = List::<form::Field>::worst_case(limits.request_bytes.checked_add(1)?)?
        .checked_add(u64::from(limits.request_bytes).checked_mul(4)?)?;
    let redirect = uri.checked_add(token.checked_add(state)?.checked_mul(3)?)?.checked_add(64)?;
    let discovery = uri.checked_mul(30)?.checked_add(2048)?;
    held.checked_add(signing.max(parsing).max(redirect).max(discovery))
}

impl Issuer {
    pub fn new(mut settings: Settings, limits: Limits) -> Result<Issuer, Error> {
        if !valid(&limits) {
            return Err(Error::Limits);
        }
        for url in [
            &settings.issuer,
            &settings.authorization_endpoint,
            &settings.token_endpoint,
            &settings.userinfo_endpoint,
            &settings.jwks_uri,
        ] {
            bounded(url, limits.uri_bytes)?;
            if !address(url) {
                return Err(Error::Address);
            }
        }
        if settings.issuer.contains(&b'?') || settings.issuer.ends_with(b"/") {
            return Err(Error::Address);
        }
        if settings.clients.len() > limits.clients {
            return Err(Error::Limits);
        }
        if settings.clients.is_empty() {
            return Err(Error::Client);
        }
        for (index, client) in settings.clients.iter().enumerate() {
            if client.id.is_empty() || client.secret.is_empty() {
                return Err(Error::Client);
            }
            bounded(&client.id, limits.client_bytes)?;
            bounded(client.secret.expose(), limits.client_bytes)?;
            for &byte in &client.id {
                if !(0x21..=0x7e).contains(&byte) {
                    return Err(Error::Client);
                }
            }
            bounded(&client.redirect_uri, limits.uri_bytes)?;
            if !address(&client.redirect_uri) {
                return Err(Error::Address);
            }
            for prior in settings.clients.as_slice().get(..index).expect("prior clients") {
                if prior.id == client.id {
                    return Err(Error::Client);
                }
            }
        }
        let public = skein_tls::rs256_public(settings.key.expose()).ok_or(Error::Key)?;
        let modulus = documents::encoded(&public.modulus)?;
        let exponent = documents::encoded(&public.exponent)?;
        let thumbprint = documents::thumbprint(&modulus, &exponent)?;
        let kid = documents::encoded(&skein_lib::sha256::digest(&thumbprint))?;
        let clients = core::mem::replace(&mut settings.clients, List::with_capacity(0)).into_boxed();
        Ok(Issuer { settings, clients, limits, modulus, exponent, kid })
    }

    #[expect(clippy::manual_find, reason = "step code uses a bounded scan without closures")]
    pub(crate) fn client(&self, id: &[u8]) -> Option<&Client> {
        for client in &self.clients {
            if client.id.as_ref() == id {
                return Some(client);
            }
        }
        None
    }
}

pub(crate) fn bounded(value: &[u8], cap: u32) -> Result<(), Error> {
    if value.len() > usize::try_from(cap).expect("u32 fits usize") { Err(Error::Limits) } else { Ok(()) }
}
pub(crate) fn optional(value: Option<&[u8]>, cap: u32) -> Result<(), Error> {
    if let Some(value) = value {
        bounded(value, cap)?;
    }
    Ok(())
}

// URLs have an authority, no userinfo or fragment, and visible URI bytes.
fn address(uri: &[u8]) -> bool {
    for &byte in uri {
        if !(0x21..=0x7e).contains(&byte) || byte == b'#' || byte == b'\\' {
            return false;
        }
    }
    let (rest, secure) = if let Some(rest) = uri.strip_prefix(b"https://") {
        (rest, true)
    } else if let Some(rest) = uri.strip_prefix(b"http://") {
        (rest, false)
    } else {
        return false;
    };
    let mut end = rest.len();
    for separator in [b"/".as_slice(), b"?"] {
        if let Some(at) = bytes::find(rest, separator) {
            end = end.min(at);
        }
    }
    let authority = rest.get(..end).expect("authority ends within URL");
    if authority.is_empty() || authority.contains(&b'@') {
        return false;
    }
    let (host, suffix) = if let Some(ipv6) = authority.strip_prefix(b"[") {
        let Some(end) = bytes::find(ipv6, b"]") else {
            return false;
        };
        let host = ipv6.get(..end).expect("bracket in authority");
        let suffix = ipv6.get(end.checked_add(1).expect("bracket in bounded URI")..).expect("after bracket");
        // A bracketed host is IPv6, whose grammar core validates without IO.
        if !ipv6_host(host) {
            return false;
        }
        (authority.get(..end.checked_add(2).expect("brackets in bounded URI")).expect("host"), suffix)
    } else {
        let end = bytes::find(authority, b":").unwrap_or(authority.len());
        let host = authority.get(..end).expect("host");
        for &byte in host {
            if !byte.is_ascii_alphanumeric() && byte != b'.' && byte != b'-' {
                return false;
            }
        }
        (host, authority.get(end..).expect("port suffix"))
    };
    if host.is_empty() {
        return false;
    }
    if suffix.is_empty() {
        return secure;
    }
    let Some(port) = suffix.strip_prefix(b":") else {
        return false;
    };
    if !port_number(port) {
        return false;
    }
    secure || host == b"127.0.0.1" || host == b"[::1]" || host == b"localhost"
}

#[expect(clippy::disallowed_methods, reason = "bounded IPv6 host parsing performs no IO")]
fn ipv6_host(host: &[u8]) -> bool {
    use core::str::FromStr;
    let Ok(host) = core::str::from_utf8(host) else {
        return false;
    };
    core::net::Ipv6Addr::from_str(host).is_ok()
}
fn port_number(port: &[u8]) -> bool {
    if port.is_empty() {
        return false;
    }
    let mut number = 0_u16;
    for &byte in port {
        if !byte.is_ascii_digit() {
            return false;
        }
        let Some(tens) = number.checked_mul(10) else {
            return false;
        };
        let Some(next) = tens.checked_add(u16::from(byte.wrapping_sub(b'0'))) else {
            return false;
        };
        number = next;
    }
    number != 0
}
