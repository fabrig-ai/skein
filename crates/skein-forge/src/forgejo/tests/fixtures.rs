use crate::forgejo::{
    client,
    decode::{Malformed, Step},
    document::{Document, Row, User},
    limits::Limits,
};
use skein_json::tokenizer;
use skein_lib::{
    Env, Intake, List, Queue, Time, Wall,
    stream::{Down, Up},
};
pub fn bot() -> User {
    User { id: 1, login: Box::from(*b"fixture"), is_admin: true, full_name: Box::from(*b"kit:bot") }
}
#[expect(dead_code, reason = "the shared limits also serve integration document tests")]
mod admitted;
pub use admitted::{SMALL_BODY_BYTES, limits, small, user};
// Both preserved comment stories use thread three in the fixture/specimen repository.
pub const COMMENT_THREAD: u64 = 3;
pub fn repository() -> crate::forgejo::request::Repo {
    crate::forgejo::request::Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") }
}
pub fn comments(number: u64) -> crate::forgejo::request::Request {
    crate::forgejo::request::Request::Comments { at: repository(), number }
}
pub fn capped(body: &[u8], cap: u32, limits: &Limits) -> tokenizer::Event {
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake =
        Intake::with_capacity(u32::try_from(body.len()).unwrap().max(tokenizer::largest_demand(&limits.json)));
    intake.append(body).unwrap();
    tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Text(cap), &mut above, &mut below);
    for _ in 0..body.len().saturating_mul(4).saturating_add(32) {
        if let Some(event) = above.pop() {
            return event;
        }
        match below.pop().expect("tokenizer demand") {
            Down::Demand { read, room: 0 } => {
                let up = match intake.meet(read) {
                    Some(bytes) => Up::Bytes(bytes),
                    None => Up::End,
                };
                tokenizer::up(&mut tokenizer, &env, up, &mut above, &mut below);
            }
            other @ (Down::Demand { .. } | Down::Send(_) | Down::Finish) => panic!("unexpected demand {other:?}"),
        }
    }
    panic!("capped text exhausted the input step bound")
}
pub struct Decoded {
    pub rows: List<Row>,
    pub document: Result<Document, Malformed>,
}
pub fn decode(decoder: client::Decoder, body: &[u8], limits: &Limits) -> Decoded {
    drive(decoder, body, limits, None)
}
pub fn decode_cut(decoder: client::Decoder, body: &[u8], limits: &Limits, after_rows: u32) -> Decoded {
    drive(decoder, body, limits, Some(after_rows))
}
fn drive(mut decoder: client::Decoder, body: &[u8], limits: &Limits, cut_at: Option<u32>) -> Decoded {
    let mut pump = Pump::new(body, limits);
    let mut rows = List::with_capacity(64);
    let mut demand = client::first(&decoder);
    loop {
        let event = pump.next(demand);
        match client::step(&mut decoder, limits, &event) {
            Step::Demand(next) => demand = next,
            Step::Row(row, next) => {
                rows.push(row).expect("test rows");
                demand = if cut_at == Some(rows.len()) { client::cut(&mut decoder) } else { next };
            }
            Step::Done(value) => return Decoded { rows, document: Ok(value) },
            Step::Malformed(error) => return Decoded { rows, document: Err(error) },
        }
    }
}
pub fn deliver(header: &[u8], body: &[u8], limits: &Limits) -> Result<crate::forgejo::delivery::Delivery, Malformed> {
    use crate::forgejo::client::delivery::{self, Step};
    let mut decoder = delivery::decoder(header, limits).expect("known delivery");
    let mut pump = Pump::new(body, limits);
    let mut demand = delivery::first(&decoder);
    loop {
        let event = pump.next(demand);
        match delivery::step(&mut decoder, limits, &event) {
            Step::Demand(next) => demand = next,
            Step::Done(value) => return Ok(value),
            Step::Malformed(error) => return Err(error),
        }
    }
}
struct Pump {
    env: Env<tokenizer::Limits>,
    tokenizer: tokenizer::Tokenizer,
    above: Queue<tokenizer::Event>,
    below: Queue<Down>,
    intake: Intake,
    remaining: usize,
}
impl Pump {
    fn new(body: &[u8], limits: &Limits) -> Self {
        let mut intake = Intake::with_capacity(
            u32::try_from(body.len()).expect("test bytes").max(tokenizer::largest_demand(&limits.json)),
        );
        intake.append(body).expect("test input");
        Self {
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json },
            tokenizer: tokenizer::Tokenizer::new(&limits.json),
            above: Queue::with_capacity(1),
            below: Queue::with_capacity(1),
            intake,
            remaining: body.len().saturating_mul(4).saturating_add(32),
        }
    }
    fn next(&mut self, demand: tokenizer::Request) -> tokenizer::Event {
        tokenizer::down(&mut self.tokenizer, &self.env, demand, &mut self.above, &mut self.below);
        while self.remaining > 0 {
            self.remaining = self.remaining.saturating_sub(1);
            if let Some(event) = self.above.pop() {
                return event;
            }
            match self.below.pop().expect("tokenizer demand") {
                Down::Demand { read, room: 0 } => {
                    let up = match self.intake.meet(read) {
                        Some(bytes) => Up::Bytes(bytes),
                        None => Up::End,
                    };
                    tokenizer::up(&mut self.tokenizer, &self.env, up, &mut self.above, &mut self.below);
                }
                other @ (Down::Demand { .. } | Down::Send(_) | Down::Finish) => {
                    panic!("unexpected stream demand {other:?}")
                }
            }
        }
        panic!("decoder exhausted the input step bound")
    }
}
pub fn request(
    method: Method,
    target: &[u8],
    body: &[u8],
    limits: &Limits,
) -> Result<crate::forgejo::request::Request, Malformed> {
    use crate::forgejo::server;
    let server::Route::Body(mut decoder) = server::route(method, target, limits).unwrap() else { panic!("body route") };
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake =
        Intake::with_capacity(u32::try_from(body.len()).unwrap().max(tokenizer::largest_demand(&limits.json)));
    intake.append(body).unwrap();
    let mut demand = server::body_first(&decoder);
    let mut read = None;
    for _ in 0..body.len().saturating_mul(4).saturating_add(32) {
        match read.take() {
            Some(read) => {
                let event = match intake.meet(read) {
                    Some(bytes) => Up::Bytes(bytes),
                    None => Up::End,
                };
                tokenizer::up(&mut tokenizer, &env, event, &mut above, &mut below);
            }
            None => tokenizer::down(&mut tokenizer, &env, demand, &mut above, &mut below),
        }
        if let Some(Down::Demand { read: r, room: 0 }) = below.pop() {
            read = Some(r);
        }
        if let Some(event) = above.pop() {
            match server::body(&mut decoder, limits, &event) {
                server::BodyStep::Demand(next) => demand = next,
                server::BodyStep::Decoded(request) => return Ok(request),
                server::BodyStep::Malformed(error) => return Err(error),
            }
        }
    }
    panic!("body decoder exhausted input step bound")
}
pub fn hook_spec() -> crate::forgejo::request::HookSpec {
    crate::forgejo::request::HookSpec {
        url: Box::from(*b"http://127.0.0.1:3000/hooks/1"),
        secret: skein_lib::Secret::new(Box::from(*b"0123456789abcdef")),
        events: Box::from([Box::<[u8]>::from(*b"push")]),
        active: true,
    }
}
pub fn hook_body() -> &'static str {
    r#"{"type":"forgejo","config":{"url":"http://127.0.0.1:3000/hooks/1","content_type":"json","secret":"0123456789abcdef"},"events":["push"],"active":true}"#
}
pub fn debug(value: &impl core::fmt::Debug) -> Box<[u8]> {
    struct Text {
        bytes: [u8; 64],
        len: usize,
    }
    impl core::fmt::Write for Text {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let end = self.len.checked_add(text.len()).ok_or(core::fmt::Error)?;
            for (to, from) in self.bytes.get_mut(self.len..end).ok_or(core::fmt::Error)?.iter_mut().zip(text.as_bytes())
            {
                *to = *from;
            }
            self.len = end;
            Ok(())
        }
    }
    let mut text = Text { bytes: [0; 64], len: 0 };
    core::fmt::write(&mut text, format_args!("{value:?}")).unwrap();
    Box::from(&text.bytes[..text.len])
}

pub const REPO: crate::api::Repository = crate::api::Repository { forge: 1, repository: 1 };

pub const START_USER: u64 = 7;
pub const TOKEN: &[u8] = b"secret";
/// A canned server's positive page cap (forge.md, section 4.1).
pub const PAGE_CAP: u32 = 50;

pub fn credential() -> crate::credential::Credential {
    crate::credential::Credential::Token(skein_lib::Secret::new(Box::from(TOKEN)))
}

pub fn api_settings(cap: u32) -> Document {
    // Explicit paging uses max_response_items, not the default (forge.md, 5.1).
    const DEFAULT_PAGING_NUM: u32 = 30;
    Document::ApiSettings(crate::forgejo::document::ApiSettings {
        max_response_items: cap,
        default_paging_num: DEFAULT_PAGING_NUM,
    })
}

pub fn fields() -> crate::forgejo::plans::Fields {
    crate::forgejo::plans::Fields {
        date: None,
        retry_after: None,
        has_more: None,
        total_count: None,
        content_range: None,
        close: false,
    }
}

pub fn specimen() -> crate::forgejo::plans::tables::Repository {
    crate::forgejo::plans::tables::Repository {
        owner: Box::from(&b"fixture"[..]),
        name: Box::from(&b"specimen"[..]),
        object_format: crate::forgejo::id::ObjectFormat::Sha1,
        merge_style: crate::forgejo::document::MergeStyle::Squash,
        html_url: Box::from(&b"http://127.0.0.1:3000/fixture/specimen"[..]),
    }
}

use crate::api::Op;
use crate::forgejo::plans::{
    Body, Heard,
    plan::{Bounds, Exchange, Plan, Step as PlanStep},
    tables::Tables,
};
use crate::forgejo::request::Method;
/// The protocol fixtures' small bounds (forge.md, sections 5.1 and 11).
pub const ROWS: u32 = 8;
pub const ANSWER_BYTES: u32 = 65_536;
pub const BODY_BYTES: u32 = 1045;
pub const PAGE_SIZE: u32 = 2;
pub const RAW_BYTES: u32 = 4;
pub const WORD: &[u8] = b"kit";
pub const BOUNDS: Bounds =
    Bounds { page_size: PAGE_SIZE, rows: ROWS, answer_bytes: ANSWER_BYTES, body_bytes: BODY_BYTES };
pub const HEAD: [u8; 32] = {
    let mut id = [0; 32];
    id[0] = 1;
    id
};
pub const BASE: [u8; 32] = {
    let mut id = [0; 32];
    id[0] = 3;
    id
};
pub const ATTEMPT: crate::api::JobAttempt = crate::api::JobAttempt { head: HEAD, run: 7, job: 10, attempt: 3 };
pub const REPOSITORY: &[u8] = br#"{"id":1,"owner":{"id":2,"login":"fixture"},"name":"specimen","object_format_name":"sha1","default_branch":"main","default_merge_style":"squash","allow_merge_commits":true,"allow_squash_merge":false,"allow_rebase":false,"html_url":"http://127.0.0.1:3000/fixture/specimen"}"#;
pub const PULL: &[u8] = br#"{"number":1,"state":"open","head":{"repo_id":1,"ref":"work","sha":"0100000000000000000000000000000000000000"},"base":{"repo_id":1,"ref":"main","sha":"0300000000000000000000000000000000000000"},"merged":false,"merge_commit_sha":null,"merge_base":"0300000000000000000000000000000000000000","mergeable":true,"requested_reviewers":[]}"#;
pub const BRANCH: &[u8] = br#"{"name":"work","commit":{"id":"0100000000000000000000000000000000000000"},"effective_branch_protection_name":""}"#;
pub const EMPTY_STATUS: &[u8] = br#"{"state":"","total_count":0,"statuses":[]}"#;
pub const RUN: &[u8] = br#"{"id":7,"index_in_repo":5,"commit_sha":"0100000000000000000000000000000000000000"}"#;
pub const JOB: &[u8] = br#"{"id":10,"run_id":7,"attempt":3,"steps":[{"number":1,"name":"x","status":"failure"}]}"#;
pub const EMPTY_COMPARE: &[u8] = br#"{"total_commits":0,"commits":[],"files":[]}"#;

/// A pure plan driven by the dialect's own request encoders and answer decoders.
pub struct Run {
    plan: Plan,
    bounds: Bounds,
    limits: Limits,
    tables: Tables,
    exchanges: List<Exchange>,
    current: Option<Exchange>,
}
impl Run {
    pub fn new(op: Op) -> Self {
        Self::at_repository(REPO, limits(), BOUNDS, op)
    }
    pub fn at_repository(repository: crate::api::Repository, limits: Limits, bounds: Bounds, op: Op) -> Self {
        // Tables and recorded exchanges hold no more than the fixture's row bound.
        Self {
            plan: Plan::new(repository, op),
            bounds,
            limits,
            tables: Tables::new(ROWS, ROWS),
            exchanges: List::with_capacity(ROWS),
            current: None,
        }
    }
    pub fn known(op: Op) -> Self {
        Self::with(BOUNDS, op)
    }
    pub fn with(bounds: Bounds, op: Op) -> Self {
        let mut run = Self::new(op);
        run.bounds = bounds;
        run.tables.keep_repository(REPO, specimen());
        run
    }
    pub fn with_limits(limits: Limits, bounds: Bounds, op: Op) -> Self {
        let mut run = Self::with(bounds, op);
        run.limits = limits;
        run
    }
    fn record(&mut self, step: PlanStep) -> PlanStep {
        if let PlanStep::Send(exchange) = &step {
            self.exchanges.push(exchange.clone()).expect("fixture exchange bound");
            self.current = Some(exchange.clone());
        }
        step
    }
    pub fn begin(&mut self) -> PlanStep {
        let step = match self.plan.begin(self.bounds, WORD, &mut self.tables) {
            Ok(step) => step,
            Err(error) => PlanStep::Done(Err(error)),
        };
        self.record(step)
    }
    pub fn hear(&mut self, heard: Heard) -> Option<PlanStep> {
        let step = match self.plan.heard(heard, self.bounds, WORD, &mut self.tables) {
            Ok(step) => step,
            Err(error) => Some(PlanStep::Done(Err(error))),
        };
        Some(self.record(step?))
    }
    pub fn fail(&mut self, failure: crate::forgejo::plans::Failure) -> PlanStep {
        self.hear(Heard::Failed(failure)).expect("failure ends the exchange")
    }
    pub fn answer(&mut self, status: u16, bytes: &[u8]) -> PlanStep {
        self.answer_with(status, bytes, crate::forgejo::plans::Fields { date: Some(Wall::EPOCH), ..fields() })
    }
    pub fn answer_with(&mut self, status: u16, bytes: &[u8], fields: crate::forgejo::plans::Fields) -> PlanStep {
        if let Some(step) = self.hear(Heard::Head { status, fields }) {
            return step;
        }
        let request = &self.current.as_ref().expect("sent request").request;
        let decoder = if status >= 400 {
            Some(client::error_decoder(&self.limits))
        } else {
            client::decoder(request, &self.limits)
        };
        let body = if let Some(decoder) = decoder {
            let result = decode(decoder, bytes, &self.limits);
            for row in result.rows.into_boxed() {
                if let Some(step) = self.hear(Heard::Row(row)) {
                    return step;
                }
            }
            match result.document {
                Ok(document) => Body::Document(document),
                Err(error) => Body::Malformed(error),
            }
        } else if client::encode(request, &self.limits).unwrap().method == Method::Get {
            Body::Raw { bytes: Box::from(bytes), more: false }
        } else {
            Body::Empty
        };
        self.hear(Heard::Done(body)).expect("answer advances the plan")
    }
    pub fn tables(&mut self) -> &mut Tables {
        &mut self.tables
    }
    pub fn last_range(&self) -> Option<crate::forgejo::plans::plan::Range> {
        self.current.as_ref().expect("sent request").range
    }
    pub fn assert_listing(&self, prefix: &str, limit: &[u8]) {
        let exchange = self.current.as_ref().expect("sent request");
        let encoded = client::encode(&exchange.request, &self.limits).unwrap();
        assert_eq!(encoded.method, Method::Get);
        assert_eq!(
            encoded.target.strip_prefix(b"/api/v1/repos/fixture/specimen/").unwrap().strip_prefix(prefix.as_bytes()),
            Some(limit)
        );
    }
    fn assert_exchange(exchange: &Exchange, line: &str, limits: &Limits) {
        let encoded = client::encode(&exchange.request, limits).unwrap();
        let method = match encoded.method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        };
        let (asked_method, target) = line.split_once(' ').expect("literal request line");
        assert_eq!(method, asked_method, "{line}");
        assert_eq!(encoded.target.as_ref(), target.as_bytes(), "{line}");
    }
    pub fn assert_bodies(&self, expected: &[Option<&[u8]>]) {
        assert_eq!(usize::try_from(self.exchanges.len()).unwrap(), expected.len());
        for (exchange, body) in self.exchanges.iter().zip(expected) {
            let encoded = client::encode(&exchange.request, &self.limits).unwrap();
            assert_eq!(encoded.body.as_deref(), *body, "{:?}", exchange.request);
        }
    }
    pub fn assert_sent(&self, expected: &[&str]) {
        assert_eq!(usize::try_from(self.exchanges.len()).unwrap(), expected.len());
        for (exchange, line) in self.exchanges.iter().zip(expected) {
            Self::assert_exchange(exchange, line, &self.limits);
        }
    }
}
