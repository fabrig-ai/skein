//! Raw Forgejo transcripts and a bounded tokenizer driver.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "fixture lengths and seeds are bounded by the literal documents and the prescribed seed range"
)]
#![expect(clippy::disallowed_macros, reason = "the document harness formats fixture values and literal expected rows")]
#![expect(
    clippy::disallowed_methods,
    reason = "the document harness transforms finite fixture rows with ordinary Rust iterators"
)]
#![expect(
    clippy::disallowed_types,
    reason = "the document test harness retains finite fixture rows and mutation buffers in ordinary Rust collections"
)]
#![expect(
    clippy::must_use_candidate,
    reason = "fixture-returning helpers are only consumed by assertions or other fixture construction"
)]
#![expect(
    clippy::std_instead_of_alloc,
    reason = "the integration harness runs under std and retains the copied fixture imports"
)]
#![expect(clippy::std_instead_of_core, reason = "the integration harness reads captured files through std")]
use skein_forge::forgejo::{
    client,
    decode::{Malformed, Step},
    document::{Document, Row},
    limits::Limits,
    request::{Method, Page, Repo, Request},
    server,
};
use skein_json::tokenizer;
use skein_lib::{
    Env, Intake, Queue, Time, Wall,
    stream::{Down, Up},
};
use std::{collections::BTreeSet, fs};
#[derive(Debug)]
pub struct Transcript {
    pub method: Method,
    pub target: Vec<u8>,
    pub request_body: Vec<u8>,
    pub status: u16,
    pub body: Vec<u8>,
}
#[must_use]
pub fn transcript(case: &str) -> Transcript {
    transcript_in("tests/forge/conformance/transcripts", case)
}
#[must_use]
pub fn label_transcript(case: &str) -> Transcript {
    transcript_in("tests/forge/conformance/labels/transcripts", case)
}
fn transcript_in(directory: &str, case: &str) -> Transcript {
    let root = skein_scratch::paths::workspace().join(directory).join(case);
    let request = fs::read(root.join("request")).expect("request transcript");
    let response = fs::read(root.join("response")).expect("response transcript");
    let line = request.split(|b| *b == b'\r').next().expect("request line");
    let mut words = line.split(|b| *b == b' ');
    let method = match words.next().expect("method") {
        b"GET" => Method::Get,
        b"POST" => Method::Post,
        b"PUT" => Method::Put,
        b"PATCH" => Method::Patch,
        b"DELETE" => Method::Delete,
        other => panic!("method {other:?}"),
    };
    let target = words.next().expect("target").to_vec();
    let status = std::str::from_utf8(response.split(|b| *b == b' ').nth(1).expect("status"))
        .expect("status text")
        .parse()
        .expect("status number");
    Transcript { method, target, request_body: body(&request), status, body: body(&response) }
}
fn body(bytes: &[u8]) -> Vec<u8> {
    let at = bytes.windows(4).position(|w| w == b"\r\n\r\n").expect("HTTP blank line");
    bytes[at + 4..].to_vec()
}
#[must_use]
pub fn limits() -> Limits {
    Limits {
        json: tokenizer::Limits { depth: 16, string: 65_536, number: 32, chunk: 4_096, length: 4_194_304 },
        writer: skein_json::writer::Limits { depth: 16, length: 4_194_304 },
        string_bytes: 1024,
        body_bytes: 65_536,
        inner: 64,
        target_bytes: 4_096,
    }
}
#[derive(Debug)]
pub struct Decoded {
    pub rows: Vec<Row>,
    pub document: Result<Document, Malformed>,
    pub steps: usize,
}
#[must_use]
pub fn decode(request: &Request, body: &[u8]) -> Decoded {
    let limits = limits();
    let decoder = client::decoder(request, &limits).expect("JSON answer");
    decode_answer(decoder, body, &limits)
}
#[must_use]
pub fn decode_answer(mut decoder: client::Decoder, body: &[u8], limits: &Limits) -> Decoded {
    let mut rows = Vec::new();
    let mut document = None;
    let first = client::first(&decoder);
    let steps = drive(body, limits, first, |event| match client::step(&mut decoder, limits, event) {
        Step::Demand(next) => Some(next),
        Step::Row(row, next) => {
            rows.push(row);
            Some(next)
        }
        Step::Done(value) => {
            document = Some(Ok(value));
            None
        }
        Step::Malformed(error) => {
            document = Some(Err(error));
            None
        }
    });
    Decoded { rows, document: document.expect("terminal decoder"), steps }
}
#[must_use]
pub fn route(transcript: &Transcript) -> Request {
    decode_route(transcript.method, &transcript.target, &transcript.request_body, &limits())
        .expect("transcript request")
        .0
}
/// # Errors
/// Returns the route or body refusal for malformed input.
pub fn decode_route(method: Method, target: &[u8], body: &[u8], limits: &Limits) -> Result<(Request, usize), String> {
    match server::route(method, target, limits) {
        Err(error) => Err(format!("route: {error:?}")),
        Ok(server::Route::Ready(request)) => Ok((request, 0)),
        Ok(server::Route::Body(decoder)) => {
            decode_body(decoder, body, limits).map_err(|error| format!("body: {error:?}"))
        }
    }
}
#[must_use]
pub fn decode_refusal(method: Method, target: &[u8], body: &[u8], limits: &Limits) -> Malformed {
    let server::Route::Body(decoder) = server::route(method, target, limits).expect("body route") else {
        panic!("a body decoder")
    };
    decode_body(decoder, body, limits).expect_err("a malformed request body")
}
fn decode_body(mut decoder: server::BodyDecoder, body: &[u8], limits: &Limits) -> Result<(Request, usize), Malformed> {
    let mut result = None;
    let first = server::body_first(&decoder);
    let steps = drive(body, limits, first, |event| match server::body(&mut decoder, limits, event) {
        server::BodyStep::Demand(next) => Some(next),
        server::BodyStep::Decoded(request) => {
            result = Some(Ok(request));
            None
        }
        server::BodyStep::Malformed(error) => {
            result = Some(Err(error));
            None
        }
    });
    result.expect("terminal body decoder").map(|request| (request, steps))
}

pub(super) fn drive(
    body: &[u8],
    limits: &Limits,
    demand: tokenizer::Request,
    event: impl FnMut(&tokenizer::Event) -> Option<tokenizer::Request>,
) -> usize {
    drive_split(body, limits, demand, event, body.len())
}
fn drive_split(
    body: &[u8],
    limits: &Limits,
    demand: tokenizer::Request,
    event: impl FnMut(&tokenizer::Event) -> Option<tokenizer::Request>,
    split: usize,
) -> usize {
    drive_parts(body, limits, demand, event, [&body[..split], &body[split..]].into_iter())
}
fn drive_chunked(
    body: &[u8],
    limits: &Limits,
    demand: tokenizer::Request,
    event: impl FnMut(&tokenizer::Event) -> Option<tokenizer::Request>,
    chunk: usize,
) -> usize {
    drive_parts(body, limits, demand, event, body.chunks(chunk))
}
fn drive_parts<'a>(
    body: &[u8],
    limits: &Limits,
    mut demand: tokenizer::Request,
    mut event: impl FnMut(&tokenizer::Event) -> Option<tokenizer::Request>,
    mut chunks: impl Iterator<Item = &'a [u8]>,
) -> usize {
    // No decoded string can exceed the complete input, including malformed
    // mutations. Keep the configured bound while reserving only useful room.
    let mut json = limits.json;
    json.string = json.string.min(u32::try_from(body.len()).expect("body fits"));
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: json };
    let mut tokenizer = tokenizer::Tokenizer::new(&json);
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake = Intake::with_capacity(
        u32::try_from(body.len()).expect("body fits").max(tokenizer::largest_demand(&limits.json)),
    );
    let mut read = None;
    for steps in 1..=body.len() * 4 + 32 {
        match read.take() {
            Some(read) => {
                let first = intake.meet(read);
                let bytes = match first {
                    Some(bytes) => Some(bytes),
                    None => {
                        let mut met = None;
                        for bytes in chunks.by_ref() {
                            intake.append(bytes).expect("chunk fits");
                            met = intake.meet(read);
                            if met.is_some() {
                                break;
                            }
                        }
                        met
                    }
                };
                let up = match bytes {
                    Some(bytes) => Up::Bytes(bytes),
                    None => Up::End,
                };
                tokenizer::up(&mut tokenizer, &env, up, &mut above, &mut below);
            }
            None => tokenizer::down(&mut tokenizer, &env, demand, &mut above, &mut below),
        }
        match below.pop() {
            Some(Down::Demand { read: r, room: 0 }) => read = Some(r),
            None => {}
            Some(other) => panic!("unexpected demand {other:?}"),
        }
        if let Some(token) = above.pop() {
            match event(&token) {
                Some(next) => demand = next,
                None => return steps,
            }
        }
    }
    panic!("decoder exhausted its input step bound")
}
#[must_use]
pub fn specimen() -> Repo {
    Repo { owner: Box::from(*b"fixture"), name: Box::from(*b"specimen") }
}
#[must_use]
pub fn sha(n: u64) -> skein_forge::forgejo::id::ObjectId {
    skein_forge::forgejo::id::read(format!("{n:040x}").as_bytes()).expect("masked sha")
}
#[must_use]
pub fn page(page: u32, limit: u32) -> Page {
    Page { page: Some(page), limit: Some(limit) }
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    Object,
    Array,
    Number,
    String,
    Bool,
    Null,
}
#[must_use]
pub fn shape(bytes: &[u8]) -> BTreeSet<(String, Kind)> {
    super::json::shape(bytes)
}

#[must_use]
pub fn user(id: u64) -> skein_forge::forgejo::document::User {
    skein_forge::forgejo::document::User {
        id,
        login: Box::from(if id == 1 { b"fixture".as_slice() } else { b"writer" }),
        is_admin: false,
        full_name: Box::from([]),
    }
}
#[must_use]
pub fn write_cases() -> Vec<(&'static str, Request)> {
    use skein_forge::forgejo::document::IssueState;
    let mut rows = vec![
        (
            "create-issue",
            Request::CreateIssue {
                at: specimen(),
                title: Box::from(*b"fixture issue"),
                body: Box::from(*b"<!-- kit:key 01000001 -->\nissue text"),
                labels: Box::new([]),
            },
        ),
        (
            "open-pull",
            Request::OpenPull {
                at: specimen(),
                head: Box::from(*b"writer-head"),
                base: Box::from(*b"landing"),
                title: Box::from(*b"writer pull"),
                body: Box::from(*b"writer text"),
            },
        ),
        (
            "open-pull-no-changes",
            Request::OpenPull {
                at: specimen(),
                head: Box::from(*b"no-changes"),
                base: Box::from(*b"main"),
                title: Box::from(*b"empty"),
                body: Box::from(*b"text"),
            },
        ),
        (
            "post-comment",
            Request::PostComment {
                at: specimen(),
                number: 3,
                body: Box::from(*b"<!-- kit:key 01000002 -->\ncomment text"),
            },
        ),
        ("post-comment-plain", Request::PostComment { at: specimen(), number: 3, body: Box::from(*b"plain comment") }),
        (
            "edit-issue-title",
            Request::EditIssue {
                at: specimen(),
                number: 3,
                title: Some(Box::from(*b"edited title")),
                body: None,
                state: None,
            },
        ),
        (
            "close-issue",
            Request::EditIssue { at: specimen(), number: 3, title: None, body: None, state: Some(IssueState::Closed) },
        ),
        (
            "reopen-issue",
            Request::EditIssue { at: specimen(), number: 3, title: None, body: None, state: Some(IssueState::Open) },
        ),
    ];
    rows.extend(write_other_cases());
    rows
}
fn write_other_cases() -> Vec<(&'static str, Request)> {
    use skein_forge::forgejo::{
        document::{MergeStyle, StatusState},
        request::{ReviewEvent, UpdateStyle},
    };
    let mut rows = vec![
        (
            "request-reviewers",
            Request::RequestReviewers {
                at: specimen(),
                number: 1,
                reviewers: Box::from([Box::<[u8]>::from(*b"stranger")]),
            },
        ),
        (
            "remove-reviewers",
            Request::RemoveReviewers {
                at: specimen(),
                number: 1,
                reviewers: Box::from([Box::<[u8]>::from(*b"stranger")]),
            },
        ),
        ("merge-pull", Request::Merge { at: specimen(), number: 5, style: MergeStyle::Merge, head: sha(5) }),
        ("update-pull", Request::Update { at: specimen(), number: 4, style: Some(UpdateStyle::Merge) }),
        (
            "post-status",
            Request::PostStatus {
                at: specimen(),
                commit: sha(4),
                state: StatusState::Success,
                context: Box::from(*b"fixture/test"),
                description: Box::new([]),
                link: Box::new([]),
            },
        ),
        (
            "post-status-pending",
            Request::PostStatus {
                at: specimen(),
                commit: sha(4),
                state: StatusState::Pending,
                context: Box::from(*b"fixture/other"),
                description: Box::new([]),
                link: Box::new([]),
            },
        ),
        ("create-branch", Request::CreateBranch { at: specimen(), branch: Box::from(*b"recorded"), from: sha(4) }),
        ("delete-branch", Request::DeleteBranch { at: specimen(), branch: Box::from(*b"recorded") }),
    ];
    for (case, event, key) in [
        ("post-review-approved", ReviewEvent::Approved, "01000003"),
        ("post-review-request-changes", ReviewEvent::RequestChanges, "01000004"),
        ("post-review-comment", ReviewEvent::Comment, "01000005"),
    ] {
        rows.push((
            case,
            Request::PostReview {
                at: specimen(),
                number: 1,
                event,
                body: format!("<!-- kit:key {key} -->\nreview text").into_bytes().into_boxed_slice(),
            },
        ));
    }
    rows
}
/// A global bound above every committed JSON transcript's tokenizer steps.
pub const STEP_BOUND: usize = 1_000_000;
#[must_use]
pub fn decode_error(body: &[u8]) -> Decoded {
    let limits = limits();
    decode_answer(client::error_decoder(&limits), body, &limits)
}
/// JSON transcripts with their request, or none when their status is an error.
/// Unread write answers and probes outside the dialect also use the error
/// decoder as a bounded JSON shape check.
#[must_use]
pub fn json_cases() -> Vec<(String, Option<Request>)> {
    let mut cases = vec![];
    for case in transcript_cases() {
        let transcript = transcript(&case);
        if super::json::tokens(&transcript.body).is_none() {
            continue;
        }
        let probe = [
            "setup-org-route-orgs-made",
            "setup-team-edit-nameless",
            "status-list-keeps-description-and-target-url",
            "update-rebase-first-commit",
            "update-rebase-second-commit",
        ]
        .contains(&case.as_str());
        let request = if transcript.status < 300 && !probe {
            let request = decode_route(transcript.method, &transcript.target, &transcript.request_body, &limits())
                .unwrap_or_else(|error| panic!("{case}: transcript request: {error}"))
                .0;
            if client::decoder(&request, &limits()).is_some() { Some(request) } else { None }
        } else {
            None
        };
        cases.push((case, request));
    }
    cases
}
#[must_use]
pub fn transcript_cases() -> Vec<String> {
    let root = skein_scratch::paths::workspace().join("tests/forge/conformance/transcripts");
    let mut entries: Vec<_> = fs::read_dir(root)
        .expect("transcripts directory")
        .map(|entry| entry.expect("transcript entry"))
        .filter(|entry| entry.file_type().expect("entry type").is_dir())
        .collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    entries.into_iter().map(|entry| entry.file_name().into_string().expect("transcript name")).collect()
}

#[must_use]
pub fn profile_limits() -> Limits {
    super::admitted::profile_limits()
}
#[must_use]
pub fn setup_cases() -> Vec<(&'static str, Request)> {
    let bot = || Request::CreateUser {
        login: Box::from(*b"bot"),
        email: Box::from(*b"bot@example.invalid"),
        full_name: Box::from(*b"kit:bot"),
        password: skein_lib::Secret::new(Box::from(
            *b"0000000000000000000000000000000000000000000000000000000000000001",
        )),
        must_change_password: false,
    };
    let mut cases = vec![
        ("setup-user-create", bot()),
        ("setup-user-create-again", bot()),
        ("setup-user-create-by-writer", bot()),
        ("setup-user-of", Request::UserOf { login: Box::from(*b"bot") }),
        ("setup-user-missing", Request::UserOf { login: Box::from(*b"missing") }),
        ("setup-user-admin", Request::User),
        (
            "setup-user-create-person",
            Request::CreateUser {
                login: Box::from(*b"person"),
                email: Box::from(*b"person@example.invalid"),
                full_name: Box::from(*b"Person"),
                password: skein_lib::Secret::new(Box::from(
                    *b"0000000000000000000000000000000000000000000000000000000000000002",
                )),
                must_change_password: true,
            },
        ),
        ("setup-user-person-basic", Request::User),
        ("setup-user-person-token", Request::User),
        ("setup-user-person-cleared", Request::User),
    ];
    let make = |name: &[u8], scopes: &[&[u8]]| Request::CreateToken {
        login: Box::from(*b"bot"),
        name: Box::from(name),
        scopes: scopes.iter().map(|s| Box::from(*s)).collect(),
    };
    let api = || make(b"kit-api", &[b"read:user", b"write:issue", b"write:repository"]);
    cases.extend([
        ("setup-token-make", api()),
        ("setup-token-make-git", make(b"kit-git", &[b"write:repository", b"read:user"])),
        ("setup-token-make-again", api()),
        (
            "setup-token-list",
            Request::Tokens { login: Box::from(*b"bot"), page: Page { page: Some(1), limit: Some(1) } },
        ),
        (
            "setup-token-list-page-2",
            Request::Tokens { login: Box::from(*b"bot"), page: Page { page: Some(2), limit: Some(1) } },
        ),
        ("setup-token-as-the-bot", Request::User),
        ("setup-token-git-as-the-bot", Request::User),
        ("setup-token-git-alone", Request::User),
        ("setup-token-delete", Request::DeleteToken { login: Box::from(*b"bot"), id: 6 }),
        ("setup-token-deleted-use", Request::User),
        ("setup-token-delete-missing", Request::DeleteToken { login: Box::from(*b"bot"), id: 999_999 }),
        (
            "setup-token-narrow-write",
            Request::CreateIssue {
                at: specimen(),
                title: Box::from(*b"scope probe"),
                body: Box::from(*b"scope probe"),
                labels: Box::new([]),
            },
        ),
        ("setup-scope-no-admin", Request::Tokens { login: Box::from(*b"bot"), page: Page { page: None, limit: None } }),
        ("setup-scope-no-user", Request::User),
        ("setup-scope-read-user", Request::User),
    ]);
    cases.extend(setup_org_cases());
    cases.extend(setup_hook_cases());
    cases
}

/// A recorded webhook request's event header and JSON body.
#[derive(Debug)]
pub struct DeliveryRequest {
    pub event: Vec<u8>,
    pub body: Vec<u8>,
}
#[must_use]
pub fn delivery(case: &str) -> DeliveryRequest {
    let root = skein_scratch::paths::workspace().join("tests/forge/conformance/deliveries").join(case);
    let request = fs::read(root.join("request")).expect("delivery request");
    let end = request.windows(4).position(|w| w == b"\r\n\r\n").expect("delivery head");
    let event = request[..end]
        .split(|b| *b == b'\n')
        .find_map(|line| {
            line.strip_prefix(skein_forge::forgejo::delivery::EVENT_HEADER).and_then(|rest| rest.strip_prefix(b": "))
        })
        .expect("event header")
        .strip_suffix(b"\r")
        .expect("CRLF")
        .to_vec();
    DeliveryRequest { event, body: body(&request) }
}
/// # Errors
/// Returns the delivery's malformed fields or JSON.
pub fn decode_delivery(
    event: &[u8],
    body: &[u8],
    limits: &Limits,
) -> Result<skein_forge::forgejo::delivery::Delivery, Malformed> {
    decode_delivery_steps(event, body, limits).0
}
pub fn decode_delivery_steps(
    event: &[u8],
    body: &[u8],
    limits: &Limits,
) -> (Result<skein_forge::forgejo::delivery::Delivery, Malformed>, usize) {
    use skein_forge::forgejo::client::delivery::{self, Step};
    let mut decoder = delivery::decoder(event, limits).expect("recorded delivery event");
    let mut result = None;
    let steps =
        drive(body, limits, delivery::first(&decoder), |event| match delivery::step(&mut decoder, limits, event) {
            Step::Demand(next) => Some(next),
            Step::Done(value) => {
                result = Some(Ok(value));
                None
            }
            Step::Malformed(error) => {
                result = Some(Err(error));
                None
            }
        });
    (result.expect("terminal delivery"), steps)
}
#[must_use]
pub fn delivery_cases() -> Vec<(&'static str, skein_forge::forgejo::delivery::Delivery)> {
    super::cases::deliveries::delivery_cases()
}

fn setup_team_spec() -> skein_forge::forgejo::document::TeamSpec {
    use skein_forge::forgejo::document::{Access, TeamSpec, Units};
    TeamSpec {
        name: Box::from(*b"writer"),
        permission: Access::Write,
        includes_all_repositories: true,
        units: Units { code: Access::Write, issues: Access::Write, pulls: Access::Write, actions: Access::Write },
    }
}
const SETUP_TEAM: u64 = 4;
fn setup_org_cases() -> Vec<(&'static str, Request)> {
    use skein_forge::forgejo::document::Access;
    let org = || Box::<[u8]>::from(*b"ensemble");
    let make_org = || Request::CreateOrg { person: Box::from(*b"fixture"), name: org() };
    let make_team = || Request::CreateTeam { org: org(), spec: setup_team_spec() };
    let teams = |page| Request::Teams { org: org(), page };
    let members = || Request::TeamMembers { team: SETUP_TEAM, page: Page { page: None, limit: None } };
    let mut edited = setup_team_spec();
    edited.includes_all_repositories = false;
    edited.units.pulls = Access::Read;
    edited.units.actions = Access::None;
    vec![
        (
            "setup-repository-created",
            Request::CreateRepository { org: Box::from(*b"creation"), name: Box::from(*b"starter"), private: true },
        ),
        (
            "setup-repository-created-public",
            Request::CreateRepository { org: Box::from(*b"creation"), name: Box::from(*b"public"), private: false },
        ),
        ("setup-org-made", make_org()),
        ("setup-org-made-again", make_org()),
        ("setup-org-read", Request::Org { name: org() }),
        ("setup-org-missing", Request::Org { name: Box::from(*b"missing") }),
        ("setup-team-list-new-org", teams(Page { page: None, limit: None })),
        ("setup-team-made", make_team()),
        ("setup-team-made-again", make_team()),
        ("setup-team-list", teams(Page { page: None, limit: None })),
        ("setup-team-list-page-1", teams(Page { page: Some(1), limit: Some(1) })),
        ("setup-team-list-page-3", teams(Page { page: Some(3), limit: Some(1) })),
        ("setup-team-edit", Request::EditTeam { team: SETUP_TEAM, spec: edited }),
        ("setup-team-members-none", members()),
        ("setup-team-member-added", Request::AddMember { team: SETUP_TEAM, login: Box::from(*b"writer") }),
        ("setup-team-members", members()),
        ("setup-team-member-missing-user", Request::AddMember { team: SETUP_TEAM, login: Box::from(*b"missing") }),
        (
            "setup-org-issue-by-member",
            Request::CreateIssue {
                at: Repo { owner: org(), name: Box::from(*b"stage") },
                title: Box::from(*b"member writes"),
                labels: Box::new([]),
                body: Box::from(*b"fixture"),
            },
        ),
        (
            "setup-org-repo-for-a-stranger",
            Request::Repository { at: Repo { owner: org(), name: Box::from(*b"stage") } },
        ),
    ]
}
/// The hook events sent by the setup recorder, in the request's order.
#[must_use]
pub fn setup_hook_events() -> Box<[Box<[u8]>]> {
    [
        "issues",
        "issue_assign",
        "issue_label",
        "issue_milestone",
        "pull_request",
        "pull_request_assign",
        "pull_request_label",
        "pull_request_milestone",
        "pull_request_sync",
        "pull_request_review_request",
        "issue_comment",
        "pull_request_comment",
        "pull_request_review_approved",
        "pull_request_review_rejected",
        "pull_request_review_comment",
        "push",
        "create",
        "delete",
        "workflow_job_success",
        "workflow_job_failure",
        "workflow_job_cancelled",
        "workflow_job_skipped",
    ]
    .iter()
    .map(|event| Box::from(event.as_bytes()))
    .collect()
}
fn setup_hook_spec(patched: bool, other: bool, narrow: bool) -> skein_forge::forgejo::request::HookSpec {
    skein_forge::forgejo::request::HookSpec {
        url: Box::from(if other {
            b"http://127.0.0.1:3000/other".as_slice()
        } else {
            b"http://127.0.0.1:3000/hooks/1"
        }),
        secret: skein_lib::Secret::new(Box::from(if patched {
            b"0000000000000000000000000000000000000000000000000000000000000013".as_slice()
        } else {
            b"0000000000000000000000000000000000000000000000000000000000000012"
        })),
        events: if other || narrow { Box::from([Box::<[u8]>::from(*b"push")]) } else { setup_hook_events() },
        active: !narrow,
    }
}
fn setup_hook_cases() -> Vec<(&'static str, Request)> {
    let org = || Box::from(*b"ensemble");
    let list = |page| Request::OrgHooks { org: org(), page };
    let make = |other| Request::CreateOrgHook { org: org(), hook: setup_hook_spec(false, other, false) };
    let patch = |id, narrow| Request::EditOrgHook { org: org(), id, hook: setup_hook_spec(true, false, narrow) };
    vec![
        ("setup-hook-list-none", list(Page { page: None, limit: None })),
        ("setup-hook-made", make(false)),
        ("setup-hook-made-twin", make(false)),
        ("setup-hook-made-other", make(true)),
        ("setup-hook-list", list(Page { page: None, limit: None })),
        ("setup-hook-list-page-1", list(Page { page: Some(1), limit: Some(1) })),
        ("setup-hook-list-page-4", list(Page { page: Some(4), limit: Some(1) })),
        ("setup-hook-patched", patch(1, false)),
        ("setup-hook-patched-narrow", patch(1, true)),
        ("setup-hook-patch-missing", patch(999_999, false)),
        ("setup-hook-deleted", Request::DeleteOrgHook { org: org(), id: 2 }),
        ("setup-hook-delete-missing", Request::DeleteOrgHook { org: org(), id: 999_999 }),
    ]
}

/// Edit one parsed field record, retaining the other records unchanged.
#[must_use]
pub fn replace(body: &[u8], before: &[u8], after: &[u8]) -> Vec<u8> {
    super::json::replace(body, before, after)
}

/// Decode with an input boundary at any byte, counting terminal decoder outputs.
pub fn decode_split(request: &Request, body: &[u8], split: usize, limits: &Limits) -> (Decoded, usize) {
    let mut decoder = client::decoder(request, limits).expect("answer decoder");
    let mut rows = Vec::new();
    let mut document = None;
    let mut terminals = 0;
    let steps = drive_split(
        body,
        limits,
        client::first(&decoder),
        |event| match client::step(&mut decoder, limits, event) {
            Step::Demand(next) => Some(next),
            Step::Row(row, next) => {
                rows.push(row);
                Some(next)
            }
            Step::Done(value) => {
                terminals += 1;
                document = Some(Ok(value));
                None
            }
            Step::Malformed(error) => {
                terminals += 1;
                document = Some(Err(error));
                None
            }
        },
        split,
    );
    (Decoded { rows, document: document.expect("terminal decoder"), steps }, terminals)
}

pub fn decode_chunks(request: &Request, body: &[u8], chunk: usize) -> Decoded {
    let limits = limits();
    let mut decoder = client::decoder(request, &limits).expect("repository decoder");
    let mut rows = Vec::new();
    let mut document = None;
    let first = client::first(&decoder);
    let steps = drive_chunked(
        body,
        &limits,
        first,
        |event| match client::step(&mut decoder, &limits, event) {
            Step::Demand(next) => Some(next),
            Step::Row(row, next) => {
                rows.push(row);
                Some(next)
            }
            Step::Done(value) => {
                document = Some(Ok(value));
                None
            }
            Step::Malformed(error) => {
                document = Some(Err(error));
                None
            }
        },
        chunk,
    );
    Decoded { rows, document: document.expect("terminal repository decoder"), steps }
}
