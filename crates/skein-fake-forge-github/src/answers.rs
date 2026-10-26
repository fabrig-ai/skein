//! Bounded documents and GitHub's refusal texts.
use crate::service::{Limits, Response};
use alloc::boxed::Box;
use skein_forge::github::{document as d, server};
use skein_lib::{Overflow, Writer, bytes};

pub(crate) const SUSPENDED: &[u8] = b"This installation has been suspended";

#[derive(Debug)]
pub(crate) enum Body<'a> {
    Combined(&'a d::Combined, &'a [d::Context]),
    Runs(&'a [d::Run]),
    Job(&'a d::Job),
    ProtectionRules(&'a [d::Rule]),
    Ruleset(skein_forge::api::Bypass),
    Classic(&'a d::Protection),
    Merged(&'a skein_forge::api::Commit),
    Rules(&'a [u8]),
    Done,
    Number(u64),
    Id(u64),
    App(&'a d::App),
    AppIdentity(&'a d::App),
    BotUser(u64, &'a [u8]),
    Hook(&'a d::Hook),
    Installation(&'a d::Installation),
    Installations(&'a [d::Installation]),
    Minted(&'a d::Minted, &'a [d::Permission], crate::installations::Scope),
    Reachable(&'a [d::Reachable], u64),
    Viewer(&'a d::Viewer),
    Graphql,
    GraphqlRefusal(&'a [u8], &'a [u8]),
    Enqueued(u32),
    EnqueueRefusal(&'a [u8]),
    Repository(&'a d::Repository),
    Branch(&'a d::Branch, &'a [u8]),
    Branches(&'a [&'a [u8]]),
    Issues(&'a [d::Issue]),
    Issue(&'a d::Issue),
    Comments(&'a [d::Comment]),
    PullGraphql(&'a d::PullGraphql),
    Pull(&'a d::Pull),
    Pulls(&'a [d::Pull]),
    Files(&'a [d::File]),
    Compare(&'a d::Compare, &'a [skein_forge::api::Commit], &'a [d::File]),
    User(&'a d::User),
    Collaborators(&'a [d::Collaborator]),
    Permission(skein_forge::api::Permission),
    Reviews(&'a [d::Review]),
    Remarks(&'a [d::Remark]),
    PullValidation(&'a [u8]),
    Refusal(&'a [u8]),
    RefusalStrings(&'a [u8], &'a [&'a [u8]]),
}
fn encode(body: &Body<'_>, out: &mut Writer) -> Result<(), Overflow> {
    match body {
        Body::Combined(row, contexts) => server::combined(row, contexts, out),
        Body::Runs(rows) => server::runs(rows, out),
        Body::Job(row) => server::job(row, out),
        Body::ProtectionRules(rows) => server::rules(rows, out),
        Body::Ruleset(bypass) => server::ruleset(*bypass, out),
        Body::Classic(value) => server::classic_protection(value, out),
        Body::Merged(commit) => server::merged(commit, out),
        Body::Rules(message) => server::merge_refusal(message, out),
        Body::Done => server::done(out),
        Body::Number(number) => server::number(*number, out),
        Body::Id(id) => server::id(*id, out),
        Body::BotUser(id, slug) => server::bot_user(*id, slug, out),
        Body::App(row) => server::app(row, out),
        Body::AppIdentity(row) => server::app_identity(row, out),
        Body::Hook(row) => server::hook(row, out),
        Body::Installation(row) => server::installation(row, out),
        Body::Installations(rows) => server::installations(rows, out),
        Body::Minted(row, permissions, scope) => minted(row, permissions, *scope, out),
        Body::Reachable(rows, total) => server::reachable(rows, *total, out),
        Body::Viewer(row) => server::viewer(row, out),
        Body::GraphqlRefusal(kind, message) => server::graphql_refusal(kind, message, out),
        Body::EnqueueRefusal(message) => server::enqueue_refused(b"UNPROCESSABLE", message, out),
        Body::Enqueued(position) => server::enqueued(*position, out),
        Body::Graphql => server::graphql_refusal(b"UNPROCESSABLE", b"Unsupported query", out),
        Body::Repository(row) => {
            let mut scratch = Writer::new(out.room());
            server::repository(row, &mut scratch)?;
            let encoded = finish(scratch).ok_or(Overflow)?;
            out.put(encoded.strip_suffix(b"}").ok_or(Overflow)?)?;
            out.put(br#","squash_merge_commit_title":"COMMIT_OR_PR_TITLE"}"#)
        }
        Body::Branch(row, message) => extra_json(Detail::Branch(row, message), out),
        Body::Branches(rows) => server::branches(rows, out),
        Body::Issues(rows) => server::issues(rows, out),
        Body::Issue(row) => server::issue(row, out),
        Body::Comments(rows) => server::comments(rows, out),
        Body::PullGraphql(row) => server::pull_graphql(row, out),
        Body::Pull(row) => server::pull(row, out),
        Body::Pulls(rows) => server::pulls(rows, out),
        Body::Files(rows) => server::file_rows(rows, out),
        Body::Compare(row, commits, files) => server::compare(row, commits, files, out),
        Body::User(row) => server::user(row, out),
        Body::Collaborators(rows) => server::collaborators(rows, out),
        Body::Permission(value) => server::permission(*value, out),
        Body::Reviews(rows) => server::reviews(rows, out),
        Body::Remarks(rows) => server::remarks(rows, out),
        Body::PullValidation(message) => extra_json(Detail::PullValidation(message), out),
        Body::Refusal(message) => server::refusal(message, &[], out),
        Body::RefusalStrings(message, errors) => server::refusal_strings(message, errors, out),
    }
}
enum Detail<'a> {
    Branch(&'a d::Branch, &'a [u8]),
    PullValidation(&'a [u8]),
}
fn extra_json(detail: Detail<'_>, out: &mut Writer) -> Result<(), Overflow> {
    use skein_json::writer::{Encoder, Limits};
    let Ok(length) = u32::try_from(out.room()) else { return Err(Overflow) };
    let limits = Limits { depth: 4, length };
    let mut measure = Encoder::measure(&limits);
    extra_fields(&detail, &mut measure)?;
    let Ok(length) = measure.measured() else { return Err(Overflow) };
    let mut json = Encoder::write(length, &limits);
    extra_fields(&detail, &mut json)?;
    out.put(&json.finish())
}
fn extra_fields(detail: &Detail<'_>, json: &mut skein_json::writer::Encoder) -> Result<(), Overflow> {
    json.object_start();
    match detail {
        Detail::Branch(row, message) => {
            json.key(b"name");
            json.string(&row.name);
            json.key(b"commit");
            json.object_start();
            json.key(b"sha");
            json.string(&skein_forge::github::id::write_commit(&row.commit).ok_or(Overflow)?);
            json.key(b"commit");
            json.object_start();
            json.key(b"message");
            json.string(message);
            json.object_end();
            json.object_end();
        }
        Detail::PullValidation(message) => {
            json.key(b"message");
            json.string(b"Validation Failed");
            json.key(b"errors");
            json.array_start();
            json.object_start();
            json.key(b"resource");
            json.string(b"PullRequest");
            json.key(b"code");
            json.string(b"custom");
            json.key(b"message");
            json.string(message);
            json.object_end();
            json.array_end();
        }
    }
    json.object_end();
    Ok(())
}
fn minted(
    row: &d::Minted,
    permissions: &[d::Permission],
    scope: crate::installations::Scope,
    out: &mut Writer,
) -> Result<(), Overflow> {
    use skein_json::writer::{Encoder, Limits};
    let Ok(length) = u32::try_from(out.room()) else { return Err(Overflow) };
    let limits = Limits { depth: 2, length };
    let mut measure = Encoder::measure(&limits);
    minted_json(row, permissions, scope, &mut measure);
    let Ok(length) = measure.measured() else { return Err(Overflow) };
    let mut json = Encoder::write(length, &limits);
    minted_json(row, permissions, scope, &mut json);
    out.put(&json.finish())
}
fn minted_json(
    row: &d::Minted,
    permissions: &[d::Permission],
    scope: crate::installations::Scope,
    json: &mut skein_json::writer::Encoder,
) {
    json.object_start();
    json.key(b"token");
    json.string(row.token.expose());
    json.key(b"expires_at");
    json.string(&skein_forge::forgejo::time::write(row.expires));
    json.key(b"permissions");
    json.object_start();
    for permission in permissions {
        let include = match scope {
            crate::installations::Scope::Full => true,
            crate::installations::Scope::Contents => permission.name.as_ref() == b"contents",
        };
        if include {
            json.key(&permission.name);
            json.string(permission.access.text());
        }
    }
    if scope == crate::installations::Scope::Contents {
        json.key(b"metadata");
        json.string(b"read");
    }
    json.object_end();
    json.object_end();
}
// Writer is exact-sized. Pad the scratch writer, then retain only the encoded
// prefix; both allocations are included in worst_case's response scratch.
pub(crate) fn finish(mut out: Writer) -> Option<Box<[u8]>> {
    let length = out.written();
    let padding = [0; 128];
    for _chunk in 0..out.room().div_euclid(padding.len()) {
        out.put(&padding).ok()?;
    }
    let tail = out.room();
    out.put(padding.get(..tail)?).ok()?;
    let bytes = out.finish();
    Some(bytes::copy_of(bytes.get(..length)?))
}
pub(crate) fn response(status: u16, body: &Body<'_>, limits: &Limits) -> Response {
    let Ok(capacity) = usize::try_from(limits.response) else {
        return failed();
    };
    let mut out = Writer::new(capacity);
    if encode(body, &mut out).is_err() {
        return failed();
    }
    match finish(out) {
        Some(body) => Response {
            rate: None,
            etag: None,
            content_type: crate::service::ContentType::Json,
            status,
            link: None,
            location: None,
            body,
        },
        None => failed(),
    }
}
pub(crate) fn refusal(status: u16, message: &[u8], limits: &Limits) -> Response {
    response(status, &Body::Refusal(message), limits)
}
pub(crate) fn missing(limits: &Limits) -> Response {
    refusal(404, b"Not Found", limits)
}
pub(crate) fn unauthorized(limits: &Limits) -> Response {
    refusal(401, b"Bad credentials", limits)
}
pub(crate) fn failed() -> Response {
    // new requires room for this terminal even when another encoder refuses.
    Response {
        rate: None,
        etag: None,
        content_type: crate::service::ContentType::Json,
        status: 500,
        link: None,
        location: None,
        body: bytes::copy_of(br#"{"message":"Server Error","errors":[]}"#),
    }
}

#[derive(Debug)]
pub(crate) struct Page {
    start: u64,
    end: u64,
    next: Option<u32>,
    last: u32,
}
impl Page {
    pub(crate) fn new(page: u32, size: u32, total: u64) -> Option<Self> {
        let start = u64::from(page.checked_sub(1)?).checked_mul(u64::from(size))?;
        let end = start.checked_add(u64::from(size))?;
        let last =
            u32::try_from(total.checked_add(u64::from(size.checked_sub(1)?))?.checked_div(u64::from(size))?).ok()?;
        let next = if end < total { Some(page.checked_add(1)?) } else { None };
        Some(Self { start, end, next, last })
    }
    pub(crate) fn contains(&self, index: u64) -> bool {
        index >= self.start && index < self.end
    }
    pub(crate) fn link_next(&self, response: Response, target: &[u8], limits: &Limits) -> Response {
        let mut response = self.link(response, target, limits);
        if let Some(link) = &response.link
            && let Some(end) = bytes::find(link, b", ")
        {
            response.link = Some(bytes::copy_of(link.get(..end).expect("link delimiter")));
        }
        response
    }
    pub(crate) fn link(&self, mut response: Response, target: &[u8], limits: &Limits) -> Response {
        if response.status != 200 {
            return response;
        }
        if let Some(next) = self.next {
            let Ok(capacity) = usize::try_from(limits.response) else {
                return failed();
            };
            let mut out = Writer::new(capacity);
            if server::link(target, next, self.last, &mut out).is_err() {
                return failed();
            }
            let Some(link) = finish(out) else {
                return failed();
            };
            response.link = Some(link);
        }
        response
    }
}

/// An empty response with no media bytes or provider headers.
pub(crate) fn empty(status: u16) -> Response {
    Response {
        location: None,
        rate: None,
        etag: None,
        status,
        content_type: crate::service::ContentType::Json,
        link: None,
        body: Box::new([]),
    }
}
