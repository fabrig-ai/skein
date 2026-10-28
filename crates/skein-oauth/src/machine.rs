//! One bounded OAuth exchange (oauth.md, sections 1 and 3). It keeps the
//! registered endpoints, one exchange and its deadlines; it knows no provider
//! identity or token store. `Client::step` emits visits, HTTP demands and one
//! terminal. HTTP transport and persistence belong to the caller.

mod fetch;
mod revoke;

use crate::redirect::{form_fields, issued_id, missing_scopes, resource_field, validate_registration};
use crate::{ClaimSelector, DecodeError, Failure, Json, Limits, OAuthError, RefreshState, SavedToken, TokenResponse};
use crate::{classify, decode_error, decode_response, rotate};
use alloc::boxed::Box;
use skein_json::writer::Encoder;
use skein_lib::{Duration, List, Queue, Time, Wall, bytes};

/// Startup bounds for one client and exchange.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClientLimits {
    pub document: Limits,
    pub uri_bytes: u32,
    pub scope_bytes: u32,
    pub state_bytes: u32,
    pub code_bytes: u32,
    pub url_bytes: u32,
    pub request_bytes: u32,
    pub parameter_bytes: u32,
    pub resource_bytes: u32,
    pub nonce_bytes: u32,
    pub metadata_bytes: u32,
    pub key_set_bytes: u32,
    pub keys: u32,
    pub modulus_bits: u32,
    pub skew: Duration,
    pub sign_in_time: Duration,
    pub request_time: Duration,
    pub backoff_base: Duration,
    pub backoff_ceiling: Duration,
    pub max_attempts: u32,
}

/// Whether these limits are representable: every edge bound they size
/// is finite and every capacity they derive fits (programming-model.md, 6.3).
/// Exchange timing and key sizes are usable, saved records fit their schema,
/// and JSON input and request-field slots have representable lengths.
#[must_use]
pub fn client_valid(limits: &ClientLimits) -> bool {
    limits.max_attempts > 0
        && limits.request_time > Duration::ZERO
        && limits.sign_in_time > Duration::ZERO
        && (2048..=8192).contains(&limits.modulus_bits)
        && crate::record::record_limits(&limits.document).is_some()
        && limits.document.document_bytes < u32::MAX
        && limits.parameter_bytes.checked_add(11).is_some()
}

/// Upper bound for retained registration, exchange, request and answer bytes.
#[must_use]
pub fn client_worst_case(limits: &ClientLimits) -> Option<u64> {
    if !client_valid(limits) {
        return None;
    }
    crate::worst_case(&limits.document)?
        .checked_add(u64::from(limits.uri_bytes).checked_mul(7)?)?
        .checked_add(u64::from(limits.scope_bytes).checked_mul(2)?)?
        .checked_add(u64::from(limits.parameter_bytes))?
        .checked_add(
            u64::from(limits.parameter_bytes).checked_mul(u64::try_from(size_of::<crate::Parameter>()).ok()?)?,
        )?
        .checked_add(List::<Option<(&[u8], &[u8])>>::worst_case(limits.parameter_bytes.checked_add(11)?)?)?
        .checked_add(u64::from(limits.resource_bytes))?
        .checked_add(u64::from(limits.nonce_bytes))?
        .checked_add(u64::from(limits.state_bytes).checked_mul(2)?)?
        .checked_add(u64::from(limits.code_bytes))?
        .checked_add(u64::from(limits.url_bytes))?
        .checked_add(u64::from(limits.request_bytes).checked_mul(2)?)?
        .checked_add(u64::from(limits.metadata_bytes.max(limits.key_set_bytes)).checked_mul(2)?)?
        .checked_add(List::<crate::keys::Key>::worst_case(limits.keys)?)?
        .checked_add(
            u64::from(limits.keys).checked_mul(
                u64::from(limits.modulus_bits.checked_div(8)?)
                    .checked_add(u64::from(limits.document.string_bytes).checked_mul(2)?)?,
            )?,
        )?
        .checked_add(4096)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WireFormat {
    Json,
    Form,
}

/// Caller-provided registration, with no provider identity or discovery.
#[expect(missing_debug_implementations, reason = "registration may hold a client secret")]
pub struct Registration {
    pub authorization_url: Box<[u8]>,
    pub token_endpoint: Box<[u8]>,
    pub client_id: Box<[u8]>,
    pub redirect_uri: Box<[u8]>,
    pub scope: Box<[u8]>,
    pub wire: WireFormat,
    pub client_secret: Option<Box<[u8]>>,
    pub pkce_for_confidential: bool,
    pub metadata_claim: Option<ClaimSelector>,
    pub terms: Option<crate::Terms>,
}

/// What an HTTP demand asks of the caller's stack (oauth.md, section 3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Method {
    /// A token exchange or revocation.
    Post,
    /// The issuer's metadata or key set, without a body or secret.
    Get,
}

/// A complete bounded request for the caller's HTTP/TLS stack.
#[expect(missing_debug_implementations, reason = "request body contains credentials")]
pub struct HttpRequest {
    pub id: u64,
    pub method: Method,
    pub endpoint: Box<[u8]>,
    pub content_type: &'static [u8],
    pub body: Box<[u8]>,
    pub deadline: Time,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HttpEvidence {
    Unsent,
    Unknown,
    Response,
}

/// Terminal HTTP answer, including bounded body and transport evidence.
#[expect(missing_debug_implementations, reason = "answer may contain tokens")]
pub struct HttpResponse {
    pub id: u64,
    pub status: u16,
    pub body: Box<[u8]>,
    pub retry_after: Duration,
    pub evidence: HttpEvidence,
    pub now: Time,
    pub wall: Wall,
}

/// A redirect's values as its caller decoded them: the address without its query, and the
/// fields the client reads (oauth.md, sections 3 and 6.5).
#[expect(missing_debug_implementations, reason = "redirects carry state and codes")]
pub struct Redirect {
    pub uri: Box<[u8]>,
    pub state: Box<[u8]>,
    pub code: Option<Box<[u8]>>,
    pub error: Option<Box<[u8]>>,
    pub client_id: Option<Box<[u8]>>,
}

/// Why `Client::admits` would not take a redirect, in the order it checks (oauth.md, section 3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Inadmissible {
    /// No sign-in waits for a redirect: any phase but `AwaitRedirect`, or its deadline reached.
    NotWaiting,
    /// Not the bound address; both code and error or neither; or no required issued client id.
    Address,
    /// Another sign-in's state, compared in constant time.
    State,
}

#[expect(missing_debug_implementations, reason = "events can contain secrets")]
pub enum Event {
    SignIn {
        registration: Registration,
        key: u32,
        generation: u64,
        state: Box<[u8]>,
        verifier: Option<Box<[u8]>>,
        nonce: Option<Box<[u8]>>,
        port: Option<u16>,
        issued_client_id: Option<Box<[u8]>>,
        id_token: Option<Box<[u8]>>,
        subject: Option<Box<[u8]>>,
        now: Time,
    },
    Redirected {
        uri: Box<[u8]>,
        state: Box<[u8]>,
        code: Option<Box<[u8]>>,
        error: Option<Box<[u8]>>,
        client_id: Option<Box<[u8]>>,
        now: Time,
    },
    Refresh {
        registration: Registration,
        prior: RefreshState,
        now: Time,
    },
    Revoke {
        registration: Registration,
        prior: RefreshState,
        now: Time,
    },
    Http(HttpResponse),
    Tick {
        now: Time,
    },
    Cancel,
    Reset,
}

#[expect(missing_debug_implementations, reason = "requests can contain secrets")]
pub enum Request {
    Visit { url: Box<[u8]> },
    Http(HttpRequest),
    Tokens { record: SavedToken, missing: Box<[u8]> },
    Revoked { confirmed: bool },
    Failed { failure: Failure },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    Idle,
    AwaitRedirect,
    AwaitKeys { id: u64, deadline: Time },
    AwaitHttp { id: u64, deadline: Time },
    Backoff { due: Time },
    Done,
}

struct SignInValues {
    key: u32,
    generation: u64,
    state: Box<[u8]>,
    verifier: Option<Box<[u8]>>,
    nonce: Option<Box<[u8]>>,
    port: Option<u16>,
    issued_client_id: Option<Box<[u8]>>,
    id_token: Option<Box<[u8]>>,
    subject: Option<Box<[u8]>>,
    now: Time,
}

enum Grant {
    SignIn { key: u32, generation: u64, code: Option<Box<[u8]>> },
    Refresh { prior: RefreshState },
    Revoke { prior: RefreshState },
}

struct Exchange {
    registration: Registration,
    grant: Grant,
    state: Box<[u8]>,
    verifier: Option<Box<[u8]>>,
    nonce: Option<Box<[u8]>>,
    deadline: Time,
    attempts: u32,
    visit: Option<Box<[u8]>>,
    fetch: Option<fetch::Fetch>,
    keys: Box<[crate::keys::Key]>,
}

/// One sans-IO sign-in, refresh or revocation. The caller dispatches each entrance's output before the next.
#[expect(missing_debug_implementations, reason = "machine retains secrets")]
pub struct Client {
    limits: ClientLimits,
    phase: Phase,
    active: Option<Exchange>,
    next_id: u64,
}

impl Client {
    pub fn new(limits: ClientLimits) -> Result<Client, Failure> {
        if !client_valid(&limits) {
            return Err(Failure::Limit);
        }
        Ok(Client { limits, phase: Phase::Idle, active: None, next_id: 1 })
    }

    #[must_use]
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The next time the caller must send `Tick`, or none while idle or done.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        match self.phase {
            Phase::Idle | Phase::Done => None,
            Phase::AwaitRedirect => Some(self.active.as_ref()?.deadline),
            Phase::AwaitHttp { deadline, .. } | Phase::AwaitKeys { deadline, .. } => Some(deadline),
            Phase::Backoff { due } => Some(due),
        }
    }

    pub fn step(&mut self, event: Event, out: &mut Queue<Request>) {
        assert!(out.is_empty(), "the caller dispatches an entrance's output before the next");
        match event {
            Event::SignIn {
                registration,
                key,
                generation,
                state,
                verifier,
                nonce,
                port,
                issued_client_id,
                id_token,
                subject,
                now,
            } => {
                if self.phase != Phase::Idle {
                    return;
                }
                let result = self.sign_in(
                    registration,
                    SignInValues {
                        key,
                        generation,
                        state,
                        verifier,
                        nonce,
                        port,
                        issued_client_id,
                        id_token,
                        subject,
                        now,
                    },
                );
                self.emit_start(result, out);
            }
            Event::Refresh { registration, prior, now } => {
                if self.phase != Phase::Idle {
                    return;
                }
                let result = self.refresh(registration, prior, now);
                self.emit_start(result, out);
            }
            Event::Revoke { registration, prior, now } => {
                if self.phase == Phase::Idle {
                    self.revoke(registration, prior, now, out);
                }
            }
            Event::Redirected { uri, state, code, error, client_id, now } => {
                self.redirected(Redirect { uri, state, code, error, client_id }, now, out);
            }
            Event::Http(answer) => self.http(answer, out),
            Event::Tick { now } => self.tick(now, out),
            Event::Cancel => {
                if self.phase != Phase::Idle && self.phase != Phase::Done {
                    self.finish(Failure::Cancelled, out);
                }
            }
            Event::Reset => {
                if self.phase == Phase::Done {
                    self.active = None;
                    self.phase = Phase::Idle;
                }
            }
        }
    }

    fn emit_start(&mut self, result: Result<(Exchange, Request, Phase), Failure>, out: &mut Queue<Request>) {
        match result {
            Ok((exchange, request, phase)) => {
                self.active = Some(exchange);
                self.phase = phase;
                out.push(request);
            }
            Err(failure) => self.finish(failure, out),
        }
    }

    fn refresh(
        &mut self,
        mut registration: Registration,
        prior: RefreshState,
        now: Time,
    ) -> Result<(Exchange, Request, Phase), Failure> {
        validate_registration(&registration, &self.limits, false)?;
        if prior.refresh_token.is_empty()
            || prior.refresh_token.len() > usize::try_from(self.limits.document.token_bytes).expect("u32 fits")
        {
            return Err(Failure::Limit);
        }
        if let Err(error) = crate::record::validate_refresh_additions(&prior, &self.limits.document) {
            return Err(admit(error));
        }
        if issued_id(&registration) {
            let Some(client_id) = &prior.issued_client_id else { return Err(Failure::Refused) };
            if client_id.is_empty() {
                return Err(Failure::Refused);
            }
            registration.client_id.clone_from(client_id);
        }
        let deadline = now.checked_add(self.limits.sign_in_time).ok_or(Failure::Limit)?;
        let mut exchange = Exchange {
            registration,
            grant: Grant::Refresh { prior },
            state: bytes::copy_of(b""),
            verifier: None,
            nonce: None,
            deadline,
            attempts: 0,
            visit: None,
            fetch: None,
            keys: Box::default(),
        };
        let (request, phase) = self.http_request(&mut exchange, now)?;
        Ok((exchange, Request::Http(request), phase))
    }

    /// Whether `Redirected` would take this redirect at `now`; changes nothing (oauth.md, section 3).
    #[must_use = "the caller must judge admission before taking a redirect"]
    pub fn admits(&self, redirect: &Redirect, now: Time) -> Result<(), Inadmissible> {
        match self.check_redirect(redirect, now) {
            Ok(()) => Ok(()),
            Err((why, _failure)) => Err(why),
        }
    }

    fn check_redirect(&self, redirect: &Redirect, now: Time) -> Result<(), (Inadmissible, Failure)> {
        if self.phase != Phase::AwaitRedirect {
            return Err((Inadmissible::NotWaiting, Failure::InvalidRedirect));
        }
        let Some(exchange) = &self.active else {
            return Err((Inadmissible::NotWaiting, Failure::Malformed));
        };
        if now >= exchange.deadline {
            return Err((Inadmissible::NotWaiting, Failure::InvalidRedirect));
        }
        if redirect.uri != exchange.registration.redirect_uri {
            return Err((Inadmissible::Address, Failure::InvalidRedirect));
        }
        let code = present(redirect.code.as_deref());
        let error = present(redirect.error.as_deref());
        if code.is_some() == error.is_some() {
            return Err((Inadmissible::Address, Failure::Malformed));
        }
        if code.is_some() && issued_id(&exchange.registration) && present(redirect.client_id.as_deref()).is_none() {
            return Err((Inadmissible::Address, Failure::Incomplete));
        }
        if !skein_lib::sha256::equal(&redirect.state, &exchange.state) {
            return Err((Inadmissible::State, Failure::InvalidRedirect));
        }
        Ok(())
    }

    fn redirected(&mut self, redirect: Redirect, now: Time, out: &mut Queue<Request>) {
        if self.phase != Phase::AwaitRedirect {
            return;
        }
        if let Err((_why, failure)) = self.check_redirect(&redirect, now) {
            self.finish(failure, out);
            return;
        }
        let mut exchange = self.active.take().expect("admitted redirect's exchange");
        if present(redirect.error.as_deref()).is_some() {
            self.finish(Failure::Refused, out);
            return;
        }
        let code = redirect.code.expect("admitted redirect's code");
        let client_id = redirect.client_id;
        if code.len() > usize::try_from(self.limits.code_bytes).expect("u32 fits") {
            self.finish(Failure::Limit, out);
            return;
        }
        if issued_id(&exchange.registration) {
            let client_id = client_id.expect("admitted redirect's issued client id");
            if client_id.len() > usize::try_from(self.limits.document.issued_client_bytes).expect("u32 fits") {
                self.finish(Failure::Limit, out);
                return;
            }
            exchange.registration.client_id = client_id;
        }
        match &mut exchange.grant {
            Grant::SignIn { code: slot, .. } => *slot = Some(code),
            Grant::Refresh { .. } | Grant::Revoke { .. } => {
                self.finish(Failure::Malformed, out);
                return;
            }
        }
        match self.http_request(&mut exchange, now) {
            Ok((request, phase)) => {
                self.phase = phase;
                self.active = Some(exchange);
                out.push(Request::Http(request));
            }
            Err(failure) => self.finish(failure, out),
        }
    }

    fn http(&mut self, answer: HttpResponse, out: &mut Queue<Request>) {
        let (id, deadline) = match self.phase {
            Phase::AwaitKeys { id, deadline } => {
                if answer.id == id {
                    self.keys_answer(answer, deadline, out);
                }
                return;
            }
            Phase::AwaitHttp { id, deadline } => (id, deadline),
            Phase::Idle | Phase::AwaitRedirect | Phase::Backoff { .. } | Phase::Done => return,
        };
        if answer.id != id {
            return;
        }
        let Some(exchange) = self.active.take() else {
            self.finish(Failure::Malformed, out);
            return;
        };
        if exchange.revoking() {
            self.revocation_answer(exchange, answer, deadline, out);
            return;
        }
        if answer.now >= deadline || answer.now >= exchange.deadline {
            self.finish(Failure::TimedOut, out);
            return;
        }
        if answer.evidence == HttpEvidence::Unsent {
            self.retry_or_fail(exchange, Failure::Unavailable, answer.now, answer.retry_after, true, out);
            return;
        }
        if answer.body.len() > usize::try_from(self.limits.document.document_bytes).expect("u32 fits") {
            self.finish(Failure::Limit, out);
            return;
        }
        if (200..300).contains(&answer.status) {
            let response = match Json::from_bytes(&answer.body, &self.limits.document) {
                Ok(value) => decode_response(&value, &self.limits.document),
                Err(error) => Err(error),
            };
            match response {
                Ok(tokens) => match self.record(&exchange, &tokens, answer.wall) {
                    Ok(record) => {
                        self.phase = Phase::Done;
                        let missing = missing_scopes(&exchange.registration, &record);
                        out.push(Request::Tokens { record, missing });
                    }
                    Err(failure) => self.finish(failure, out),
                },
                Err(error) => self.finish(admit(error), out),
            }
            return;
        }
        let error: Result<Option<OAuthError>, DecodeError> = if answer.body.is_empty() {
            Ok(None)
        } else {
            match Json::from_bytes(&answer.body, &self.limits.document) {
                Ok(value) => match decode_error(&value, &self.limits.document) {
                    Ok(error) => Ok(Some(error)),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            }
        };
        let failure = match error {
            Ok(value) => {
                let mut added = false;
                if let (Some(terms), Some(error)) = (&exchange.registration.terms, &value) {
                    for name in &terms.sign_in_again {
                        if name == &error.code {
                            added = true;
                        }
                    }
                }
                if added { Failure::Refused } else { classify(answer.status, value.as_ref(), answer.retry_after) }
            }
            Err(_) => Failure::Malformed,
        };
        let safe = answer.status == 429 && answer.evidence == HttpEvidence::Response;
        self.retry_or_fail(exchange, failure, answer.now, answer.retry_after, safe, out);
    }

    fn record(&self, exchange: &Exchange, response: &TokenResponse, wall: Wall) -> Result<SavedToken, Failure> {
        let prior = match &exchange.grant {
            Grant::Refresh { prior } => {
                return match rotate(
                    prior,
                    response,
                    exchange.registration.metadata_claim.as_ref(),
                    wall,
                    &self.limits.document,
                ) {
                    Ok(record) => Ok(record),
                    Err(error) => Err(admit(error)),
                };
            }
            Grant::Revoke { .. } => return Err(Failure::Malformed),
            Grant::SignIn { key, generation, .. } => {
                let identity = match self.identity(exchange, response, wall) {
                    Ok(value) => value,
                    Err(check) => return Err(Failure::Unverified { check }),
                };
                SavedToken {
                    key: *key,
                    generation: *generation,
                    access_token: Box::new([]),
                    refresh_token: None,
                    metadata: None,
                    expires_at: wall,
                    earliest_refresh_at: None,
                    issued_client_id: if issued_id(&exchange.registration) {
                        Some(exchange.registration.client_id.clone())
                    } else {
                        None
                    },
                    granted_scopes: None,
                    subject: identity.subject,
                    id_token: identity.token,
                }
            }
        };
        match crate::record::complete(
            prior,
            response,
            exchange.registration.metadata_claim.as_ref(),
            wall,
            &self.limits.document,
        ) {
            Ok(record) => Ok(record),
            Err(error) => Err(admit(error)),
        }
    }

    fn retry_or_fail(
        &mut self,
        exchange: Exchange,
        failure: Failure,
        now: Time,
        retry_after: Duration,
        safe: bool,
        out: &mut Queue<Request>,
    ) {
        let transient = match failure {
            Failure::Unavailable | Failure::TimedOut | Failure::RateLimited { .. } => true,
            Failure::Refused
            | Failure::ClientRejected
            | Failure::Malformed
            | Failure::Incomplete
            | Failure::Unverified { .. }
            | Failure::InvalidRedirect
            | Failure::Limit
            | Failure::Cancelled => false,
        };
        if safe && transient && exchange.attempts < self.limits.max_attempts {
            let shift = u64::from(exchange.attempts.saturating_sub(1)).min(63);
            let factor = 1_u64.checked_shl(u32::try_from(shift).expect("shift fits")).unwrap_or(u64::MAX);
            let delay =
                self.limits.backoff_base.saturating_mul(factor).min(self.limits.backoff_ceiling).max(retry_after);
            if let Some(due) = now.checked_add(delay)
                && due < exchange.deadline
            {
                self.phase = Phase::Backoff { due };
                self.active = Some(exchange);
                return;
            }
        }
        if exchange.revoking() {
            self.revoked(false, out);
        } else {
            self.finish(if exchange.fetch.is_some() { fetch::failure() } else { failure }, out);
        }
    }

    fn tick(&mut self, now: Time, out: &mut Queue<Request>) {
        match self.phase {
            Phase::AwaitRedirect => {
                if let Some(value) = &self.active
                    && now >= value.deadline
                {
                    self.finish(Failure::TimedOut, out);
                }
            }
            Phase::AwaitKeys { deadline, .. } => {
                if now >= deadline {
                    self.keys_timeout(now, out);
                }
            }
            Phase::AwaitHttp { deadline, .. } => {
                if now >= deadline {
                    self.finish(Failure::TimedOut, out);
                }
            }
            Phase::Backoff { due } => {
                if now < due {
                    return;
                }
                let Some(mut exchange) = self.active.take() else {
                    self.finish(Failure::Malformed, out);
                    return;
                };
                match self.http_request(&mut exchange, now) {
                    Ok((request, phase)) => {
                        self.phase = phase;
                        self.active = Some(exchange);
                        out.push(Request::Http(request));
                    }
                    Err(failure) => {
                        if exchange.revoking() {
                            self.revoked(false, out);
                        } else {
                            self.finish(if exchange.fetch.is_some() { fetch::failure() } else { failure }, out);
                        }
                    }
                }
            }
            Phase::Idle | Phase::Done => {}
        }
    }

    fn http_request(&mut self, exchange: &mut Exchange, now: Time) -> Result<(HttpRequest, Phase), Failure> {
        if now >= exchange.deadline || exchange.attempts >= self.limits.max_attempts {
            return Err(Failure::TimedOut);
        }
        let deadline = now.checked_add(self.limits.request_time).ok_or(Failure::Limit)?.min(exchange.deadline);
        if exchange.fetch.is_some() {
            return self.keys_request(exchange, now, deadline);
        }
        let body = token_body(exchange, &self.limits)?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(Failure::Limit)?;
        exchange.attempts = exchange.attempts.checked_add(1).ok_or(Failure::Limit)?;
        let content_type = match (exchange.revoking(), exchange.registration.wire) {
            (false, WireFormat::Json) => b"application/json".as_slice(),
            (true, WireFormat::Json | WireFormat::Form) | (false, WireFormat::Form) => {
                b"application/x-www-form-urlencoded".as_slice()
            }
        };
        let request = HttpRequest {
            id,
            method: Method::Post,
            endpoint: bytes::copy_of(if exchange.revoking() {
                revoke::endpoint(&exchange.registration).ok_or(Failure::Malformed)?
            } else {
                &exchange.registration.token_endpoint
            }),
            content_type,
            body,
            deadline,
        };
        Ok((request, Phase::AwaitHttp { id, deadline }))
    }

    fn finish(&mut self, failure: Failure, out: &mut Queue<Request>) {
        if failure != Failure::Cancelled
            && let Some(exchange) = &self.active
            && exchange.revoking()
        {
            self.revoked(false, out);
            return;
        }
        self.active = None;
        self.phase = Phase::Done;
        out.push(Request::Failed { failure });
    }
}

fn admit(error: DecodeError) -> Failure {
    match error {
        DecodeError::TooLarge => Failure::Limit,
        DecodeError::Malformed | DecodeError::Missing | DecodeError::WrongType | DecodeError::Version => {
            Failure::Malformed
        }
    }
}

fn token_body(exchange: &Exchange, limits: &ClientLimits) -> Result<Box<[u8]>, Failure> {
    let registration = &exchange.registration;
    let (grant_type, name, value, redirect, verifier) = match &exchange.grant {
        Grant::SignIn { code: Some(code), .. } => (
            b"authorization_code".as_slice(),
            b"code".as_slice(),
            code.as_ref(),
            Some(registration.redirect_uri.as_ref()),
            exchange.verifier.as_deref(),
        ),
        Grant::SignIn { code: None, .. } => return Err(Failure::Malformed),
        Grant::Revoke { prior } => return revoke::body(registration, prior, limits),
        Grant::Refresh { prior } => {
            (b"refresh_token".as_slice(), b"refresh_token".as_slice(), prior.refresh_token.as_ref(), None, None)
        }
    };
    let fields = [
        resource_field(registration),
        Some((b"grant_type".as_slice(), grant_type)),
        Some((b"client_id".as_slice(), registration.client_id.as_ref())),
        Some((name, value)),
        match redirect {
            Some(item) => Some((b"redirect_uri".as_slice(), item)),
            None => None,
        },
        match verifier {
            Some(item) => Some((b"code_verifier".as_slice(), item)),
            None => None,
        },
        match registration.client_secret.as_deref() {
            Some(item) => Some((b"client_secret".as_slice(), item)),
            None => None,
        },
    ];
    match registration.wire {
        WireFormat::Form => form_fields(&fields, limits.request_bytes),
        WireFormat::Json => json_fields(&fields, limits),
    }
}

fn json_fields(fields: &[Option<(&[u8], &[u8])>], limits: &ClientLimits) -> Result<Box<[u8]>, Failure> {
    let writer_limits = skein_json::writer::Limits { depth: 2, length: limits.request_bytes };
    let mut measure = Encoder::measure(&writer_limits);
    measure.object_start();
    for field in fields {
        let Some((key, value)) = field else { continue };
        measure.key(key);
        measure.string(value);
    }
    measure.object_end();
    let length = match measure.measured() {
        Ok(length) => length,
        Err(skein_json::writer::Refusal::TooLong | skein_json::writer::Refusal::TooDeep) => return Err(Failure::Limit),
        Err(skein_json::writer::Refusal::Text | skein_json::writer::Refusal::Number) => return Err(Failure::Malformed),
    };
    let mut writer = Encoder::write(length, &writer_limits);
    writer.object_start();
    for field in fields {
        let Some((key, value)) = field else { continue };
        writer.key(key);
        writer.string(value);
    }
    writer.object_end();
    Ok(writer.finish())
}

fn present(value: Option<&[u8]>) -> Option<&[u8]> {
    match value {
        Some(value) if !value.is_empty() => Some(value),
        Some(_) | None => None,
    }
}
