//! Signed issuer documents and faults (oauth.md, 2.2, 5 and 7).
use crate::{Account, Config, Error, HttpEvidence, HttpResponse, Limits, Request, key};
use alloc::boxed::Box;
use skein_json::writer::{Encoder, Limits as WriterLimits};
use skein_lib::{
    Duration, Queue, Time, Wall, Writer,
    base64::{self, Alphabet},
    bytes,
};

/// The fake's `OpenID` issuer, its documents and one scripted fault.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct OpenId {
    pub issuer: Box<[u8]>,
    pub key_set_url: Box<[u8]>,
    pub subject: Box<[u8]>,
    pub lifetime: Duration,
    /// Include the signing key's identifier in the token header.
    pub header_kid: bool,
    pub fault: Option<Fault>,
}
/// One change to a document or claim (oauth.md, 2.2 and 5).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Fault {
    UnknownKid,
    AnotherKey,
    AlgNone,
    AlgHs256,
    TwoKeysNoKid,
    Issuer,
    Audience,
    Azp,
    TwoAudiences,
    Expired(Duration),
    Nonce,
    NoSubject,
    NoIdToken,
    LongModulus,
    UseEnc,
    ExtraEcKey,
    UnreadableMetadata,
    UnreadableKeySet,
    MetadataIssuer,
}
pub(super) fn configured(config: &Config) -> Option<&OpenId> {
    config.terms.as_ref()?.openid.as_ref()
}
pub(super) fn valid(openid: Option<&OpenId>, limits: &Limits) -> bool {
    let Some(openid) = openid else { return true };
    openid.issuer.len() <= usize::try_from(limits.uri_bytes).expect("u32 fits")
        && openid.key_set_url.len() <= usize::try_from(limits.uri_bytes).expect("u32 fits")
        && openid.subject.len() <= usize::try_from(limits.document.string_bytes).expect("u32 fits")
}
pub(super) fn worst_case(limits: &Limits) -> Option<u64> {
    // The decoded fixed key, RSA signing workspace and encoded signature fit 32 KiB.
    u64::from(limits.uri_bytes)
        .checked_mul(2)?
        .checked_add(u64::from(limits.document.string_bytes))?
        .checked_add(u64::from(limits.document.id_token_bytes).checked_mul(4)?)?
        .checked_add(32_768)
}

pub(super) fn get(
    config: &Config,
    limits: &Limits,
    id: u64,
    endpoint: &[u8],
    now: Time,
    wall: Wall,
    out: &mut Queue<Request>,
) {
    let result = document(config, limits, endpoint);
    let Ok((status, body)) = result else {
        out.push(Request::Refused);
        return;
    };
    out.push(Request::Http(HttpResponse {
        id,
        status,
        body,
        retry_after: Duration::ZERO,
        evidence: HttpEvidence::Response,
        now,
        wall,
    }));
}
fn document(config: &Config, limits: &Limits, endpoint: &[u8]) -> Result<(u16, Box<[u8]>), Error> {
    let Some(openid) = configured(config) else { return Ok((404, Box::default())) };
    if let Some(suffix) = endpoint.strip_prefix(openid.issuer.as_ref())
        && suffix == b"/.well-known/openid-configuration"
    {
        if openid.fault == Some(Fault::UnreadableMetadata) {
            return raw(limits, b"not JSON");
        }
        return Ok((200, encode(&Part::Metadata(openid, config), limits.document.document_bytes)?));
    }
    if endpoint == openid.key_set_url.as_ref() {
        if openid.fault == Some(Fault::UnreadableKeySet) {
            return raw(limits, b"not JSON");
        }
        let modulus = modulus(openid.fault)?;
        return Ok((200, encode(&Part::Keys(openid, &modulus), limits.document.document_bytes)?));
    }
    Ok((404, Box::default()))
}
fn raw(limits: &Limits, value: &[u8]) -> Result<(u16, Box<[u8]>), Error> {
    if value.len() > usize::try_from(limits.document.document_bytes).expect("u32 fits") {
        return Err(Error::Limits);
    }
    Ok((200, bytes::copy_of(value)))
}
fn modulus(fault: Option<Fault>) -> Result<Box<[u8]>, Error> {
    if fault == Some(Fault::AnotherKey) {
        let mut raw = base64::decode(Alphabet::Url, key::MODULUS.as_bytes(), 256).or(Err(Error::Plan))?;
        *raw.last_mut().expect("fixed modulus") ^= 2;
        return encoded(&raw);
    }
    if fault == Some(Fault::LongModulus) {
        let raw = base64::decode(Alphabet::Url, key::MODULUS.as_bytes(), 256).or(Err(Error::Plan))?;
        let mut out = Writer::new(512);
        out.put(&raw).expect("half modulus");
        out.put(&raw).expect("whole modulus");
        return encoded(&out.finish());
    }
    Ok(bytes::copy_of(key::MODULUS.as_bytes()))
}

pub(super) fn token(
    openid: &OpenId,
    limits: &Limits,
    client: &[u8],
    account: &Account,
    nonce: Option<&[u8]>,
    wall: Wall,
) -> Result<Option<Box<[u8]>>, Error> {
    if openid.fault == Some(Fault::NoIdToken) {
        return Ok(None);
    }
    let cap = limits.document.id_token_bytes;
    let header = encoded(&header(openid, limits)?)?;
    let payload = encoded(&claims(openid, limits, client, account, nonce, wall)?)?;
    let size = header.len().checked_add(payload.len()).ok_or(Error::Limits)?.checked_add(1).ok_or(Error::Limits)?;
    if size > usize::try_from(cap).expect("u32 fits") {
        return Err(Error::Limits);
    }
    let mut input = Writer::new(size);
    input.put(&header).expect("measured");
    input.put(b".").expect("measured");
    input.put(&payload).expect("measured");
    let input = input.finish();
    let signature = if openid.fault == Some(Fault::AlgNone) { Box::default() } else { sign(&input)? };
    let size = input.len().checked_add(signature.len()).ok_or(Error::Limits)?.checked_add(1).ok_or(Error::Limits)?;
    if size > usize::try_from(cap).expect("u32 fits") {
        return Err(Error::Limits);
    }
    let mut out = Writer::new(size);
    out.put(&input).expect("measured");
    out.put(b".").expect("measured");
    out.put(&signature).expect("measured");
    Ok(Some(out.finish()))
}
pub(super) fn header(openid: &OpenId, limits: &Limits) -> Result<Box<[u8]>, Error> {
    encode(&Part::Header(openid), limits.document.id_token_bytes)
}
pub(super) fn claims(
    openid: &OpenId,
    limits: &Limits,
    client: &[u8],
    account: &Account,
    nonce: Option<&[u8]>,
    wall: Wall,
) -> Result<Box<[u8]>, Error> {
    let seconds = match openid.fault {
        Some(Fault::Expired(ago)) => {
            wall.as_secs().checked_sub(ago.as_nanos().checked_div(1_000_000_000).ok_or(Error::Limits)?)
        }
        Some(
            Fault::UnknownKid
            | Fault::AnotherKey
            | Fault::AlgNone
            | Fault::AlgHs256
            | Fault::TwoKeysNoKid
            | Fault::Issuer
            | Fault::Audience
            | Fault::Azp
            | Fault::TwoAudiences
            | Fault::Nonce
            | Fault::NoSubject
            | Fault::NoIdToken
            | Fault::LongModulus
            | Fault::UseEnc
            | Fault::ExtraEcKey
            | Fault::UnreadableMetadata
            | Fault::UnreadableKeySet
            | Fault::MetadataIssuer,
        )
        | None => {
            wall.as_secs().checked_add(openid.lifetime.as_nanos().checked_div(1_000_000_000).ok_or(Error::Limits)?)
        }
    }
    .ok_or(Error::Limits)?;
    encode(&Part::Claims(openid, client, account, nonce, seconds), limits.document.id_token_bytes)
}
fn sign(input: &[u8]) -> Result<Box<[u8]>, Error> {
    let raw = base64::decode(Alphabet::Standard, key::PKCS8.as_bytes(), 2048).or(Err(Error::Plan))?;
    let signature = skein_tls::sign_rs256_pkcs8(&raw, input).ok_or(Error::Plan)?;
    encoded(&signature)
}
fn encoded(raw: &[u8]) -> Result<Box<[u8]>, Error> {
    let size =
        base64::encoded_len(Alphabet::Url, u32::try_from(raw.len()).or(Err(Error::Limits))?).ok_or(Error::Limits)?;
    let mut out = Writer::new(usize::try_from(size).expect("u32 fits"));
    base64::encode(Alphabet::Url, raw, &mut out).or(Err(Error::Limits))?;
    Ok(out.finish())
}

enum Part<'a> {
    Header(&'a OpenId),
    Claims(&'a OpenId, &'a [u8], &'a Account, Option<&'a [u8]>, u64),
    Metadata(&'a OpenId, &'a Config),
    Keys(&'a OpenId, &'a [u8]),
}
fn encode(part: &Part<'_>, cap: u32) -> Result<Box<[u8]>, Error> {
    let limits = WriterLimits { depth: 4, length: cap };
    let mut measure = Encoder::measure(&limits);
    write(&mut measure, part);
    let length = measure.measured().or(Err(Error::Limits))?;
    let mut out = Encoder::write(length, &limits);
    write(&mut out, part);
    Ok(out.finish())
}
fn text(out: &mut Encoder, name: &[u8], value: &[u8]) {
    out.key(name);
    out.string(value);
}
fn write(out: &mut Encoder, part: &Part<'_>) {
    out.object_start();
    match part {
        Part::Header(openid) => {
            let alg: &[u8] = if openid.fault == Some(Fault::AlgNone) {
                b"none"
            } else if openid.fault == Some(Fault::AlgHs256) {
                b"HS256"
            } else {
                b"RS256"
            };
            text(out, b"alg", alg);
            text(out, b"typ", b"JWT");
            if openid.header_kid && openid.fault != Some(Fault::TwoKeysNoKid) {
                text(out, b"kid", if openid.fault == Some(Fault::UnknownKid) { b"unknown" } else { key::KID });
            }
        }
        Part::Claims(openid, client, account, nonce, seconds) => {
            text(
                out,
                b"iss",
                if openid.fault == Some(Fault::Issuer) { b"https://other.example" } else { &openid.issuer },
            );
            out.key(b"aud");
            if openid.fault == Some(Fault::Azp) || openid.fault == Some(Fault::TwoAudiences) {
                out.array_start();
                out.string(client);
                out.string(b"other-client");
                out.array_end();
                text(out, b"azp", if openid.fault == Some(Fault::Azp) { b"other-client" } else { client });
            } else {
                out.string(if openid.fault == Some(Fault::Audience) { b"other-client" } else { client });
            }
            out.key(b"exp");
            out.unsigned(*seconds);
            if openid.fault == Some(Fault::Nonce) {
                text(out, b"nonce", b"other-nonce");
            } else if let Some(nonce) = nonce {
                text(out, b"nonce", nonce);
            }
            if openid.fault != Some(Fault::NoSubject) {
                text(out, b"sub", &account.subject);
            }
            if let Some(email) = &account.email {
                text(out, b"email", &email.address);
                out.key(b"email_verified");
                out.boolean(email.verified);
            }
            if let Some(name) = &account.name {
                text(out, b"name", name);
            }
        }
        Part::Metadata(openid, config) => {
            text(
                out,
                b"issuer",
                if openid.fault == Some(Fault::MetadataIssuer) { b"https://other.example" } else { &openid.issuer },
            );
            text(out, b"authorization_endpoint", &config.authorization_url);
            text(out, b"token_endpoint", &config.token_endpoint);
            text(out, b"jwks_uri", &openid.key_set_url);
        }
        Part::Keys(openid, modulus) => {
            out.key(b"keys");
            out.array_start();
            jwk(out, modulus, key::KID, openid.fault == Some(Fault::UseEnc));
            if openid.fault == Some(Fault::TwoKeysNoKid) {
                jwk(out, modulus, b"second", false);
            }
            if openid.fault == Some(Fault::ExtraEcKey) {
                out.object_start();
                text(out, b"kty", b"EC");
                text(out, b"crv", b"P-256");
                text(out, b"kid", b"ec");
                out.object_end();
            }
            out.array_end();
        }
    }
    out.object_end();
}
fn jwk(out: &mut Encoder, modulus: &[u8], kid: &[u8], enc: bool) {
    out.object_start();
    text(out, b"kty", b"RSA");
    text(out, b"kid", kid);
    text(out, b"use", if enc { b"enc" } else { b"sig" });
    text(out, b"alg", b"RS256");
    text(out, b"n", modulus);
    text(out, b"e", key::EXPONENT.as_bytes());
    out.object_end();
}
