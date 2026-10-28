//! One LLM exchange over a caller-owned plaintext stream.
//!
//! Reserve [`MAX_OUT`] before each entry point and drain [`resume`] while
//! [`Client::has_work`] holds. `Next` demands one data event; terminal faults
//! need no demand. On `Close`, close the real stream and acknowledge its
//! settlement with [`closed`]. The caller owns deadlines and retry policy.
//! The client keeps its measured request, bounded response and observed status;
//! it knows no socket, credential store or reason to retry. Admission and
//! receiving failures retain their configured cap (llm.md, section 2.5).

use alloc::boxed::Box;
use core::mem;
use core::mem::size_of;

use skein_http::{Header, MaxOut, Method, client as http, sse};
use skein_json::{Document, collector, tokenizer};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Decimal, Env, List, Queue, Token, Writer, bytes};

use crate::{
    Block, Call, Completion, Credential, Delta, Encoding, Endpoint, Error, Failure, Prompt, Provider, Report, Scheme,
    anthropic, dialect, filter, openai, translate,
};

/// Maximum retained bytes of each provider report value (llm.md, section 4.2).
pub const REPORT_BYTES: u32 = 128;

/// Startup bounds, unchanged for a client's lifetime and reuse.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub http: http::Limits,
    pub sse: sse::Limits,
    /// Bytes of one encoded request.
    pub request: u32,
    /// Model-written bytes in one completion, excluding opaque reasoning.
    pub answer: u32,
    /// Argument bytes of one call; larger inputs become oversize blocks.
    pub input: u32,
    /// Bytes of one provider reasoning item.
    pub reasoning: u32,
    /// Bytes of other replay metadata belonging to one block.
    pub metadata: u32,
    /// Blocks and content indexes in one completion.
    pub output_items: u32,
    /// Longest retained decoded string.
    pub strings: u32,
    /// Retained tokens of one event.
    pub tokens: u32,
    /// Retained decoded text of one event.
    pub retained: u32,
    /// Answer reservation beside the measured request in the owner's pool.
    pub receiving: u32,
    /// Scanned wire bytes of one event, never buffered as a whole.
    pub skip: u32,
    /// Tools in one request.
    pub tools: u32,
    /// Messages and blocks in one request.
    pub history_items: u32,
    pub depth: u32,
    pub detail_bytes: u32,
    /// A non-success response body. At its cap the connection is closed.
    pub error_bytes: u32,
    /// Drop Codex reasoning past its cap; Anthropic thinking still fails.
    /// Default for direct prepare; explicit per-call preparation overrides it.
    pub drop_reasoning: bool,
    /// The endpoint model's declared completion ceiling, in tokens.
    pub declared_output_tokens: u32,
    /// Tool calls the caller accepts in one completion.
    pub calls_per_response: u32,
}

impl Limits {
    /// The native codec's independent limits for this endpoint's dialect.
    #[must_use]
    pub const fn native(&self) -> openai::Limits {
        openai::Limits {
            calls_per_response: self.calls_per_response,
            request: self.request,
            answer: self.answer,
            input: self.input,
            reasoning: self.reasoning,
            metadata: self.metadata,
            output_items: self.output_items,
            strings: self.strings,
            tokens: self.tokens,
            retained: self.retained,
            receiving: self.receiving,
            skip: self.skip,
            tools: self.tools,
            history_items: self.history_items,
            depth: self.depth,
            detail_bytes: self.detail_bytes,
        }
    }
}

/// The owner's byte bounds on credential and account values at one endpoint.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CredentialLimits {
    pub value: u32,
    pub account_id: u32,
}

/// Check credential bytes before constructing the bounded request head.
pub fn check_credential(credential: &Credential, limits: &CredentialLimits) -> Result<(), Error> {
    if credential.value.len() > usize::try_from(limits.value).expect("u32 fits usize") {
        return Err(Error::limit(crate::Cap::Value, limits.value));
    }
    if credential.account_id.len() > usize::try_from(limits.account_id).expect("u32 fits usize") {
        return Err(Error::limit(crate::Cap::AccountId, limits.account_id));
    }
    Ok(())
}

/// Largest collector demand the endpoint's SSE data face must admit.
#[must_use]
pub fn largest_event_demand(limits: &Limits) -> u32 {
    tokenizer::largest_demand(&filter::collector(limits).tokenizer)
}

/// Measure the largest request head from endpoint configuration and owner bounds
/// (llm-connection.md, section 3), without copying a credential or body.
pub fn request_head(endpoint: &Endpoint, credential: &CredentialLimits, limits: &Limits) -> Result<u32, Error> {
    // The native Messages grammar admits nonzero u32 max_tokens; Codex does
    // not send this route field (llm.md, sections 4.3 and 4.5).
    match endpoint.provider {
        Provider::OpenAiCodex | Provider::OpenAi => {}
        Provider::Anthropic => {
            if limits.declared_output_tokens == 0 {
                return Err(Error::limit(crate::Cap::Output, limits.declared_output_tokens));
            }
        }
    }
    if credential.value == 0 {
        return Err(Error::limit(crate::Cap::Value, credential.value));
    }
    let maximum_affinity = [[b'0'; 36]; 2];
    let fields = provider_headers(endpoint, Some(Scheme::Bearer), &[], Some(&maximum_affinity));
    let account_bytes = match endpoint.provider {
        Provider::OpenAiCodex => usize::try_from(credential.account_id).expect("u32 fits usize"),
        Provider::Anthropic | Provider::OpenAi => 0,
    };
    let fixed = match endpoint.encoding {
        Encoding::Identity => 5,
        Encoding::Gzip => 6,
    };
    let mut count = endpoint
        .headers
        .len()
        .checked_add(fixed)
        .ok_or(Error::limit(crate::Cap::ResponseFields, limits.http.headers))?;
    for field in &fields {
        if field.is_some() {
            count = count.checked_add(1).ok_or(Error::limit(crate::Cap::ResponseFields, limits.http.headers))?;
        }
    }
    if count > usize::try_from(limits.http.headers).expect("u32 fits usize") {
        return Err(Error::limit(crate::Cap::ResponseFields, limits.http.headers));
    }
    let measured = head_length(
        endpoint,
        usize::try_from(credential.value).expect("u32 fits usize"),
        account_bytes,
        // A compressed body's length stays below the request bound and a
        // body sent as rendered within it, so the bound fixes the digits.
        endpoint.encoding,
        u64::from(limits.request),
        Some(Scheme::Bearer),
        &fields,
    )?;
    match u32::try_from(measured) {
        Ok(measured) => Ok(measured),
        Err(_) => Err(Error::Limit { which: crate::Cap::RequestHead, bound: u64::from(u32::MAX) }),
    }
}

const ROUTES: u32 = 8;

/// Each entry point's bound. Buffered output requires another `resume`.
pub const MAX_OUT: MaxOut = MaxOut { above: 4, below: 20 };
pub const UP_MAX_OUT: MaxOut = MAX_OUT;
pub const DOWN_MAX_OUT: MaxOut = MAX_OUT;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Request {
    Start,
    /// One delta, block or completion. A duplicate outstanding demand is inert.
    Next,
    Cancel,
    /// An unfinished call is cancelled; an idle/terminal one is just closed.
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Evidence {
    Unsent,
    Unknown,
    /// The peer sent this HTTP status before the failure.
    Response {
        status: u16,
    },
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Event {
    Delta {
        owner: Token,
        delta: Delta,
    },
    Block {
        owner: Token,
        block: Block,
    },
    Completed {
        owner: Token,
        completion: Completion,
    },
    Failed {
        owner: Token,
        failure: Failure,
        evidence: Evidence,
        report: Report,
        detail: Box<[u8]>,
    },
    Cancelled {
        owner: Token,
    },
    /// The HTTP body has drained. Only now may the binding be reused.
    Reusable,
    /// The owner must close the actual stream and later call `closed`.
    Close,
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Waiting {
    Start,
    /// Local routing work must run before external input is accepted.
    Ready,
    /// Upload room or a response head, as the HTTP machine describes.
    Http(http::Waiting),
    Next,
    Response,
    Draining,
    Idle,
    Closing,
    Nothing,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Prepared,
    Head,
    Streaming,
    ErrorBody,
    Draining,
    Idle,
    Closing,
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Outcome {
    Pending,
    Terminal,
    Cancelled,
}

#[derive(Debug)]
enum EventData {
    Idle,
    First,
    Json,
    Sentinel(u32),
    SentinelEnd,
    Collected(Document),
}

/// A single bound call; no socket, TLS object or OAuth refresh state.
#[expect(missing_debug_implementations, reason = "the HTTP machine retains bearer credentials in its request head")]
pub struct Client {
    owner: Token,
    provider: Provider,
    authority: Box<[u8]>,
    limits: Limits,
    state: State,
    outcome: Outcome,
    http: http::Client,
    sse: sse::Reader,
    decoder: Option<dialect::Decoder>,
    content: Option<List<Block>>,
    content_bytes: u64,
    call: Option<http::Call>,
    upload: Option<Box<[u8]>>,
    upload_offset: usize,
    /// The end of the bytes sent within `upload`: the compressed member
    /// fills the front of the room its bound set aside.
    upload_end: usize,
    http_events: Queue<http::Event>,
    sse_events: Queue<sse::Event>,
    outputs: Queue<dialect::Output>,
    requests: Queue<http::Request>,
    sse_below: Queue<Down>,
    collector: collector::Collector,
    collector_events: Queue<collector::Event>,
    collector_below: Queue<Down>,
    event_data: EventData,
    error: List<u8>,
    status: u16,
    request: Box<[u8]>,
    rate: openai::RateLimit,
    asked: bool,
    close_sent: bool,
    sse_closed: bool,
    evidence: Evidence,
    activity: u64,
    measured: Measured,
}

/// One admitted request's wire lengths, before its encoded body exists.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Measured {
    /// Encoded HTTP request head bytes; for a compressed body, the most its
    /// head takes, with the compressed body's bound as `Content-Length`.
    pub head: u32,
    /// Encoded JSON request body bytes.
    pub body: u32,
    /// The room the compressed copy of the body takes on a gzip endpoint,
    /// held beside the rendered body while it is compressed; zero when the
    /// body is sent as rendered.
    pub compressed: u32,
}

/// An owned request and the exact configuration under which it was admitted.
/// Only `measure_request` constructs it; preparation consumes it without
/// accepting replacement input, endpoint, limits or reasoning policy.
#[expect(missing_debug_implementations, reason = "the measured request owns bearer credentials")]
pub struct MeasuredRequest {
    input: Call,
    limits: Limits,
    drop_reasoning: bool,
    measured: Measured,
}

impl MeasuredRequest {
    /// The lengths reserved before the body is encoded.
    #[must_use]
    pub const fn measured(&self) -> Measured {
        self.measured
    }
}

/// Checks and measures an owned request once, retaining its admission inputs.
pub fn measure_request(input: Call, limits: &Limits, drop_reasoning: bool) -> Result<MeasuredRequest, Error> {
    let measured = measure(&input.prompt, input.credential.as_ref(), &input.endpoint, limits)?;
    Ok(MeasuredRequest { input, limits: *limits, drop_reasoning, measured })
}

/// Validate exactly the preparation entrance, without allocating its body.
/// The borrowed prompt and credential remain the owner's while it waits.
pub fn measure(
    prompt: &Prompt,
    credential: Option<&Credential>,
    endpoint: &Endpoint,
    limits: &Limits,
) -> Result<Measured, Error> {
    check_preparation(limits)?;
    let body = match endpoint.provider {
        Provider::OpenAiCodex | Provider::OpenAi => {
            translate::measure_request(endpoint.provider, prompt, &limits.native())?
        }
        Provider::Anthropic => anthropic::measure_request(prompt, limits.declared_output_tokens, &limits.native())?,
    };
    let compressed = compressed(endpoint, body, limits);
    let head = measure_headers(
        endpoint,
        credential,
        prompt.affinity,
        limits,
        coding(compressed),
        content_length(body, compressed),
    )?;
    Ok(Measured { head, body, compressed })
}

/// Bytes one call holds from the grant until its terminal (llm-connection.md, 7).
#[must_use]
pub fn reservation(measured: Measured, limits: &Limits) -> Option<u64> {
    u64::from(measured.head)
        .checked_add(u64::from(measured.body))?
        .checked_add(u64::from(measured.compressed))?
        .checked_add(u64::from(limits.receiving))
}

/// The pool must hold one request at its endpoint's largest declared bounds.
/// A gzip endpoint compresses a body only while the body and its compressed
/// copy's room together stay within `request` (llm.md, section 4.8), so no
/// call's body and room pass `request`, whatever the endpoint's encoding.
#[must_use]
pub fn largest_reservation(limits: &Limits) -> Option<u64> {
    reservation(Measured { head: limits.http.request, body: limits.request, compressed: 0 }, limits)
}

/// The room the endpoint's compressed copy of a body takes beside it: on a
/// gzip endpoint the body's bound while the two fit within `request`, and
/// zero otherwise, when the body is sent as rendered (llm.md, section 4.8).
fn compressed(endpoint: &Endpoint, body: u32, limits: &Limits) -> u32 {
    let room = match endpoint.encoding {
        Encoding::Identity => return 0,
        Encoding::Gzip => match skein_http::gzip::bound(usize::try_from(body).expect("u32 fits usize")) {
            Some(room) => u64::try_from(room).expect("usize fits u64"),
            None => return 0,
        },
    };
    match u64::from(body).checked_add(room) {
        Some(both) if both <= u64::from(limits.request) => u32::try_from(room).expect("within a u32 bound"),
        Some(_) | None => 0,
    }
}

/// The coding a body measured so is sent with.
fn coding(compressed: u32) -> Encoding {
    if compressed == 0 { Encoding::Identity } else { Encoding::Gzip }
}

/// The most the `Content-Length` of a body measured so can say.
fn content_length(body: u32, compressed: u32) -> u64 {
    if compressed == 0 { u64::from(body) } else { u64::from(compressed) }
}

/// The head and body bytes sent for a request measured so whose body, as
/// encoded, is `sent` long: a gzip head was measured with the bound's
/// `Content-Length` digits, so the sent length's digits replace them.
fn sent_bytes(measured: Measured, sent: u64) -> u64 {
    let digits = |length: u64| u64::try_from(Decimal::of(length).as_bytes().len()).expect("digits fit u64");
    let bytes = || {
        let head = u64::from(measured.head).checked_sub(digits(content_length(measured.body, measured.compressed)))?;
        head.checked_add(digits(sent))?.checked_add(sent)
    };
    bytes().expect("a measured head counts its Content-Length digits")
}

fn check_preparation(limits: &Limits) -> Result<(), Error> {
    if limits.http.headers < 6 {
        return Err(Error::limit(crate::Cap::ResponseFields, limits.http.headers));
    }
    for (which, bound) in [
        (crate::Cap::ErrorBody, limits.error_bytes),
        (crate::Cap::OutputItems, limits.output_items),
        (crate::Cap::Depth, limits.depth),
        (crate::Cap::Tokens, limits.tokens),
    ] {
        if bound == 0 {
            return Err(Error::limit(which, bound));
        }
    }
    if !valid(limits) {
        return Err(Error::Invalid);
    }
    Ok(())
}

impl Client {
    /// Validates and measures before touching a connection or holding the
    /// final encoded body. The caller binds its transport to this endpoint.
    pub fn prepare(input: Call, limits: &Limits) -> Result<Client, Error> {
        Self::prepare_with_reasoning_drop(input, limits, limits.drop_reasoning)
    }

    /// Prepares one call with an explicit Codex reasoning policy. False keeps
    /// the typed limit failure even when the endpoint default permits dropping.
    /// Stored endpoint bounds remain unchanged for connection reuse.
    pub fn prepare_with_reasoning_drop(input: Call, limits: &Limits, enabled: bool) -> Result<Client, Error> {
        let request = measure_request(input, limits, enabled)?;
        Self::prepare_measured(request)
    }

    /// Consumes the checked request, retaining its input and configuration.
    /// The encoder finishes only when its writer filled exactly the measured
    /// body; the resulting lengths come from those bytes and a fresh head check.
    pub fn prepare_measured(request: MeasuredRequest) -> Result<Client, Error> {
        let MeasuredRequest { input, limits, drop_reasoning: enabled, measured } = request;
        let limits = &limits;
        let provider = input.endpoint.provider;
        let affinity = input.prompt.affinity;
        let rendered = match provider {
            Provider::OpenAiCodex | Provider::OpenAi => {
                translate::encode_measured_request(&input.prompt, &limits.native(), measured.body)?
            }
            Provider::Anthropic => anthropic::encode_measured_request(
                &input.prompt,
                limits.declared_output_tokens,
                &limits.native(),
                measured.body,
            )?,
        };
        let length = u32::try_from(rendered.len()).expect("an encoded request fits its measured bound");
        let compressed = compressed(&input.endpoint, length, limits);
        let head = measure_headers(
            &input.endpoint,
            input.credential.as_ref(),
            affinity,
            limits,
            coding(compressed),
            content_length(length, compressed),
        )?;
        let measured = Measured { head, body: length, compressed };
        // The compressed copy takes the room its bound reserved beside the
        // rendered body, which is dropped once it is compressed.
        let (body, end) = match coding(compressed) {
            Encoding::Identity => {
                let end = rendered.len();
                (rendered, end)
            }
            Encoding::Gzip => {
                let mut room = bytes::zeroed(usize::try_from(compressed).expect("u32 fits usize"));
                let end = skein_http::gzip::encode(&rendered, &mut room).expect("the room holds the body's bound");
                drop(rendered);
                (room, end)
            }
        };
        let headers = headers(
            &input.endpoint,
            input.credential.as_ref(),
            affinity,
            limits,
            coding(compressed),
            u64::try_from(end).expect("a body length fits u64"),
        )?;
        let call = http::Call {
            method: Method::Post,
            target: input.endpoint.target,
            headers,
            body: http::Body::Length(u64::try_from(end).expect("a body length fits u64")),
            close: false,
        };
        Ok(Client {
            owner: input.owner,
            provider: input.endpoint.provider,
            authority: input.endpoint.authority,
            limits: *limits,
            state: State::Prepared,
            outcome: Outcome::Pending,
            http: http::Client::new(&limits.http),
            sse: sse::Reader::new(&event_stream(limits)),
            decoder: Some(dialect::Decoder::new(provider, &limits.native(), enabled)),
            content: Some(List::with_capacity(limits.output_items)),
            content_bytes: 0,
            call: Some(call),
            upload: Some(body),
            upload_offset: 0,
            upload_end: end,
            http_events: Queue::uncapped(),
            sse_events: Queue::uncapped(),
            outputs: Queue::uncapped(),
            requests: Queue::uncapped(),
            sse_below: Queue::uncapped(),
            collector: collector::Collector::new(
                dialect::event_filter(provider),
                &filter::collector(limits),
                &filter::caps(limits),
            )
            .expect("the dialect's static filter is unambiguous"),
            collector_events: Queue::uncapped(),
            collector_below: Queue::uncapped(),
            event_data: EventData::Idle,
            error: List::with_capacity(limits.error_bytes),
            status: 0,
            request: Box::new([]),
            rate: openai::RateLimit { retry_after: None, reset: None, exhausted: false },
            asked: false,
            close_sent: false,
            sse_closed: false,
            evidence: Evidence::Unsent,
            activity: 0,
            measured,
        })
    }

    /// The request lengths admitted and encoded by this client: exact, but
    /// for a gzip endpoint's head, which is measured at the body's bound.
    #[must_use]
    pub const fn measured(&self) -> Measured {
        self.measured
    }

    /// The bytes this request takes on the wire: its head and its body as
    /// encoded, compressed on a gzip endpoint (llm-connection.md, section 4).
    #[must_use]
    pub fn request_bytes(&self) -> u64 {
        sent_bytes(self.measured, u64::try_from(self.upload_end).expect("a body length fits u64"))
    }

    #[must_use]
    pub const fn owner(&self) -> Token {
        self.owner
    }

    /// The number of complete SSE messages decoded into model output: parts,
    /// deltas or a terminal, counted at dispatch before `Next` drains them.
    /// Pings and other messages yielding only progress or no output do not count.
    #[must_use]
    pub const fn activity(&self) -> u64 {
        self.activity
    }

    /// The response head has arrived, whether or not the caller demanded its
    /// first output. The connection owner clears its head deadline here.
    #[must_use]
    pub fn response_received(&self) -> bool {
        match self.evidence {
            Evidence::Response { .. } => true,
            Evidence::Unsent | Evidence::Unknown => false,
        }
    }

    /// True only for runnable work, never for output blocked on `Next`.
    #[must_use]
    pub fn has_work(&self) -> bool {
        if self.state == State::Closing || self.state == State::Closed {
            return false;
        }
        if !self.outputs.is_empty() {
            return self.asked;
        }
        if !self.http_events.is_empty() || !self.sse_events.is_empty() || !self.requests.is_empty() {
            return true;
        }
        if self.state == State::Streaming && self.asked {
            if let Some(decoder) = &self.decoder
                && decoder.has_ready()
            {
                return true;
            }
            return self.sse.waiting() == sse::Waiting::Next || self.sse.waiting() == sse::Waiting::Above;
        }
        false
    }

    #[must_use]
    pub fn waiting(&self) -> Waiting {
        if self.has_work() {
            return Waiting::Ready;
        }
        match self.state {
            State::Prepared => Waiting::Start,
            State::Head => Waiting::Http(self.http.waiting()),
            State::Streaming if self.asked => Waiting::Response,
            State::Streaming => Waiting::Next,
            State::ErrorBody => Waiting::Response,
            State::Draining => Waiting::Draining,
            State::Idle => Waiting::Idle,
            State::Closing => Waiting::Closing,
            State::Closed => Waiting::Nothing,
        }
    }

    /// Reuses the HTTP machine and its carry-over, on the same authority and
    /// provider, only after `Reusable`. Refusal returns the prepared owner.
    #[expect(clippy::result_large_err, reason = "the owned admitted call is returned intact on refusal")]
    pub fn next_call(&mut self, mut prepared: Client) -> Result<(), Client> {
        if self.state != State::Idle
            || self.has_work()
            || self.http.waiting() != http::Waiting::Call
            || prepared.state != State::Prepared
            || self.provider != prepared.provider
            || self.authority != prepared.authority
            || self.limits != prepared.limits
        {
            return Err(prepared);
        }
        mem::swap(&mut self.http, &mut prepared.http);
        *self = prepared;
        Ok(())
    }
}

pub fn down(
    client: &mut Client,
    env: &Env<Limits>,
    request: Request,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    check_env(client, env);
    match request {
        Request::Start => {
            if client.state == State::Prepared {
                let call = client.call.take().expect("a prepared client holds its request");
                client.state = State::Head;
                http_down(client, env, http::Request::Call(call), below);
                client
                    .requests
                    .push(http::Request::Upload(Down::Demand { read: Read::Nothing, room: env.limits.http.send }));
            }
        }
        Request::Next => {
            if client.outcome == Outcome::Pending
                && (client.state == State::Prepared || client.state == State::Head || client.state == State::Streaming)
            {
                client.asked = true;
            }
        }
        Request::Cancel | Request::Close => {
            if client.outcome == Outcome::Pending {
                client.outcome = Outcome::Cancelled;
            }
            closing(client, env, above, below);
        }
    }
    resume(client, env, above, below);
}

pub fn up(client: &mut Client, env: &Env<Limits>, event: Up, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    check_env(client, env);
    match client.state {
        State::Prepared | State::Closing | State::Closed => return,
        State::Head | State::Streaming | State::ErrorBody | State::Draining | State::Idle => {}
    }
    let transport_failed = match &event {
        Up::Failed(_) => true,
        Up::Bytes(_) | Up::End | Up::Room => false,
    };
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.http };
    let before = below.len();
    http::up(&mut client.http, &child, event, &mut client.http_events, below);
    sent_evidence(client, below, before);
    // A peer failure ends the accepted attempt even when ordered provider
    // output is parked on an undemanded Next. Data backpressure must not
    // postpone faults or settlement of the actual lower binding.
    let queued = queued_http_failure(client);
    // A failed stream is `Unavailable` whatever the HTTP machine made of it;
    // the queued error, if any, says how far the exchange had gone.
    let failure = if transport_failed {
        let detail = match queued {
            Some((_, detail)) => detail,
            None => bytes::copy_of(b"connection failed"),
        };
        Some((Failure::Unavailable, detail))
    } else {
        queued
    };
    if let Some((failure, detail)) = failure {
        fail(client, failure, detail, above);
        closing(client, env, above, below);
        return;
    }
    resume(client, env, above, below);
    if client.state == State::Idle && client.http.waiting() == http::Waiting::Close {
        closing(client, env, above, below);
    }
}

/// At most eight routing operations. The owner schedules another turn when
/// `has_work` is true; an undemanded output stays bounded in the client.
pub fn resume(client: &mut Client, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    check_env(client, env);
    if client.state == State::Closing || client.state == State::Closed {
        return;
    }
    for _route in 0..ROUTES {
        if below.room() < 2 || above.room() < 3 {
            break;
        }
        if !client.outputs.is_empty() {
            if !client.asked {
                break;
            }
            let event = client.outputs.pop().expect("the output queue is nonempty");
            output(client, event, env, above, below);
        } else if let Some(event) = client.sse_events.pop() {
            sse_event(client, event, env, above, below);
        } else if let Some(event) = client.http_events.pop() {
            http_event(client, event, env, above, below);
        } else if let Some(request) = client.requests.pop() {
            http_down(client, env, request, below);
        } else if client.state == State::Streaming && client.asked {
            let decoder = client.decoder.as_mut().expect("a streaming client has its decoder");
            if decoder.has_ready() {
                decoder.ready(&env.limits.native(), &mut client.outputs);
            } else if client.sse.waiting() == sse::Waiting::Next {
                sse_down(client, env, sse::Request::Next);
            } else if client.sse.waiting() == sse::Waiting::Above {
                let demand = match &client.event_data {
                    EventData::Json => client.collector_below.pop().expect("a collecting event has its next read"),
                    EventData::First | EventData::Sentinel(_) => Down::Demand { read: Read::Fill(1), room: 0 },
                    EventData::Idle | EventData::SentinelEnd | EventData::Collected(_) => break,
                };
                sse_down(client, env, sse::Request::Data(demand));
            } else {
                break;
            }
        } else {
            break;
        }
        if client.state == State::Closing || client.state == State::Closed {
            break;
        }
    }
}

/// Report a caller-owned deadline or transport failure. One failure is
/// emitted before closing; already terminal/cancelled calls keep their result.
pub fn abort(
    client: &mut Client,
    env: &Env<Limits>,
    failure: Failure,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    check_env(client, env);
    let detail = match failure {
        Failure::TimedOut { .. } => Box::new([]),
        Failure::Unauthorized
        | Failure::Refused
        | Failure::Unsupported
        | Failure::Exhausted { .. }
        | Failure::RateLimited { .. }
        | Failure::Overloaded
        | Failure::Unavailable
        | Failure::ContextTooLong
        | Failure::Invalid
        | Failure::Limit { .. }
        | Failure::Protocol
        | Failure::Cancelled => bytes::copy_of(b"call aborted by its owner"),
    };
    fail(client, failure, detail, above);
    closing(client, env, above, below);
}

/// Actual lower settlement, not merely the HTTP/SSE machines saying closed.
pub fn closed(client: &mut Client, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    check_env(client, env);
    if client.state == State::Closed {
        return;
    }
    if client.outcome == Outcome::Pending {
        fail(client, Failure::Unavailable, bytes::copy_of(b"stream closed before completion"), above);
    }
    close_machines(client, env, below);
    if client.outcome == Outcome::Cancelled {
        above.push(Event::Cancelled { owner: client.owner });
        client.outcome = Outcome::Terminal;
    }
    client.state = State::Closed;
    above.push(Event::Closed);
}

fn check_env(client: &Client, env: &Env<Limits>) {
    assert!(client.limits == env.limits, "limits are fixed for the client's lifetime");
}

fn http_down(client: &mut Client, env: &Env<Limits>, request: http::Request, below: &mut Queue<Down>) {
    let child = Env { now: env.now, wall: env.wall, limits: env.limits.http };
    let before = below.len();
    http::down(&mut client.http, &child, request, &mut client.http_events, below);
    sent_evidence(client, below, before);
}

fn sent_evidence(client: &mut Client, below: &Queue<Down>, before: u32) {
    if client.evidence == Evidence::Unsent {
        for down in below.iter().skip(usize::try_from(before).expect("u32 fits usize")) {
            match down {
                Down::Send(_) => client.evidence = Evidence::Unknown,
                Down::Demand { .. } | Down::Finish => {}
            }
        }
    }
}

fn sse_down(client: &mut Client, env: &Env<Limits>, request: sse::Request) {
    let child = Env { now: env.now, wall: env.wall, limits: event_stream(&env.limits) };
    sse::down(&mut client.sse, &child, request, &mut client.sse_events, &mut client.sse_below);
    route_sse_down(client);
}

fn route_sse_down(client: &mut Client) {
    if let Some(request) = client.sse_below.pop() {
        client.requests.push(http::Request::Body(request));
    }
}

fn http_event(
    client: &mut Client,
    event: http::Event,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match event {
        http::Event::Response(response) => response_head(client, response, env, above, below),
        http::Event::Upload(Up::Room) => {
            if let Some(body) = &client.upload {
                let end = client
                    .upload_offset
                    .saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits usize"))
                    .min(client.upload_end);
                let piece =
                    body.get(client.upload_offset..end).expect("the upload offset stays within the measured body");
                client.requests.push(http::Request::Upload(Down::Send(bytes::copy_of(piece))));
                client.upload_offset = end;
                if end == client.upload_end {
                    client.upload = None;
                    client.requests.push(http::Request::Upload(Down::Finish));
                } else {
                    client
                        .requests
                        .push(http::Request::Upload(Down::Demand { read: Read::Nothing, room: env.limits.http.send }));
                }
            }
        }
        http::Event::Upload(Up::Failed(_)) => client.upload = None,
        http::Event::Upload(Up::Bytes(_) | Up::End) => {
            fail(client, Failure::Protocol, bytes::copy_of(b"invalid upload event"), above);
            closing(client, env, above, below);
        }
        http::Event::Body(event) => body_event(client, event, env, above, below),
        http::Event::Done(reuse) => {
            if client.outcome == Outcome::Pending {
                if client.state == State::ErrorBody {
                    error_end(client, env, above);
                } else {
                    fail(client, Failure::Protocol, bytes::copy_of(b"provider stream has no terminal"), above);
                }
            }
            match reuse {
                http::Reuse::Keep
                    if client.state == State::Draining && client.http.waiting() == http::Waiting::Call =>
                {
                    client.state = State::Idle;
                    above.push(Event::Reusable);
                }
                http::Reuse::Keep | http::Reuse::Close => closing(client, env, above, below),
            }
        }
        http::Event::Failed(error) => {
            let failure = http_failure(error, &env.limits);
            fail(client, failure, bytes::copy_of(http_detail(error)), above);
            closing(client, env, above, below);
        }
        http::Event::Closed => {}
    }
}

fn queued_http_failure(client: &Client) -> Option<(Failure, Box<[u8]>)> {
    for event in &client.http_events {
        match event {
            http::Event::Failed(error) => {
                return Some((http_failure(*error, &client.limits), bytes::copy_of(http_detail(*error))));
            }
            http::Event::Response(_)
            | http::Event::Upload(_)
            | http::Event::Body(_)
            | http::Event::Done(_)
            | http::Event::Closed => {}
        }
    }
    None
}

fn http_failure(error: http::Error, limits: &Limits) -> Failure {
    match error {
        http::Error::Refused(_) => Failure::Invalid,
        // A cut exchange is no answer of the provider's (llm.md, section 2.5).
        http::Error::Closed(_) | http::Error::Truncated { .. } | http::Error::Stream(_) => Failure::Unavailable,
        http::Error::Status
        | http::Error::Version
        | http::Error::Header
        | http::Error::Framing
        | http::Error::ChunkSize
        | http::Error::Chunk
        | http::Error::Trailer
        | http::Error::Upgrade => Failure::Protocol,
        http::Error::HeadTooLong => {
            Failure::Limit { which: crate::Cap::ResponseHead, bound: u64::from(limits.http.head) }
        }
        http::Error::TooManyHeaders => {
            Failure::Limit { which: crate::Cap::ResponseFields, bound: u64::from(limits.http.headers) }
        }
    }
}

/// What happened to the exchange, for the failure's detail. The limits' own
/// details replace these when the failure is reported.
const fn http_detail(error: http::Error) -> &'static [u8] {
    match error {
        http::Error::Refused(_) => b"request refused by the HTTP layer",
        http::Error::Closed(_) => b"connection closed before the request was sent",
        http::Error::Truncated { answered: false } => b"connection closed before a response",
        http::Error::Truncated { answered: true } => b"connection closed within the response",
        http::Error::Stream(_) => b"connection failed",
        http::Error::Status => b"malformed HTTP response: status line",
        http::Error::Version => b"malformed HTTP response: version",
        http::Error::Header => b"malformed HTTP response: header",
        http::Error::Framing => b"malformed HTTP response: framing",
        http::Error::ChunkSize => b"malformed HTTP response: chunk size",
        http::Error::Chunk => b"malformed HTTP response: chunk",
        http::Error::Trailer => b"malformed HTTP response: trailer",
        http::Error::Upgrade => b"malformed HTTP response: upgrade",
        http::Error::HeadTooLong => b"HTTP response head too long",
        http::Error::TooManyHeaders => b"HTTP response has too many headers",
    }
}

fn response_head(
    client: &mut Client,
    response: http::Response,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    client.evidence = Evidence::Response { status: response.status };
    client.status = response.status;
    client.request = request_id(client.provider, &response.headers);
    for header in &response.headers {
        client.rate.observe(&header.name, &header.value);
    }
    if !identity_encoding(&response.headers) {
        fail(client, Failure::Protocol, bytes::copy_of(b"unsupported content encoding"), above);
        closing(client, env, above, below);
    } else if (200..300).contains(&response.status) {
        // Both OpenAI Responses routes can omit Content-Type on valid SSE.
        // The event decoder still requires a terminal event (llm.md, 4.8).
        let allow_missing = match client.provider {
            Provider::OpenAi | Provider::OpenAiCodex => true,
            Provider::Anthropic => false,
        };
        let media = media_type(&response.headers);
        let accepted = match media {
            Ok(Some(value)) => value.eq_ignore_ascii_case(b"text/event-stream"),
            Ok(None) => allow_missing,
            Err(()) => false,
        };
        if accepted {
            client.state = State::Streaming;
        } else {
            fail(client, Failure::Protocol, stream_type_detail(media), above);
            closing(client, env, above, below);
        }
    } else {
        client.state = State::ErrorBody;
        client.requests.push(http::Request::Body(Down::Demand {
            read: Read::Arrived { max: client.error.room().saturating_add(1) },
            room: 0,
        }));
    }
}

const CODEX_REQUEST_HEADER: &[u8] = b"x-oai-request-id";
const OPENAI_REQUEST_HEADER: &[u8] = b"x-request-id";

fn request_id(provider: Provider, headers: &[Header]) -> Box<[u8]> {
    let name: &[u8] = match provider {
        Provider::OpenAi => OPENAI_REQUEST_HEADER,
        Provider::OpenAiCodex => CODEX_REQUEST_HEADER,
        Provider::Anthropic => b"request-id",
    };
    for header in headers {
        if header.name.eq_ignore_ascii_case(name) {
            return dialect::report_value(&header.value);
        }
    }
    Box::new([])
}

fn body_event(client: &mut Client, event: Up, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    match client.state {
        State::Streaming => {
            let child = Env { now: env.now, wall: env.wall, limits: event_stream(&env.limits) };
            sse::up(&mut client.sse, &child, event, &mut client.sse_events, &mut client.sse_below);
            route_sse_down(client);
        }
        State::ErrorBody => match event {
            Up::Bytes(data) => {
                let room = usize::try_from(client.error.room()).expect("u32 fits usize");
                for &byte in data.iter().take(room) {
                    client.error.push(byte).expect("error bytes are within their cap");
                }
                if data.len() > room {
                    error_cut(client, env, above);
                    closing(client, env, above, below);
                } else {
                    client.requests.push(http::Request::Body(Down::Demand {
                        read: Read::Arrived { max: client.error.room().saturating_add(1) },
                        room: 0,
                    }));
                }
            }
            Up::End => error_end(client, env, above),
            Up::Failed(_) | Up::Room => {
                fail(client, Failure::Unavailable, bytes::copy_of(b"error body failed"), above);
                closing(client, env, above, below);
            }
        },
        State::Prepared | State::Head | State::Draining | State::Idle | State::Closing | State::Closed => {}
    }
}

fn event_stream(limits: &Limits) -> sse::Limits {
    sse::Limits { line: limits.skip, event: limits.skip, field: limits.sse.field, chunk: limits.sse.chunk }
}

fn sse_event(
    client: &mut Client,
    event: sse::Event,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if client.state != State::Streaming {
        return;
    }
    match event {
        sse::Event::Opened => {
            client.event_data = EventData::First;
        }
        sse::Event::Data(event) => {
            event_data(client, event, env, above, below);
        }
        sse::Event::Dispatched(dispatch) => {
            let before = usize::try_from(client.outputs.len()).expect("the output count fits usize");
            let data = mem::replace(&mut client.event_data, EventData::Idle);
            let decoder = client.decoder.as_mut().expect("the streaming state has its decoder");
            let parsed = match data {
                EventData::Collected(document) => decoder.event(
                    &dispatch.name,
                    document,
                    &env.limits.native(),
                    env.wall,
                    client.status,
                    client.rate,
                    &mut client.outputs,
                ),
                EventData::SentinelEnd => {
                    decoder.end(&env.limits.native(), &mut client.outputs);
                    Ok(())
                }
                EventData::Idle | EventData::First | EventData::Json | EventData::Sentinel(_) => {
                    Err(openai::DecodeError::Malformed)
                }
            };
            if let Err(error) = parsed {
                let failure = match error {
                    openai::DecodeError::TooLarge { which, bound } => Failure::Limit { which, bound },
                    openai::DecodeError::Malformed | openai::DecodeError::Missing | openai::DecodeError::WrongType => {
                        Failure::Protocol
                    }
                };
                fail(client, failure, bytes::copy_of(b"malformed provider event"), above);
                closing(client, env, above, below);
            } else {
                for output in client.outputs.iter().skip(before) {
                    match output {
                        dialect::Output::Progress => {}
                        dialect::Output::Part(_)
                        | dialect::Output::TextDelta { .. }
                        | dialect::Output::ToolStart { .. }
                        | dialect::Output::ArgumentsDelta { .. }
                        | dialect::Output::ReasoningDelta { .. }
                        | dialect::Output::Completed { .. }
                        | dialect::Output::Failed { .. } => {
                            client.activity = client.activity.checked_add(1).expect("an SSE message count fits u64");
                            break;
                        }
                    }
                }
            }
        }
        sse::Event::Ended => {
            let decoder = client.decoder.as_mut().expect("the streaming state has its decoder");
            decoder.end(&env.limits.native(), &mut client.outputs);
        }
        sse::Event::Failed(error) => {
            let failure = match error {
                sse::Error::LineTooLong | sse::Error::EventTooLong => {
                    Failure::Limit { which: crate::Cap::Skip, bound: u64::from(env.limits.skip) }
                }
                sse::Error::FieldTooLong => {
                    Failure::Limit { which: crate::Cap::Field, bound: u64::from(env.limits.sse.field) }
                }
                sse::Error::Stream(_) => Failure::Unavailable,
            };
            fail(client, failure, bytes::copy_of(b"event stream failed"), above);
            closing(client, env, above, below);
        }
        sse::Event::Closed => {}
    }
}

/// Route one data delivery; the first byte chooses JSON or the fixed sentinel.
fn event_data(client: &mut Client, event: Up, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    let state = mem::replace(&mut client.event_data, EventData::Idle);
    match state {
        EventData::First => match event {
            Up::Bytes(bytes) => {
                assert_eq!(bytes.len(), 1, "the first data demand is one byte");
                if bytes.first() == Some(&b'[') {
                    client.event_data = EventData::Sentinel(1);
                } else {
                    if client.collector.waiting() == collector::Waiting::Close {
                        client.collector.restart();
                    }
                    let child = Env { now: env.now, wall: env.wall, limits: filter::collector(&env.limits) };
                    collector::down(
                        &mut client.collector,
                        &child,
                        collector::Request::Collect,
                        &mut client.collector_events,
                        &mut client.collector_below,
                    );
                    let first = client.collector_below.pop().expect("the collector reads its first byte");
                    assert_eq!(
                        first,
                        Down::Demand { read: Read::Fill(1), room: 0 },
                        "the collector starts with one byte"
                    );
                    client.event_data = EventData::Json;
                    collected_data(client, Up::Bytes(bytes), env, above, below);
                }
            }
            Up::End => {
                fail(client, Failure::Protocol, bytes::copy_of(b"empty provider event"), above);
                closing(client, env, above, below);
            }
            Up::Failed(_) => {}
            Up::Room => unreachable!("SSE data is read only"),
        },
        EventData::Json => {
            client.event_data = EventData::Json;
            match event {
                Up::Failed(_) => {}
                Up::Bytes(_) | Up::End | Up::Room => collected_data(client, event, env, above, below),
            }
        }
        EventData::Sentinel(at) => match event {
            Up::Bytes(bytes) => {
                let mut at = at;
                for &byte in &bytes {
                    if b"[DONE]".get(usize::try_from(at).expect("sentinel offset")) != Some(&byte) {
                        fail(client, Failure::Protocol, bytes::copy_of(b"invalid provider sentinel"), above);
                        closing(client, env, above, below);
                        return;
                    }
                    at = at.checked_add(1).expect("fixed sentinel length");
                }
                client.event_data = EventData::Sentinel(at);
            }
            Up::End => {
                if at == 6 {
                    client.event_data = EventData::SentinelEnd;
                } else {
                    fail(client, Failure::Protocol, bytes::copy_of(b"truncated provider sentinel"), above);
                    closing(client, env, above, below);
                }
            }
            Up::Failed(_) => {}
            Up::Room => unreachable!("SSE data is read only"),
        },
        EventData::Idle | EventData::SentinelEnd | EventData::Collected(_) => {
            unreachable!("data answers an outstanding read")
        }
    }
}

fn collected_data(
    client: &mut Client,
    event: Up,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    let child = Env { now: env.now, wall: env.wall, limits: filter::collector(&env.limits) };
    collector::up(&mut client.collector, &child, event, &mut client.collector_events, &mut client.collector_below);
    match client.collector_events.pop() {
        Some(collector::Event::Collected(document)) => client.event_data = EventData::Collected(document),
        Some(collector::Event::Failed(error)) => {
            fail(client, dialect::failure(error, &env.limits), bytes::copy_of(b"provider JSON failed"), above);
            closing(client, env, above, below);
        }
        Some(collector::Event::Closed) => unreachable!("the active collector was not closed"),
        None => {}
    }
}

fn output(
    client: &mut Client,
    event: dialect::Output,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if client.state != State::Streaming {
        return;
    }
    match event {
        dialect::Output::Part(part) => match part {
            Ok(block) => {
                let size = block_size(&block);
                let next = client.content_bytes.checked_add(size);
                match next {
                    Some(next) if next <= u64::from(env.limits.receiving) => client.content_bytes = next,
                    Some(_) | None => {
                        fail(
                            client,
                            Failure::Limit { which: crate::Cap::Answer, bound: u64::from(env.limits.receiving) },
                            bytes::copy_of(b""),
                            above,
                        );
                        closing(client, env, above, below);
                        return;
                    }
                }
                let content = client.content.as_mut().expect("a pending call holds its completion");
                if content.push(block.clone()).is_err() {
                    fail(
                        client,
                        Failure::Limit { which: crate::Cap::OutputItems, bound: u64::from(env.limits.output_items) },
                        bytes::copy_of(b""),
                        above,
                    );
                    closing(client, env, above, below);
                } else {
                    client.asked = false;
                    above.push(Event::Block { owner: client.owner, block });
                }
            }
            Err(error) => {
                let failure = match error {
                    Error::Limit { which, bound } => Failure::Limit { which, bound },
                    Error::Invalid | Error::Unsupported => Failure::Protocol,
                };
                fail(client, failure, bytes::copy_of(b"completion translation failed"), above);
                closing(client, env, above, below);
            }
        },
        dialect::Output::TextDelta { index, content_index, text } => {
            client.asked = false;
            above.push(Event::Delta { owner: client.owner, delta: Delta::Text { index, content_index, text } });
        }
        dialect::Output::ToolStart { index, id, name } => {
            client.asked = false;
            above.push(Event::Delta { owner: client.owner, delta: Delta::ToolStart { index, id, name } });
        }
        dialect::Output::ArgumentsDelta { index, delta } => {
            client.asked = false;
            above.push(Event::Delta { owner: client.owner, delta: Delta::ToolArguments { index, delta } });
        }
        dialect::Output::ReasoningDelta { index, summary_index, text } => {
            client.asked = false;
            above.push(Event::Delta { owner: client.owner, delta: Delta::Reasoning { index, summary_index, text } });
        }
        dialect::Output::Completed { stop, usage } => {
            let content = client.content.take().expect("a pending call holds its completion");
            client.outcome = Outcome::Terminal;
            client.asked = false;
            above.push(Event::Completed {
                owner: client.owner,
                completion: Completion { content: content.into_boxed(), stop, usage },
            });
            client.state = State::Draining;
            close_sse(client, env);
            clear_requests(client);
            client.requests.push(http::Request::Discard);
        }
        dialect::Output::Failed { failure, report, detail } => {
            fail_report(client, failure, report, detail, above);
            closing(client, env, above, below);
        }
        dialect::Output::Progress => {}
    }
}

fn fail(client: &mut Client, failure: Failure, detail: Box<[u8]>, above: &mut Queue<Event>) {
    fail_report(client, failure, Report::none(), detail, above);
}

fn fail_report(client: &mut Client, failure: Failure, mut report: Report, detail: Box<[u8]>, above: &mut Queue<Event>) {
    if client.outcome != Outcome::Pending {
        return;
    }
    // A decoded head can be queued when an unsolicited lower failure wins
    // over undemanded provider output. Its status is still observed evidence.
    for event in &client.http_events {
        match event {
            http::Event::Response(response) => {
                client.status = response.status;
                client.request = request_id(client.provider, &response.headers);
                client.evidence = Evidence::Response { status: response.status };
            }
            http::Event::Upload(_)
            | http::Event::Body(_)
            | http::Event::Done(_)
            | http::Event::Failed(_)
            | http::Event::Closed => {}
        }
    }
    client.outcome = Outcome::Terminal;
    client.asked = false;
    client.content = None;
    let detail = match failure {
        Failure::Limit { which, bound } => openai::limit_detail(which, bound),
        Failure::TimedOut { phase } => openai::clip_detail(timeout_detail(phase), client.limits.detail_bytes),
        Failure::Unauthorized
        | Failure::Refused
        | Failure::Unsupported
        | Failure::Exhausted { .. }
        | Failure::RateLimited { .. }
        | Failure::Overloaded
        | Failure::Unavailable
        | Failure::ContextTooLong
        | Failure::Invalid
        | Failure::Protocol
        | Failure::Cancelled => detail,
    };
    let detail = openai::clip_detail(&detail, client.limits.detail_bytes);
    report.request = mem::replace(&mut client.request, Box::new([]));
    above.push(Event::Failed { owner: client.owner, failure, evidence: client.evidence, report, detail });
}

/// The deadline's name survives owner aborts, within the configured detail cap.
fn timeout_detail(phase: crate::Phase) -> &'static [u8] {
    match phase {
        crate::Phase::Connect => b"timed out connecting the socket",
        crate::Phase::Handshake => b"timed out completing the TLS handshake",
        crate::Phase::Head => b"timed out waiting for the response head",
        crate::Phase::Idle => b"timed out waiting for a response event",
        crate::Phase::Whole => b"timed out waiting for the whole call",
    }
}

fn error_end(client: &mut Client, env: &Env<Limits>, above: &mut Queue<Event>) {
    let limits = env.limits.native();
    let decoded = match openai::Json::from_bytes(client.error.as_slice(), &error_document(&env.limits)) {
        Ok(json) => openai::decode_error(&json, &limits),
        Err(error) => Err(error),
    };
    let error = decoded.ok();
    let failure =
        translate::failure(openai::classify(client.provider, client.status, error.as_ref(), client.rate, env.wall));
    let report = match &error {
        Some(error) => dialect::error_report(client.provider, error),
        None => Report::none(),
    };
    let detail = match error {
        Some(error) => error.message,
        None => openai::clip_detail(client.error.as_slice(), env.limits.detail_bytes),
    };
    client.error.clear();
    fail_report(client, failure, report, detail, above);
}

fn error_document(limits: &Limits) -> crate::DocumentLimits {
    crate::DocumentLimits {
        bytes: limits.error_bytes,
        strings: limits.error_bytes,
        tokens: limits.error_bytes,
        depth: limits.depth,
    }
}

// An oversized body contributes bounded, unparsed detail. Status and head
// classify it; closing immediately leaves the remaining proxy page unread.
fn error_cut(client: &mut Client, env: &Env<Limits>, above: &mut Queue<Event>) {
    let marker = b"error body cut: ";
    let maximum = marker
        .len()
        .checked_add(client.error.as_slice().len())
        .expect("marker and u32-bounded body fit usize")
        .min(usize::try_from(env.limits.detail_bytes).expect("u32 fits usize"));
    let mut detail = Writer::new(maximum);
    let label = marker.get(..marker.len().min(maximum)).expect("bounded marker prefix");
    detail.put(label).expect("marker within measured detail");
    let room = maximum.checked_sub(label.len()).expect("marker fits detail");
    let prefix =
        client.error.as_slice().get(..client.error.as_slice().len().min(room)).expect("bounded unparsed body prefix");
    detail.put(prefix).expect("prefix within measured detail");
    let failure = translate::failure(openai::classify(client.provider, client.status, None, client.rate, env.wall));
    client.error.clear();
    fail(client, failure, detail.finish(), above);
}

fn clear_requests(client: &mut Client) {
    for _request in 0..client.requests.len() {
        if client.requests.pop().is_none() {
            break;
        }
    }
}

fn close_sse(client: &mut Client, env: &Env<Limits>) {
    if client.sse_closed {
        return;
    }
    for _event in 0..client.sse_events.len() {
        drop(client.sse_events.pop());
    }
    clear_requests(client);
    let child = Env { now: env.now, wall: env.wall, limits: filter::collector(&env.limits) };
    drop(client.collector_below.pop());
    drop(client.collector_events.pop());
    collector::down(
        &mut client.collector,
        &child,
        collector::Request::Close,
        &mut client.collector_events,
        &mut client.collector_below,
    );
    drop(client.collector_events.pop());
    drop(client.collector_below.pop());
    client.event_data = EventData::Idle;
    sse_down(client, env, sse::Request::Close);
    client.sse_closed = true;
}

fn close_machines(client: &mut Client, env: &Env<Limits>, below: &mut Queue<Down>) {
    if client.close_sent {
        return;
    }
    close_sse(client, env);
    clear_requests(client);
    for _event in 0..client.http_events.len() {
        drop(client.http_events.pop());
    }
    for _event in 0..client.outputs.len() {
        drop(client.outputs.pop());
    }
    client.upload = None;
    client.call = None;
    client.content = None;
    client.decoder = None;
    client.error.clear();
    http_down(client, env, http::Request::Close, below);
    for _event in 0..client.http_events.len() {
        drop(client.http_events.pop());
    }
    client.close_sent = true;
    client.asked = false;
}

fn closing(client: &mut Client, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if client.state == State::Closing || client.state == State::Closed {
        return;
    }
    close_machines(client, env, below);
    client.state = State::Closing;
    above.push(Event::Close);
}

#[must_use]
pub fn largest_read(limits: &Limits) -> u32 {
    http::largest_read(&limits.http)
}

#[must_use]
pub fn largest_room(limits: &Limits) -> u32 {
    http::largest_room(&limits.http)
}

/// Whether these limits are representable: every edge bound they size
/// is finite and every capacity they derive fits (programming-model.md, 6.3).
/// HTTP, SSE and JSON demands fit their lower streams; tagged projections,
/// error documents and tokenizer lengths have representable capacities.
#[must_use]
pub fn valid(limits: &Limits) -> bool {
    limits.error_bytes > 0
        && limits.error_bytes < u32::MAX
        && limits.http.headers >= 6
        && limits.output_items > 0
        && limits.depth > 0
        && limits.tokens > 0
        && http::valid(&limits.http)
        && limits.sse.chunk > 0
        && sse::largest_demand(&limits.sse) <= limits.http.read
        && tokenizer::largest_demand(&filter::collector(limits).tokenizer) <= limits.sse.chunk
        && limits.skip < u32::MAX
        && openai::filter::valid(limits)
        && anthropic::filter::valid(limits)
}

/// Fixed machine, table and buffer capacities, excluding the pool's request
/// and receiving reservation and the caller-owned prompt/output payloads.
/// Standalone owners add `largest_reservation` and their entry-point scratch.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if !valid(limits) {
        return None;
    }
    let mut http_fixed = limits.http;
    http_fixed.request = 0; // The admitted encoded head belongs to the reservation.
    let native = limits.native();
    let decoder = openai::response::decoder_fixed_worst_case(&native)?
        .max(anthropic::decoder_worst_case(&native)?.checked_sub(u64::from(limits.answer))?);
    u64::try_from(size_of::<Client>())
        .ok()?
        .checked_add(http::worst_case(&http_fixed)?)?
        .checked_add(sse::worst_case(&event_stream(limits))?)?
        .checked_add(decoder)?
        .checked_add(List::<Block>::worst_case(limits.output_items)?.checked_mul(3)?)?
        .checked_add(
            collector::worst_case(&filter::collector(limits), &filter::caps(limits), &openai::filter::EVENT)?.max(
                collector::worst_case(&filter::collector(limits), &filter::caps(limits), &anthropic::filter::EVENT)?,
            ),
        )?
        .checked_add(u64::from(limits.error_bytes).checked_mul(4)?)?
        .checked_add(u64::from(limits.error_bytes).checked_add(1)?)?
        .checked_add(crate::document::worst_case(&error_document(limits))?)?
        .checked_add(u64::from(REPORT_BYTES).checked_mul(3)?)
}
/// One borrowed request's fixed encoder/grammar scratch, excluding its reserved bytes.
/// Neither document density nor request counts grow it (llm.md, section 4.2).
#[must_use]
pub fn encoding_worst_case(limits: &Limits) -> Option<u64> {
    openai::encoding_worst_case(&limits.native())
}

#[expect(clippy::manual_map, reason = "the strict subset excludes closure-taking maps")]
fn measure_headers(
    endpoint: &Endpoint,
    credential: Option<&Credential>,
    affinity: Option<crate::Affinity>,
    limits: &Limits,
    coding: Encoding,
    content_length: u64,
) -> Result<u32, Error> {
    if let Some(credential) = credential
        && (credential.value.is_empty() || !header_value(&credential.value) || !header_value(&credential.account_id))
    {
        return Err(Error::Invalid);
    }
    match (endpoint.provider, credential) {
        (Provider::OpenAiCodex, Some(credential)) => {
            if credential.account_id.is_empty() || credential.scheme == Scheme::Key {
                return Err(Error::Invalid);
            }
        }
        (Provider::OpenAiCodex, None) => return Err(Error::Invalid),
        (Provider::Anthropic | Provider::OpenAi, credential) => {
            if let Some(credential) = credential
                && !credential.account_id.is_empty()
            {
                return Err(Error::Invalid);
            }
        }
    }
    let (scheme, value_bytes, account_id) = match credential {
        Some(credential) => (Some(credential.scheme), credential.value.len(), credential.account_id.as_ref()),
        None => (None, 0, b"".as_slice()),
    };
    let rendered = match affinity {
        Some(affinity) => Some(crate::affinity::rendered(affinity)),
        None => None,
    };
    let fields = provider_headers(endpoint, scheme, account_id, rendered.as_ref());
    let total = head_length(endpoint, value_bytes, account_id.len(), coding, content_length, scheme, &fields)?;
    let mut fixed_count = if credential.is_some() { 5_usize } else { 4_usize };
    if coding == Encoding::Gzip {
        fixed_count =
            fixed_count.checked_add(1).ok_or(Error::limit(crate::Cap::ResponseFields, limits.http.headers))?;
    }
    for field in &fields {
        if field.is_some() {
            fixed_count =
                fixed_count.checked_add(1).ok_or(Error::limit(crate::Cap::ResponseFields, limits.http.headers))?;
        }
    }
    if total > usize::try_from(limits.http.request).expect("u32 fits usize") {
        return Err(Error::limit(crate::Cap::RequestHead, limits.http.request));
    }
    if endpoint
        .headers
        .len()
        .checked_add(fixed_count)
        .ok_or(Error::limit(crate::Cap::ResponseFields, limits.http.headers))?
        > usize::try_from(limits.http.headers).expect("u32 fits usize")
    {
        return Err(Error::limit(crate::Cap::ResponseFields, limits.http.headers));
    }
    u32::try_from(total).or(Err(Error::limit(crate::Cap::RequestHead, limits.http.request)))
}

#[expect(clippy::manual_map, reason = "the strict subset excludes closure-taking maps")]
fn headers(
    endpoint: &Endpoint,
    credential: Option<&Credential>,
    affinity: Option<crate::Affinity>,
    limits: &Limits,
    coding: Encoding,
    content_length: u64,
) -> Result<Box<[Header]>, Error> {
    measure_headers(endpoint, credential, affinity, limits, coding, content_length)?;
    let rendered = match affinity {
        Some(affinity) => Some(crate::affinity::rendered(affinity)),
        None => None,
    };
    let (scheme, account_id) = match credential {
        Some(credential) => (Some(credential.scheme), credential.account_id.as_ref()),
        None => (None, b"".as_slice()),
    };
    let fields = provider_headers(endpoint, scheme, account_id, rendered.as_ref());
    let mut headers = List::with_capacity(limits.http.headers);
    for (name, value) in [
        (b"Host".as_slice(), endpoint.authority.as_ref()),
        (b"Content-Type".as_slice(), b"application/json".as_slice()),
        (b"Accept".as_slice(), b"text/event-stream".as_slice()),
        (b"Accept-Encoding".as_slice(), b"identity".as_slice()),
    ] {
        headers
            .push(Header { name: bytes::copy_of(name), value: bytes::copy_of(value) })
            .expect("fixed headers fit the checked cap");
    }
    if coding == Encoding::Gzip {
        headers
            .push(Header { name: bytes::copy_of(b"Content-Encoding"), value: bytes::copy_of(b"gzip") })
            .expect("the coding field fits the checked cap");
    }
    for (name, value) in fields.iter().flatten() {
        headers
            .push(Header { name: bytes::copy_of(name), value: bytes::copy_of(value) })
            .expect("provider headers fit the checked cap");
    }
    if let Some(credential) = credential {
        let field = match (endpoint.provider, credential.scheme) {
            (Provider::Anthropic, Scheme::Key) => {
                Header { name: bytes::copy_of(b"x-api-key"), value: credential.value.clone() }
            }
            (Provider::OpenAiCodex | Provider::OpenAi, Scheme::Key)
            | (Provider::OpenAiCodex | Provider::OpenAi | Provider::Anthropic, Scheme::Bearer) => {
                let len = credential
                    .value
                    .len()
                    .checked_add(7)
                    .ok_or(Error::limit(crate::Cap::RequestHead, limits.http.request))?;
                let mut bearer = Writer::new(len);
                bearer.put(b"Bearer ").expect("the bearer prefix was measured");
                bearer.put(&credential.value).expect("the credential was measured");
                Header { name: bytes::copy_of(b"Authorization"), value: bearer.finish() }
            }
        };
        headers.push(field).expect("credential field fits the checked cap");
    }
    for header in &endpoint.headers {
        headers.push(header.clone()).expect("extra headers fit the checked cap");
    }
    Ok(headers.into_boxed())
}

fn head_length(
    endpoint: &Endpoint,
    value_bytes: usize,
    account_bytes: usize,
    coding: Encoding,
    content_length: u64,
    scheme: Option<Scheme>,
    fields: &[Option<(&[u8], &[u8])>],
) -> Result<usize, Error> {
    if endpoint.authority.is_empty() || !valid_authority(&endpoint.authority) || endpoint.target.first() != Some(&b'/')
    {
        return Err(Error::Invalid);
    }
    for &byte in &endpoint.target {
        if !(0x21..=0x7e).contains(&byte) || byte == b'#' {
            return Err(Error::Invalid);
        }
    }
    let mut total = endpoint.authority.len().checked_add(endpoint.target.len()).ok_or(Error::Invalid)?;
    total = total.checked_add(value_bytes).ok_or(Error::Invalid)?;
    total = total.checked_add(account_bytes).ok_or(Error::Invalid)?;
    for (index, header) in endpoint.headers.iter().enumerate() {
        if reserved(&header.name) || !header_name(&header.name) || !header_value(&header.value) {
            return Err(Error::Invalid);
        }
        for previous in endpoint.headers.get(..index).ok_or(Error::Invalid)? {
            if previous.is(&header.name) {
                return Err(Error::Invalid);
            }
        }
        total = total.checked_add(header.name.len()).ok_or(Error::Invalid)?;
        total = total.checked_add(header.value.len()).ok_or(Error::Invalid)?;
        total = total.checked_add(4).ok_or(Error::Invalid)?;
    }
    // Exact wire length: the measured body fixes the Content-Length digits,
    // or for a compressed body its bound fixes their most.
    // Check it before allocating any request head or credential copy.
    for fixed in [
        b"POST ".as_slice(),
        b" HTTP/1.1\r\n",
        b"Host: \r\n",
        b"Content-Type: application/json\r\n",
        b"Accept: text/event-stream\r\n",
        b"Accept-Encoding: identity\r\n",
        b"Content-Length: \r\n",
        b"\r\n",
    ] {
        total = total.checked_add(fixed.len()).ok_or(Error::Invalid)?;
    }
    if coding == Encoding::Gzip {
        total = total.checked_add(b"Content-Encoding: gzip\r\n".len()).ok_or(Error::Invalid)?;
    }
    let credential_field = match (endpoint.provider, scheme) {
        (Provider::OpenAiCodex | Provider::OpenAi, Some(Scheme::Key))
        | (Provider::OpenAiCodex | Provider::OpenAi | Provider::Anthropic, Some(Scheme::Bearer)) => {
            b"Authorization: Bearer \r\n".as_slice()
        }
        (Provider::Anthropic, Some(Scheme::Key)) => b"x-api-key: \r\n".as_slice(),
        (Provider::OpenAiCodex | Provider::OpenAi | Provider::Anthropic, None) => b"".as_slice(),
    };
    total = total.checked_add(credential_field.len()).ok_or(Error::Invalid)?;
    for (name, value) in fields.iter().flatten() {
        total = total.checked_add(name.len()).ok_or(Error::Invalid)?;
        // Codex account bytes were counted with the credential above.
        if !name.eq_ignore_ascii_case(b"chatgpt-account-id") {
            total = total.checked_add(value.len()).ok_or(Error::Invalid)?;
        }
        total = total.checked_add(4).ok_or(Error::Invalid)?;
    }
    let length = Decimal::of(content_length);
    total = total.checked_add(length.as_bytes().len()).ok_or(Error::Invalid)?;
    Ok(total)
}

fn provider_headers<'a>(
    endpoint: &Endpoint,
    scheme: Option<Scheme>,
    account_id: &'a [u8],
    affinity: Option<&'a [[u8; 36]; 2]>,
) -> [Option<(&'static [u8], &'a [u8])>; 3] {
    let (session, thread) = match affinity {
        Some(rendered) => (
            Some((b"session-id".as_slice(), rendered.first().expect("fixed session UUID").as_slice())),
            Some((b"thread-id".as_slice(), rendered.get(1).expect("fixed thread UUID").as_slice())),
        ),
        None => (None, None),
    };
    match endpoint.provider {
        Provider::OpenAiCodex => [Some((b"chatgpt-account-id", account_id)), session, thread],
        Provider::OpenAi => [None, None, None],
        Provider::Anthropic => [
            default_header(endpoint, b"anthropic-version", b"2023-06-01"),
            // The bearer route is withdrawn; the second part of LLM accounts removes it.
            match scheme {
                Some(Scheme::Bearer) => default_header(endpoint, b"anthropic-beta", b"oauth-2025-04-20"),
                Some(Scheme::Key) | None => None,
            },
            None,
        ],
    }
}
fn default_header(
    endpoint: &Endpoint,
    name: &'static [u8],
    value: &'static [u8],
) -> Option<(&'static [u8], &'static [u8])> {
    for header in &endpoint.headers {
        if header.is(name) {
            return None;
        }
    }
    Some((name, value))
}

fn valid_authority(value: &[u8]) -> bool {
    for &byte in value {
        if !byte.is_ascii_alphanumeric() && !b".-:[]".contains(&byte) {
            return false;
        }
    }
    true
}

fn header_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for &byte in name {
        if !byte.is_ascii_alphanumeric() && !b"!#$%&'*+-.^_`|~".contains(&byte) {
            return false;
        }
    }
    true
}

fn header_value(value: &[u8]) -> bool {
    for &byte in value {
        if !(0x20..=0x7e).contains(&byte) && byte != b'\t' {
            return false;
        }
    }
    true
}

fn reserved(name: &[u8]) -> bool {
    for reserved in [
        b"host".as_slice(),
        b"authorization",
        b"chatgpt-account-id",
        b"session-id",
        b"thread-id",
        b"x-api-key",
        b"content-type",
        b"accept",
        b"accept-encoding",
        b"content-encoding",
        b"content-length",
        b"transfer-encoding",
        b"connection",
    ] {
        if name.eq_ignore_ascii_case(reserved) {
            return true;
        }
    }
    false
}

fn identity_encoding(headers: &[Header]) -> bool {
    let mut seen = false;
    for header in headers {
        if header.is(b"content-encoding") {
            if seen || !trim(&header.value).eq_ignore_ascii_case(b"identity") {
                return false;
            }
            seen = true;
        }
    }
    true
}

fn media_type(headers: &[Header]) -> Result<Option<&[u8]>, ()> {
    let mut found = None;
    for header in headers {
        if header.is(b"content-type") {
            if found.is_some() {
                return Err(());
            }
            let end = match bytes::find(&header.value, b";") {
                Some(end) => end,
                None => header.value.len(),
            };
            found = Some(trim(header.value.get(..end).expect("the delimiter is within the value")));
        }
    }
    Ok(found)
}

fn stream_type_detail(media: Result<Option<&[u8]>, ()>) -> Box<[u8]> {
    // Keep only a bounded media type, never its parameters or response body.
    const MEDIA_BYTES: usize = 64;
    const PREFIX: &[u8] = b"response is not an event stream (content-type: ";
    let value: &[u8] = match media {
        Ok(Some(value)) => {
            if value.len() > MEDIA_BYTES {
                b"too long"
            } else if let Some(slash) = bytes::find(value, b"/") {
                let kind = value.get(..slash).expect("the slash is within the media type");
                let subtype = value.get(slash.saturating_add(1)..).expect("the slash is within the media type");
                if header_name(kind) && header_name(subtype) { value } else { b"invalid" }
            } else {
                b"invalid"
            }
        }
        Ok(None) => b"missing",
        Err(()) => b"duplicate",
    };
    let length = PREFIX.len().checked_add(value.len()).expect("bounded media type");
    let length = length.checked_add(1).expect("bounded media detail");
    let mut out = Writer::new(length);
    out.put(PREFIX).expect("measured detail prefix");
    out.put(value).expect("measured media type");
    out.put(b")").expect("measured detail suffix");
    out.finish()
}

fn trim(mut value: &[u8]) -> &[u8] {
    for _byte in 0..value.len() {
        if value.first() == Some(&b' ') || value.first() == Some(&b'\t') {
            value = value.get(1..).expect("a nonempty slice loses one byte");
        } else {
            break;
        }
    }
    for _byte in 0..value.len() {
        if value.last() == Some(&b' ') || value.last() == Some(&b'\t') {
            value = value.get(..value.len().saturating_sub(1)).expect("a nonempty slice loses one byte");
        } else {
            break;
        }
    }
    value
}

fn block_size(block: &Block) -> u64 {
    // Replay owns exactly its retained text and records. Its completion's
    // total belongs to the receiving reservation (llm.md, section 4.2).
    let mut bytes = 0_u64;
    let replay = match block {
        Block::Text { text, replay } | Block::Refusal { text, replay } => {
            bytes = u64::try_from(text.len()).expect("a slice length fits u64");
            replay.as_ref()
        }
        Block::ToolCall { id, name, arguments, replay } => {
            for part in [id, name, arguments] {
                bytes = bytes.saturating_add(u64::try_from(part.len()).expect("a slice length fits u64"));
            }
            replay.as_ref()
        }
        Block::Oversize { id, name, .. } => {
            return u64::try_from(id.len())
                .expect("slice length fits u64")
                .saturating_add(u64::try_from(name.len()).expect("slice length fits u64"));
        }
        Block::Cut { id, name, arguments } => {
            return u64::try_from(id.len())
                .expect("slice length fits u64")
                .saturating_add(u64::try_from(name.len()).expect("slice length fits u64"))
                .saturating_add(u64::try_from(arguments.len()).expect("slice length fits u64"));
        }
        Block::ToolResult { id, text, .. } => {
            return u64::try_from(id.len())
                .expect("a slice length fits u64")
                .saturating_add(u64::try_from(text.len()).expect("a slice length fits u64"));
        }
        Block::Reasoning { replay } => Some(replay),
        Block::Dropped { .. } => return 0,
    };
    if let Some(replay) = replay {
        let document = replay.value.document();
        let records = List::<skein_json::Compact>::worst_case(document.len()).expect("u32 record count fits u64");
        bytes = bytes.saturating_add(u64::from(document.text_len())).saturating_add(records);
    }
    bytes
}

#[cfg(test)]
mod tests;
