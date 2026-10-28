//! Deterministic, demand-checking plaintext stream world. Wire fixtures are
//! handwritten independently of the product encoders.
use skein_http::{Header, client as http};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Env, Intake, Queue, Rng, Time, Token, Wall};
use skein_llm::{Block, Call, Credential, Endpoint, Message, Prompt, Role, client};

/// Small declared model used by ordinary protocol stories.
pub const TINY_DECL: skein_llm::Declared = skein_llm::Declared {
    window: 171,
    output: 4096,
    reasoning_item: 2048,
    tool_payload: 342,
    calls_per_response: 7,
    conversations: 1,
    tools: 174,
    history_items: 684,
};

#[must_use]
pub fn limits_for(provider: skein_llm::Provider) -> client::Limits {
    let mut limits = skein_llm::Limits::derive(&TINY_DECL).expect("tiny declaration").dialect(provider);
    // The world's transport supplies its pieces; stories may lower a bound.
    limits.http.request = 4096;
    limits.http.read = 256;
    limits.http.send = 31;
    limits.sse.chunk = 128;
    limits
}

#[must_use]
pub fn limits() -> client::Limits {
    limits_for(skein_llm::Provider::OpenAiCodex)
}

#[must_use]
pub fn call(owner: u64) -> Call {
    let mut endpoint = Endpoint::codex();
    endpoint.headers =
        Box::new([Header { name: b"originator".to_vec().into(), value: b"skein-world".to_vec().into() }]);
    Call {
        owner: Token::new(owner),
        endpoint,
        credential: Some(Credential {
            scheme: skein_llm::Scheme::Bearer,
            value: b"secret-test-token".to_vec().into(),
            account_id: b"account-test".to_vec().into(),
        }),
        prompt: Prompt {
            model: b"fixture-model".to_vec().into(),
            instructions: b"Be brief.".to_vec().into(),
            tools: Box::new([]),
            messages: Box::new([Message {
                role: Role::User,
                content: Box::new([Block::Text { text: b"Hello".to_vec().into(), replay: None }]),
            }]),
            reasoning_effort: None,
            affinity: Some(skein_llm::Affinity { key: [0x42; 16], thread: 0 }),
            choice: skein_llm::ToolChoice::Auto,
            max_output_tokens: None,
        },
    }
}

/// Public Responses call with the fixture bearer and no account id.
#[must_use]
pub fn public_call(owner: u64) -> Call {
    let mut input = call(owner);
    input.endpoint = Endpoint::openai();
    input.credential.as_mut().expect("fixture credential").account_id = Box::new([]);
    input
}

/// Anthropic Messages call with the fixture bearer and explicit output cap.
#[must_use]
pub fn anthropic_call(owner: u64) -> skein_llm::Call {
    let mut input = call(owner);
    input.endpoint = Endpoint::anthropic();
    input.credential = Some(Credential::anthropic(b"synthetic-oauth-token".to_vec().into()));
    input.prompt.affinity = None;
    input.prompt.max_output_tokens = Some(1024);
    input
}

#[must_use]
pub fn events(documents: &[&str]) -> Vec<u8> {
    documents.iter().flat_map(|document| format!("data: {document}\n\n").into_bytes()).collect()
}

#[must_use]
pub fn response(status: u16, headers: &str, body: &[u8], chunked: bool) -> Vec<u8> {
    let framing = if chunked {
        "Transfer-Encoding: chunked\r\n".to_owned()
    } else {
        format!("Content-Length: {}\r\n", body.len())
    };
    let mut bytes = format!("HTTP/1.1 {status} Test\r\n{headers}{framing}\r\n").into_bytes();
    if chunked {
        for piece in body.chunks(37) {
            bytes.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            bytes.extend_from_slice(piece);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(b"0\r\n\r\n");
    } else {
        bytes.extend_from_slice(body);
    }
    bytes
}

pub const TERMINAL: &str = r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":12,"output_tokens":3,"input_tokens_details":{"cached_tokens":4}}}}"#;
pub const TEXT_ADDED: &str =
    r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message"}}"#;
pub const TEXT_DELTA: &str =
    r#"{"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hello"}"#;
pub const TEXT_DONE: &str = r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","phase":"final_answer","content":[{"type":"output_text","text":"Hello"}]}}"#;

#[must_use]
pub fn text_response(chunked: bool) -> Vec<u8> {
    response(
        200,
        "Content-Type: text/event-stream\r\n",
        &events(&[TEXT_ADDED, TEXT_DELTA, TEXT_DONE, TERMINAL]),
        chunked,
    )
}

/// A long burst of distinct three-byte text deltas for demand-pause stories.
pub const TINY_DELTAS: usize = 256;

#[must_use]
pub fn tiny_delta_response() -> Vec<u8> {
    let mut documents = vec![TEXT_ADDED.to_owned()];
    let mut text = String::new();
    for index in 0..TINY_DELTAS {
        let delta = format!("{index:03}");
        text.push_str(&delta);
        documents.push(format!(
            r#"{{"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"{delta}"}}"#
        ));
    }
    documents.push(format!(
        r#"{{"type":"response.output_item.done","output_index":0,"item":{{"id":"msg_1","type":"message","content":[{{"type":"output_text","text":"{text}"}}]}}}}"#
    ));
    documents.push(TERMINAL.to_owned());
    let documents: Vec<_> = documents.iter().map(String::as_str).collect();
    response(200, "Content-Type: text/event-stream\r\n", &events(&documents), false)
}

/// Queues are deliberately only as large as the public entry-point bound.
#[expect(missing_debug_implementations, reason = "Client intentionally hides credential-bearing state")]
pub struct World {
    pub machine: client::Client,
    pub env: Env<client::Limits>,
    pub seen: Vec<client::Event>,
    pub sent: Vec<u8>,
    pub demand: Option<(Read, u32)>,
    pub grants: u32,
    pub source_at: usize,
    /// Read demands observed at the lower stream boundary.
    pub read_demands: usize,
    source: Vec<u8>,
    intake: Intake,
    above: Queue<client::Event>,
    below: Queue<Down>,
    rng: Rng,
    fragment: u32,
    room_delay: u32,
    ticks: u32,
    ended: bool,
    early_reads: bool,
}
impl World {
    #[must_use]
    pub fn new(call: Call, limits: client::Limits, source: Vec<u8>, seed: u64) -> Self {
        let machine = client::Client::prepare(call, &limits).expect("fixture request is admitted");
        Self::prepared(machine, limits, source, seed)
    }

    /// Adopts the caller's one freshly prepared Client and independent literal
    /// response bytes without preparing another Client or entering either side.
    /// The caller supplies the unchanged limits used at admission; the original
    /// callback owner and request bytes remain owned by this Client. Start, Next,
    /// Cancel, Close and actual lower settlement use the existing entrances.
    /// Queue and intake capacities are identical to [`Self::new`]. Price the
    /// Client once, with source bytes, sent/seen observations and world buffers
    /// separately; this constructor adds no state or memory allowance.
    /// Contract: docs/design/fake-llm.md, section 5; programming-model.md,
    /// sections 4.4, 5.2 and 6.3; testing-strategy.md, sections 2.4 and 6.
    #[must_use]
    pub fn prepared(machine: client::Client, limits: client::Limits, source: Vec<u8>, seed: u64) -> Self {
        Self {
            machine,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            seen: Vec::new(),
            sent: Vec::new(),
            demand: None,
            grants: 0,
            source_at: 0,
            read_demands: 0,
            source,
            intake: Intake::with_capacity(http::largest_read(&limits.http)),
            above: Queue::with_capacity(client::MAX_OUT.above),
            below: Queue::with_capacity(client::MAX_OUT.below),
            rng: Rng::new(seed),
            fragment: 257,
            room_delay: 0,
            ticks: 0,
            ended: false,
            early_reads: false,
        }
    }
    pub fn fragmentation(&mut self, max: u32, room_delay: u32) {
        self.fragment = max;
        self.room_delay = room_delay;
    }
    pub fn allow_early_response(&mut self) {
        self.early_reads = true;
    }
    pub fn eof(&mut self) {
        self.ended = true;
        client::up(&mut self.machine, &self.env, Up::End, &mut self.above, &mut self.below);
        self.take();
    }
    pub fn transport_failed(&mut self) {
        self.demand = None;
        self.grants = 0;
        client::up(
            &mut self.machine,
            &self.env,
            Up::Failed(skein_lib::stream::Fault::Reset),
            &mut self.above,
            &mut self.below,
        );
        self.take();
    }
    pub fn request(&mut self, request: client::Request) {
        client::down(&mut self.machine, &self.env, request, &mut self.above, &mut self.below);
        self.take();
    }
    pub fn settle(&mut self) {
        client::closed(&mut self.machine, &self.env, &mut self.above, &mut self.below);
        self.take();
    }
    pub fn abort(&mut self, failure: skein_llm::Failure) {
        client::abort(&mut self.machine, &self.env, failure, &mut self.above, &mut self.below);
        self.take();
    }
    fn take(&mut self) {
        while let Some(event) = self.above.pop() {
            self.seen.push(event);
        }
        while let Some(request) = self.below.pop() {
            match request {
                Down::Demand { read: Read::Nothing, room: 0 } => self.demand = None,
                Down::Demand { read, room } => {
                    assert!(self.demand.is_none(), "a demand is answered before its replacement");
                    assert_eq!(self.grants, 0, "a send grant is spent before another demand");
                    if read != Read::Nothing {
                        self.read_demands += 1;
                    }
                    self.demand = Some((read, room));
                }
                Down::Send(data) => {
                    assert!(self.grants > 0, "each send follows a room grant");
                    assert!(data.len() <= usize::try_from(self.grants).expect("u32 fits usize"), "send fits its grant");
                    self.grants = 0;
                    self.sent.extend_from_slice(&data);
                }
                Down::Finish => panic!("HTTP client does not finish a reusable lower stream"),
            }
        }
    }
    /// One deterministic turn; source fragments may arrive without satisfying
    /// the outstanding demand. Reads are delivered only through `Intake::meet`.
    pub fn tick(&mut self, auto_next: bool) -> bool {
        self.ticks += 1;
        if self.machine.has_work() {
            client::resume(&mut self.machine, &self.env, &mut self.above, &mut self.below);
            self.take();
            return true;
        }
        match self.machine.waiting() {
            client::Waiting::Next if auto_next => {
                self.request(client::Request::Next);
                return true;
            }
            client::Waiting::Closing | client::Waiting::Nothing | client::Waiting::Idle => return false,
            client::Waiting::Start
            | client::Waiting::Ready
            | client::Waiting::Http(_)
            | client::Waiting::Next
            | client::Waiting::Response
            | client::Waiting::Draining => {}
        }
        if self.source_at < self.source.len() && self.intake.room() > 0 {
            let n = usize::try_from(self.rng.between(1, u64::from(self.fragment)))
                .expect("u32 fits usize")
                .min(self.source.len() - self.source_at)
                .min(usize::try_from(self.intake.room()).expect("u32 fits usize"));
            self.intake.append(&self.source[self.source_at..self.source_at + n]).expect("fragment fits intake");
            self.source_at += n;
        }
        let Some((read, room)) = self.demand else {
            return false;
        };
        let answer = if room > 0 && self.ticks.is_multiple_of(self.room_delay + 1) {
            self.grants = room;
            Some(Up::Room)
        } else if room > 0 && !self.early_reads {
            None
        } else if let Some(data) = self.intake.meet(read) {
            Some(Up::Bytes(data))
        } else if read != Read::Nothing && self.source_at == self.source.len() && !self.ended {
            self.ended = true;
            Some(Up::End)
        } else {
            None
        };
        if let Some(answer) = answer {
            if !matches!(answer, Up::End) {
                self.demand = None;
            }
            client::up(&mut self.machine, &self.env, answer, &mut self.above, &mut self.below);
            self.take();
        }
        true
    }
    pub fn run(&mut self) {
        for _ in 0..100_000 {
            if !self.tick(true) {
                return;
            }
        }
        panic!("bounded wire world stalled in {:?}, demand {:?}", self.machine.waiting(), self.demand);
    }
    #[must_use]
    pub fn terminals(&self) -> usize {
        self.seen
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    client::Event::Completed { .. } | client::Event::Failed { .. } | client::Event::Cancelled { .. }
                )
            })
            .count()
    }
    pub fn assert_once(&self) {
        assert_eq!(self.terminals(), 1, "one terminal per accepted call");
        assert!(self.seen.iter().filter(|event| matches!(event, client::Event::Close)).count() <= 1, "Close is unique");
        assert!(
            self.seen.iter().filter(|event| matches!(event, client::Event::Closed)).count() <= 1,
            "Closed is unique"
        );
    }
}

/// Shared actual-client/byte-peer world; applications supply only schemas and scripts.
pub mod fake;

/// Names of request-head fields that contain the supplied fixture value.
#[must_use]
pub fn fields_holding(sent: &[u8], value: &[u8]) -> Vec<String> {
    let end = sent.windows(4).position(|part| part == b"\r\n\r\n").unwrap_or(sent.len());
    sent[..end]
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let colon = line.iter().position(|byte| *byte == b':')?;
            line[colon + 1..]
                .windows(value.len())
                .any(|part| part == value)
                .then(|| String::from_utf8(line[..colon].to_vec()).expect("header field name is ASCII"))
        })
        .collect()
}

/// The sent requests as they would read sent without a coding: each request
/// whose head says `Content-Encoding: gzip` has its body inflated by the fake
/// LLM's own inflater, that field dropped and its `Content-Length` restated.
/// Bytes after the last whole request, and identity requests, stay as sent.
#[must_use]
pub fn identity(sent: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sent.len());
    let mut rest = sent;
    while let Some(end) = rest.windows(4).position(|part| part == b"\r\n\r\n") {
        let head = std::str::from_utf8(&rest[..end]).expect("a request head is ASCII");
        let length = head
            .split("\r\n")
            .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("content-length")))
            .map(|(_, value)| value.trim().parse::<usize>().expect("decimal Content-Length"));
        let Some(length) = length.filter(|length| end + 4 + length <= rest.len()) else { break };
        let body = &rest[end + 4..end + 4 + length];
        let gzip = head.split("\r\n").any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("content-encoding") && value.trim().eq_ignore_ascii_case("gzip")
            })
        });
        if gzip {
            let inflated = inflated(body);
            for line in head.split("\r\n") {
                match line.split_once(':') {
                    Some((name, _)) if name.eq_ignore_ascii_case("content-encoding") => continue,
                    Some((name, _)) if name.eq_ignore_ascii_case("content-length") => {
                        out.extend_from_slice(format!("{name}: {}", inflated.len()).as_bytes());
                    }
                    _ => out.extend_from_slice(line.as_bytes()),
                }
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(&inflated);
        } else {
            out.extend_from_slice(&rest[..end + 4 + length]);
        }
        rest = &rest[end + 4 + length..];
    }
    out.extend_from_slice(rest);
    out
}

/// One gzip member inflated by the fake LLM's inflater, which checks its CRC
/// and length.
#[must_use]
pub fn inflated(member: &[u8]) -> Vec<u8> {
    let size = member.len().checked_sub(4).map(|at| &member[at..]).expect("a gzip trailer");
    let size = u32::from_le_bytes(size.try_into().expect("four bytes"));
    let mut out = skein_lib::List::with_capacity(size);
    skein_fake_llm_protocol::provider::inflate(member, &mut out).expect("a well-formed gzip member");
    out.as_slice().to_vec()
}

/// Build a fixture field without repeating its owned wire representation.
#[must_use]
pub fn header(name: &[u8], value: &[u8]) -> Header {
    Header { name: name.to_vec().into(), value: value.to_vec().into() }
}

/// Observe admission's refusal without exposing the credential-bearing client.
#[must_use]
pub fn rejection(input: Call, bounds: &client::Limits) -> Option<skein_llm::Error> {
    client::Client::prepare(input, bounds).err()
}

/// Drive one fixture call to its demand-driven terminal.
#[must_use]
pub fn run_call(input: Call, bounds: client::Limits, source: Vec<u8>, seed: u64) -> World {
    let mut world = World::new(input, bounds, source, seed);
    world.request(client::Request::Start);
    world.run();
    world
}

/// Borrow the credential carried by the authenticated call fixture.
#[must_use]
pub fn credential_of(input: &Call) -> &Credential {
    input.credential.as_ref().expect("fixture credential")
}

/// Checks one named tool stream against its completed call or per-call outcome.
pub fn assert_tool_stream(events: &[client::Event], index: u32, block: &skein_llm::Block) {
    let (id, name, arguments) = match block {
        skein_llm::Block::ToolCall { id, name, arguments, .. } | skein_llm::Block::Cut { id, name, arguments } => {
            (id, name, Some(arguments))
        }
        skein_llm::Block::Oversize { id, name, .. } => (id, name, None),
        skein_llm::Block::Text { .. }
        | skein_llm::Block::Refusal { .. }
        | skein_llm::Block::ToolResult { .. }
        | skein_llm::Block::Dropped { .. }
        | skein_llm::Block::Reasoning { .. } => panic!("expected a tool outcome"),
    };
    let mut starts = 0;
    let mut joined = Vec::new();
    let mut fragments = 0;
    for event in events {
        match event {
            client::Event::Delta {
                delta: skein_llm::Delta::ToolStart { index: actual, id: actual_id, name: actual_name },
                ..
            } if *actual == index => {
                starts += 1;
                assert_eq!(starts, 1, "one start per call");
                assert_eq!(actual_id, id);
                assert_eq!(actual_name, name);
            }
            client::Event::Delta { delta: skein_llm::Delta::ToolArguments { index: actual, delta }, .. }
                if *actual == index =>
            {
                assert_eq!(starts, 1, "identity precedes every fragment");
                fragments += 1;
                joined.extend_from_slice(delta);
            }
            client::Event::Delta { .. }
            | client::Event::Block { .. }
            | client::Event::Completed { .. }
            | client::Event::Failed { .. }
            | client::Event::Cancelled { .. }
            | client::Event::Reusable
            | client::Event::Close
            | client::Event::Closed => {}
        }
    }
    assert_eq!(starts, 1);
    assert!(fragments > 0, "the provider streamed arguments");
    if let Some(arguments) = arguments {
        assert_eq!(joined.as_slice(), arguments.as_ref());
    }
}
