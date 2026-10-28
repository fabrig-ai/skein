//! Independent bounded OAuth issuer. No IO, clock reads, or provider policy.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use skein_json::Token;
use skein_lib::{Duration, List, Queue, Time, Wall, Writer, bytes};
use skein_oauth::{
    HttpEvidence, HttpRequest, HttpResponse, Json, Limits as DocumentLimits, OAuthError, TokenResponse, challenge,
    encode_error, encode_response,
};

mod key;
mod openid;
pub use openid::{Fault, OpenId};
mod terms;
pub use terms::{ISSUED, Terms};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub document: DocumentLimits,
    pub uri_bytes: u32,
    pub request_bytes: u32,
    pub codes: u32,
    pub rotations: u32,
    pub plans: u32,
    /// Names retained from the last request beyond OAuth’s own fields.
    pub carried: u32,
}

#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let fields = 10_u32.checked_add(limits.carried)?;
    skein_oauth::worst_case(&limits.document)?
        .checked_add(terms::worst_case(limits)?)?
        .checked_add(List::<Field>::worst_case(fields)?)?
        .checked_add(
            u64::from(limits.codes)
                .checked_mul(u64::from(limits.uri_bytes).checked_add(u64::from(limits.document.string_bytes))?)?,
        )?
        .checked_add(List::<Code>::worst_case(limits.codes)?)?
        .checked_add(
            u64::from(limits.codes)
                .checked_add(1)?
                .checked_mul(3)?
                .checked_mul(u64::from(limits.document.string_bytes))?,
        )?
        .checked_add(u64::from(limits.codes).checked_mul(59)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.rotations)?)?
        .checked_add(Queue::<Plan>::worst_case(limits.plans)?)?
        .checked_add(u64::from(limits.document.token_bytes).checked_mul(u64::from(limits.rotations).checked_add(8)?)?)?
        .checked_add(u64::from(limits.document.document_bytes).checked_mul(u64::from(limits.plans).checked_add(2)?)?)?
        .checked_add(u64::from(limits.uri_bytes).checked_mul(4)?)?
        .checked_add(u64::from(limits.request_bytes).checked_mul(2)?)
}

#[expect(missing_debug_implementations, reason = "issuer configuration contains secrets")]
pub struct Config {
    pub authorization_url: Box<[u8]>,
    pub token_endpoint: Box<[u8]>,
    pub client_id: Box<[u8]>,
    pub client_secret: Option<Box<[u8]>>,
    pub redirect_uri: Box<[u8]>,
    pub refresh_token: Box<[u8]>,
    /// Optional issued-registration and revocation terms.
    pub terms: Option<Terms>,
}

#[expect(missing_debug_implementations, reason = "response plans contain tokens")]
pub enum Body {
    Token(TokenResponse),
    Error(OAuthError),
    Raw(Box<[u8]>),
}

#[expect(missing_debug_implementations, reason = "response plans contain tokens")]
pub struct Plan {
    pub status: u16,
    pub body: Body,
    pub delay: Duration,
    pub retry_after: Duration,
}

#[expect(missing_debug_implementations, reason = "events contain codes or token requests")]
pub enum Event {
    Authorize { url: Box<[u8]>, now: Time },
    Post { request: HttpRequest, now: Time, wall: Wall },
    Get { id: u64, endpoint: Box<[u8]>, now: Time, wall: Wall },
    Tick { now: Time, wall: Wall },
    LoseResponse,
}

#[expect(missing_debug_implementations, reason = "outputs contain codes or tokens")]
pub enum Request {
    Redirect { uri: Box<[u8]>, state: Box<[u8]>, code: Box<[u8]>, client_id: Option<Box<[u8]>> },
    Http(HttpResponse),
    Refused,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    Limits,
    Full,
    Plan,
}

/// Who signs in at the fake's authorization (oauth.md, 5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Account {
    pub subject: Box<[u8]>,
    pub email: Option<Email>,
    pub name: Option<Box<[u8]>>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Email {
    pub address: Box<[u8]>,
    pub verified: bool,
}

struct Code {
    value: Box<[u8]>,
    challenge: Option<Box<[u8]>>,
    redirect_uri: Box<[u8]>,
    nonce: Option<Box<[u8]>>,
    account: Option<Account>,
    used: bool,
    expires: Time,
}
struct Pending {
    id: u64,
    status: u16,
    body: Box<[u8]>,
    retry_after: Duration,
    due: Time,
    revoke: bool,
}
struct Field {
    key: Box<[u8]>,
    value: Box<[u8]>,
}

#[expect(missing_debug_implementations, reason = "issuer retains rotating credentials")]
pub struct Issuer {
    config: Config,
    account: Option<Account>,
    limits: Limits,
    plans: Queue<Plan>,
    codes: List<Code>,
    spent: List<Box<[u8]>>,
    generation: u64,
    posts: u64,
    next_code: u64,
    pending: Option<Pending>,
    carried: List<Box<[u8]>>,
    authorizations: usize,
    revoked: bool,
}

impl Issuer {
    pub fn new(config: Config, limits: Limits) -> Result<Issuer, Error> {
        let secret_too_large = match &config.client_secret {
            Some(value) => value.len() > usize::try_from(limits.document.client_bytes).expect("u32 fits"),
            None => false,
        };
        if worst_case(&limits).is_none()
            || limits.codes == 0
            || limits.rotations == 0
            || config.client_id.is_empty()
            || config.refresh_token.is_empty()
            || config.client_id.len() > usize::try_from(limits.document.client_bytes).expect("u32 fits")
            || config.refresh_token.len() > usize::try_from(limits.document.token_bytes).expect("u32 fits")
            || config.authorization_url.len() > usize::try_from(limits.uri_bytes).expect("u32 fits")
            || config.token_endpoint.len() > usize::try_from(limits.uri_bytes).expect("u32 fits")
            || config.redirect_uri.len() > usize::try_from(limits.uri_bytes).expect("u32 fits")
            || !terms::valid(config.terms.as_ref(), &limits)
            || secret_too_large
        {
            return Err(Error::Limits);
        }
        #[expect(clippy::manual_map, reason = "programming-model.md, 10.3 forbids closures in step code")]
        let account = match openid::configured(&config) {
            Some(openid) => Some(Account { subject: openid.subject.clone(), email: None, name: None }),
            None => None,
        };
        Ok(Issuer {
            config,
            account,
            limits,
            plans: Queue::with_capacity(limits.plans),
            codes: List::with_capacity(limits.codes),
            spent: List::with_capacity(limits.rotations),
            generation: 0,
            posts: 0,
            next_code: 1,
            pending: None,
            carried: List::with_capacity(limits.carried),
            authorizations: 0,
            revoked: false,
        })
    }

    /// The account the following authorizations sign in, until another is set.
    /// `Err(Error::Limits)` for an empty subject or any part past `limits.document.string_bytes`;
    /// `Err(Error::Plan)` without `OpenID` terms.
    pub fn sign_in_as(&mut self, account: Account) -> Result<(), Error> {
        if openid::configured(&self.config).is_none() {
            return Err(Error::Plan);
        }
        let cap = usize::try_from(self.limits.document.string_bytes).expect("u32 fits");
        let email_too_large = match &account.email {
            Some(email) => email.address.len() > cap,
            None => false,
        };
        let name_too_large = match &account.name {
            Some(name) => name.len() > cap,
            None => false,
        };
        if account.subject.is_empty() || account.subject.len() > cap || email_too_large || name_too_large {
            return Err(Error::Limits);
        }
        self.account = Some(account);
        Ok(())
    }

    pub fn queue(&mut self, plan: Plan) -> Result<(), Error> {
        if self.plans.room() == 0 {
            return Err(Error::Full);
        }
        if !(200..600).contains(&plan.status) {
            return Err(Error::Plan);
        }
        match &plan.body {
            Body::Token(value) => {
                if encode_response(value, &self.limits.document).is_err() {
                    return Err(Error::Plan);
                }
            }
            Body::Error(value) => {
                if encode_error(value, &self.limits.document).is_err() {
                    return Err(Error::Plan);
                }
            }
            Body::Raw(value) => {
                if value.len() > usize::try_from(self.limits.document.document_bytes).expect("u32 fits") {
                    return Err(Error::Plan);
                }
            }
        }
        self.plans.push(plan);
        Ok(())
    }

    #[must_use]
    pub const fn posts(&self) -> u64 {
        self.posts
    }
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        Some(self.pending.as_ref()?.due)
    }

    /// Names beyond OAuth’s own fields in the last authorization or POST, in order.
    #[must_use]
    pub fn carried(&self) -> &[Box<[u8]>] {
        self.carried.as_slice()
    }

    /// Access tokens in queued successful token plans; the script mints none.
    #[must_use]
    pub fn granted(&self) -> Box<[Box<[u8]>]> {
        let mut tokens = List::with_capacity(self.plans.len());
        for plan in &self.plans {
            if (200..300).contains(&plan.status) {
                match &plan.body {
                    Body::Token(token) => {
                        tokens.push(token.access_token.clone()).expect("one per plan");
                    }
                    Body::Error(_) | Body::Raw(_) => {}
                }
            }
        }
        tokens.into_boxed()
    }

    pub fn step(&mut self, event: Event, out: &mut Queue<Request>) {
        match event {
            Event::Authorize { url, now } => self.authorize(&url, now, out),
            Event::Post { request, now, wall } => self.post(request, now, wall, out),
            Event::Get { id, endpoint, now, wall } => {
                openid::get(&self.config, &self.limits, id, &endpoint, now, wall, out);
            }
            Event::Tick { now, wall } => self.deliver(now, wall, out),
            Event::LoseResponse => self.pending = None,
        }
    }

    fn authorize(&mut self, url: &[u8], now: Time, out: &mut Queue<Request>) {
        self.carried.clear();
        let Some(query_at) = bytes::find(url, b"?") else {
            out.push(Request::Refused);
            return;
        };
        let Some(base) = url.get(..query_at) else {
            out.push(Request::Refused);
            return;
        };
        if base != self.config.authorization_url.as_ref() {
            out.push(Request::Refused);
            return;
        }
        let Some(query) = url.get(query_at.saturating_add(1)..) else {
            out.push(Request::Refused);
            return;
        };
        let Ok(fields) = form_fields(query, &self.limits) else {
            out.push(Request::Refused);
            return;
        };
        if !terms::observe(&fields, &mut self.carried, &self.limits) {
            out.push(Request::Refused);
            return;
        }
        let redirect = field(&fields, b"redirect_uri");
        if let Some(nonce) = field(&fields, b"nonce")
            && nonce.len() > usize::try_from(self.limits.document.string_bytes).expect("u32 fits")
        {
            out.push(Request::Refused);
            return;
        }
        if field(&fields, b"response_type") != Some(b"code".as_slice())
            || !terms::authorization_client(&self.config, self.authorizations, field(&fields, b"client_id"))
            || !terms::redirect(&self.config, redirect)
            || terms::too_long(redirect, self.limits.uri_bytes)
            || self.codes.room() == 0
        {
            out.push(Request::Refused);
            return;
        }
        let Some(state) = field(&fields, b"state") else {
            out.push(Request::Refused);
            return;
        };
        if state.len() < 16 || state.len() > usize::try_from(self.limits.document.string_bytes).expect("u32 fits") {
            out.push(Request::Refused);
            return;
        }
        let challenge = field(&fields, b"code_challenge");
        let method = field(&fields, b"code_challenge_method");
        if self.config.client_secret.is_none() && (challenge.is_none() || method != Some(b"S256".as_slice())) {
            out.push(Request::Refused);
            return;
        }
        if challenge.is_some() && method != Some(b"S256".as_slice()) {
            out.push(Request::Refused);
            return;
        }
        let code = code_bytes(self.next_code);
        let Some(next) = self.next_code.checked_add(1) else {
            out.push(Request::Refused);
            return;
        };
        self.next_code = next;
        let entry = Code {
            value: code.clone(),
            challenge: copy_optional(challenge),
            redirect_uri: bytes::copy_of(redirect.expect("redirect checked")),
            nonce: copy_optional(field(&fields, b"nonce")),
            account: self.account.clone(),
            used: false,
            expires: now.saturating_add(Duration::from_secs(120)),
        };
        if self.codes.push(entry).is_err() {
            out.push(Request::Refused);
            return;
        }
        self.authorizations = self.authorizations.saturating_add(1);
        out.push(Request::Redirect {
            uri: bytes::copy_of(redirect.expect("redirect checked")),
            client_id: terms::issued(&self.config, self.authorizations),
            state: bytes::copy_of(state),
            code,
        });
    }

    fn post(&mut self, request: HttpRequest, now: Time, wall: Wall, out: &mut Queue<Request>) {
        self.carried.clear();
        if self.pending.is_some() {
            out.push(Request::Refused);
            return;
        }
        let Some(posts) = self.posts.checked_add(1) else {
            out.push(Request::Refused);
            return;
        };
        self.posts = posts;
        let revocation = terms::revocation(&self.config, &request.endpoint);
        if (!revocation && request.endpoint != self.config.token_endpoint)
            || request.body.len() > usize::try_from(self.limits.request_bytes).expect("u32 fits")
        {
            self.refuse(request.id, now, wall, out);
            return;
        }
        let fields = if request.content_type == b"application/json" {
            json_fields(&request.body, &self.limits)
        } else if request.content_type == b"application/x-www-form-urlencoded" {
            form_fields(&request.body, &self.limits)
        } else {
            Err(Error::Plan)
        };
        let Ok(fields) = fields else {
            self.refuse(request.id, now, wall, out);
            return;
        };
        if !terms::observe(&fields, &mut self.carried, &self.limits) {
            out.push(Request::Refused);
            return;
        }
        if field(&fields, b"client_id") != Some(terms::latest(&self.config, self.authorizations))
            || field(&fields, b"client_secret") != self.config.client_secret.as_deref()
        {
            self.refuse(request.id, now, wall, out);
            return;
        }
        let accepted = self.accepted(&fields, revocation, now);
        if !accepted {
            self.refuse(request.id, now, wall, out);
            return;
        }
        let Some(plan) = self.plans.pop() else {
            out.push(Request::Refused);
            return;
        };
        let (body, next_refresh) = match plan.body {
            Body::Token(mut value) => {
                if self.add_id_token(&mut value, &fields, revocation, plan.status, wall).is_err() {
                    out.push(Request::Refused);
                    return;
                }
                let next =
                    if !revocation && (200..300).contains(&plan.status) { value.refresh_token.clone() } else { None };
                let Ok(body) = encode_response(&value, &self.limits.document) else {
                    out.push(Request::Refused);
                    return;
                };
                (body, next)
            }
            Body::Error(value) => {
                let Ok(body) = encode_error(&value, &self.limits.document) else {
                    out.push(Request::Refused);
                    return;
                };
                (body, None)
            }
            Body::Raw(value) => (value, None),
        };
        if let Some(next) = next_refresh
            && next != self.config.refresh_token
        {
            if self.spent.room() == 0 {
                out.push(Request::Refused);
                return;
            }
            for old in self.spent.as_slice() {
                if old == &next {
                    out.push(Request::Refused);
                    return;
                }
            }
            let Some(generation) = self.generation.checked_add(1) else {
                out.push(Request::Refused);
                return;
            };
            let old = core::mem::replace(&mut self.config.refresh_token, next);
            self.spent.push(old).expect("room checked");
            self.generation = generation;
        }
        self.pending = Some(Pending {
            id: request.id,
            status: plan.status,
            body,
            retry_after: plan.retry_after,
            due: now.saturating_add(plan.delay),
            revoke: revocation && (200..300).contains(&plan.status),
        });
        self.deliver(now, wall, out);
    }

    fn add_id_token(
        &self,
        value: &mut TokenResponse,
        fields: &List<Field>,
        revocation: bool,
        status: u16,
        wall: Wall,
    ) -> Result<(), Error> {
        if !revocation
            && (200..300).contains(&status)
            && value.id_token.is_none()
            && field(fields, b"grant_type") == Some(b"authorization_code".as_slice())
            && let Some(openid) = openid::configured(&self.config)
        {
            let code = self.code(fields).ok_or(Error::Plan)?;
            value.id_token = openid::token(
                openid,
                &self.limits,
                field(fields, b"client_id").expect("accepted client"),
                code.account.as_ref().ok_or(Error::Plan)?,
                code.nonce.as_deref(),
                wall,
            )?;
        }
        Ok(())
    }

    #[expect(clippy::manual_find, reason = "programming-model.md, 10.3 forbids closures in step code")]
    fn code(&self, fields: &List<Field>) -> Option<&Code> {
        for entry in self.codes.as_slice() {
            if Some(entry.value.as_ref()) == field(fields, b"code") {
                return Some(entry);
            }
        }
        None
    }

    fn accepted(&mut self, fields: &List<Field>, revocation: bool, now: Time) -> bool {
        if revocation {
            !self.revoked
                && field(fields, b"token") == Some(self.config.refresh_token.as_ref())
                && field(fields, b"token_type_hint") == Some(b"refresh_token".as_slice())
        } else {
            match field(fields, b"grant_type") {
                Some(b"refresh_token") => self.refresh(fields),
                Some(b"authorization_code") => self.redeem(fields, now),
                Some(_) | None => false,
            }
        }
    }

    fn refresh(&self, fields: &List<Field>) -> bool {
        !self.revoked && field(fields, b"refresh_token") == Some(self.config.refresh_token.as_ref())
    }

    fn redeem(&mut self, fields: &List<Field>, now: Time) -> bool {
        let Some(code) = field(fields, b"code") else { return false };
        for index in 0..self.codes.len() {
            let Some(entry) = self.codes.get_mut(index) else { return false };
            if entry.value.as_ref() == code && !entry.used && entry.expires > now {
                if field(fields, b"redirect_uri") != Some(entry.redirect_uri.as_ref()) {
                    return false;
                }
                if let Some(expected) = &entry.challenge {
                    let Some(verifier) = field(fields, b"code_verifier") else { return false };
                    let Ok(got) = challenge(verifier) else { return false };
                    if got != *expected {
                        return false;
                    }
                }
                entry.used = true;
                return true;
            }
        }
        false
    }

    fn refuse(&mut self, id: u64, now: Time, wall: Wall, out: &mut Queue<Request>) {
        let error =
            OAuthError { code: bytes::copy_of(b"invalid_grant"), detail: bytes::copy_of(b"credential refused") };
        let body = match encode_error(&error, &self.limits.document) {
            Ok(body) => body,
            Err(_) => bytes::copy_of(b"{}"),
        };
        self.pending = Some(Pending { id, status: 400, body, retry_after: Duration::ZERO, due: now, revoke: false });
        self.deliver(now, wall, out);
    }

    fn deliver(&mut self, now: Time, wall: Wall, out: &mut Queue<Request>) {
        let Some(pending) = self.pending.take() else { return };
        if now < pending.due {
            self.pending = Some(pending);
            return;
        }
        if pending.revoke {
            self.revoked = true;
        }
        out.push(Request::Http(HttpResponse {
            id: pending.id,
            status: pending.status,
            body: pending.body,
            retry_after: pending.retry_after,
            evidence: HttpEvidence::Response,
            now,
            wall,
        }));
    }
}

fn code_bytes(number: u64) -> Box<[u8]> {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = [0_u8; 16];
    for (at, slot) in out.iter_mut().enumerate() {
        let shift = u32::try_from(15_usize.saturating_sub(at).saturating_mul(4)).expect("shift fits");
        let nibble = usize::try_from((number >> shift) & 15_u64).expect("nibble fits");
        *slot = *HEX.get(nibble).expect("hex digit");
    }
    bytes::copy_of(&out)
}

fn field<'a>(fields: &'a List<Field>, key: &[u8]) -> Option<&'a [u8]> {
    let mut found = None;
    for entry in fields.as_slice() {
        if entry.key.as_ref() == key {
            if found.is_some() {
                return None;
            }
            found = Some(entry.value.as_ref());
        }
    }
    found
}

fn json_fields(body: &[u8], limits: &Limits) -> Result<List<Field>, Error> {
    let value = Json::from_bytes(body, &limits.document).or(Err(Error::Plan))?;
    let tokens = value.as_tokens();
    if tokens.first() != Some(&Token::ObjectStart) || tokens.last() != Some(&Token::ObjectEnd) {
        return Err(Error::Plan);
    }
    let mut fields = List::with_capacity(10_u32.checked_add(limits.carried).ok_or(Error::Limits)?);
    let mut at = 1_usize;
    while at < tokens.len().saturating_sub(1) {
        let Some(Token::Key(key)) = tokens.get(at) else { return Err(Error::Plan) };
        let Some(Token::String(value)) = tokens.get(at.saturating_add(1)) else { return Err(Error::Plan) };
        fields.push(Field { key: key.clone(), value: value.clone() }).or(Err(Error::Plan))?;
        at = at.saturating_add(2);
    }
    Ok(fields)
}

fn form_fields(body: &[u8], limits: &Limits) -> Result<List<Field>, Error> {
    if body.len() > usize::try_from(limits.request_bytes).expect("u32 fits") {
        return Err(Error::Plan);
    }
    let mut fields = List::with_capacity(10_u32.checked_add(limits.carried).ok_or(Error::Limits)?);
    let mut at = 0_usize;
    loop {
        let remaining = body.get(at..).ok_or(Error::Plan)?;
        let end = match bytes::find(remaining, b"&") {
            Some(offset) => offset,
            None => remaining.len(),
        };
        let pair = remaining.get(..end).ok_or(Error::Plan)?;
        let Some(equal) = bytes::find(pair, b"=") else { return Err(Error::Plan) };
        let key = percent_decode(pair.get(..equal).ok_or(Error::Plan)?, limits.request_bytes)?;
        let value = percent_decode(pair.get(equal.saturating_add(1)..).ok_or(Error::Plan)?, limits.request_bytes)?;
        fields.push(Field { key, value }).or(Err(Error::Plan))?;
        if end == remaining.len() {
            break;
        }
        let position = at.checked_add(end).ok_or(Error::Plan)?;
        at = position.checked_add(1).ok_or(Error::Plan)?;
    }
    Ok(fields)
}

fn percent_decode(value: &[u8], cap: u32) -> Result<Box<[u8]>, Error> {
    if value.len() > usize::try_from(cap).expect("u32 fits") {
        return Err(Error::Plan);
    }
    let mut size = 0_usize;
    let mut at = 0_usize;
    while at < value.len() {
        size = size.checked_add(1).ok_or(Error::Plan)?;
        let byte = *value.get(at).ok_or(Error::Plan)?;
        at = at.checked_add(if byte == b'%' { 3 } else { 1 }).ok_or(Error::Plan)?;
        if at > value.len() {
            return Err(Error::Plan);
        }
    }
    let mut writer = Writer::new(size);
    at = 0;
    while at < value.len() {
        let byte = *value.get(at).ok_or(Error::Plan)?;
        if byte == b'%' {
            let high = hex(*value.get(at.saturating_add(1)).ok_or(Error::Plan)?).ok_or(Error::Plan)?;
            let low = hex(*value.get(at.saturating_add(2)).ok_or(Error::Plan)?).ok_or(Error::Plan)?;
            writer.put(&[(high << 4_u32) | low]).expect("measured decode");
            at = at.saturating_add(3);
        } else {
            writer.put(&[if byte == b'+' { b' ' } else { byte }]).expect("measured decode");
            at = at.saturating_add(1);
        }
    }
    Ok(writer.finish())
}

fn copy_optional(value: Option<&[u8]>) -> Option<Box<[u8]>> {
    let value = value?;
    Some(bytes::copy_of(value))
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(byte.wrapping_sub(b'a').wrapping_add(10)),
        b'A'..=b'F' => Some(byte.wrapping_sub(b'A').wrapping_add(10)),
        _ => None,
    }
}
