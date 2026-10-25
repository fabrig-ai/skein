//! The fake's GitHub routes and answer documents (forge-github.md, section 2).
use super::{
    document, id,
    limits::{DEPTH, Limits},
    request::{self, Method, Request},
};
use crate::forgejo::{id::unsigned, time};
use alloc::boxed::Box;
use skein_json::writer::{self, Encoder};
use skein_lib::{Decimal, List, Overflow, Writer};
mod app;
mod queries;
pub use queries::{Graphql, graphql_body};
mod issues;
pub use issues::{comments, issue, issues};
mod delivery;
mod log;
pub use log::{deliveries, logged};
mod protection;
mod pulls;
pub use protection::{classic_protection, rules, ruleset};
mod writes;
pub use app::{
    app, app_identity, bot_user, conversion, graphql, graphql_refusal, hook, hook_url, installation, installations,
    mint_scope, minted, reachable, viewer,
};
pub use delivery::{Sent, encode};
pub use pulls::{combined, enqueue_refused, enqueued, job, merge_refusal, merged, pull, pull_graphql, pulls, runs};
pub use writes::{done, id, number, write_body};
/// Reads a request target into the request whose canonical spelling it carries.
#[must_use]
pub fn route(method: Method, target: &[u8], limits: &Limits) -> Option<Request> {
    if target.len() > usize::try_from(limits.target_bytes).expect("u32 fits usize") || target.contains(&b'#') {
        return None;
    }
    let (path, query) = split(target, b'?');
    let query = Query::read(query)?;
    let request = if method == Method::Get && path == b"/app/hook/deliveries" {
        let ask = match query.get(b"cursor") {
            Some(text) => super::log::Ask::Next { cursor: super::log::Cursor::new(text)? },
            None => super::log::Ask::First { validator: None },
        };
        Request::LogPage { ask, per_page: query.size()? }
    } else if method == Method::Get && path.starts_with(b"/app/hook/deliveries/") {
        // The listed event is decoder context, absent from this wire target.
        Request::LogDelivery { id: unsigned(path.strip_prefix(b"/app/hook/deliveries/")?)?, event: Box::new([]) }
    } else if let Some(request) = app::route(method, path, query.page(), query.size()) {
        request
    } else if let Some(request) = writes::route(method, path) {
        request
    } else if method != Method::Get {
        return None;
    } else if let Some(number) = path.strip_prefix(b"/repositories/") {
        Request::Repository { id: unsigned(number)? }
    } else if let Some(number) = path.strip_prefix(b"/user/") {
        Request::User { id: unsigned(number)? }
    } else {
        let path = path.strip_prefix(b"/repos/")?;
        let (owner, tail) = split(path, b'/');
        let (name, tail) = split(tail, b'/');
        let owner = request::unescape(owner)?;
        let name = request::unescape(name)?;
        repo_route(owner, name, tail, &query)?
    };
    // The fake accepts the dialect's exact query vocabulary, including fixed parameters.
    let mut encoded = request::encode(&request, limits).ok()?;
    // A raw epoch query is observable even though the dialect omits since at zero.
    if let Request::Issues { since, .. } = &request
        && since.as_nanos() == 0
        && query.get(b"since").is_some()
    {
        let at = skein_lib::bytes::find(&encoded.target, b"&sort=")?;
        let mut out = Writer::new(encoded.target.len().checked_add(b"&since=1970-01-01T00%3A00%3A00Z".len())?);
        out.put(encoded.target.get(..at)?).ok()?;
        out.put(b"&since=1970-01-01T00%3A00%3A00Z").ok()?;
        out.put(encoded.target.get(at..)?).ok()?;
        encoded.target = out.finish();
    }
    if encoded.target.as_ref() != target || encoded.method != method {
        return None;
    }
    Some(request)
}
fn repo_route(owner: Box<[u8]>, name: Box<[u8]>, tail: &[u8], query: &Query) -> Option<Request> {
    if tail == b"actions/permissions" {
        return Some(Request::ActionsPermissions { owner, name });
    }
    if tail == b"actions/workflows" {
        return Some(Request::Workflows { owner, name });
    }
    if tail.is_empty() {
        return Some(Request::Locate { owner, name });
    }
    if tail == b"issues" || tail.starts_with(b"issues/") || tail == b"pulls" || tail.starts_with(b"pulls/") {
        return item_route(owner, name, tail, query);
    }
    if let Some(tail) = tail.strip_prefix(b"commits/") {
        let (commit, suffix) = split(tail, b'/');
        let commit = id::commit(commit)?;
        return match suffix {
            b"status" => {
                Some(Request::CombinedStatus { owner, name, commit, page: query.page()?, per_page: query.size()? })
            }
            b"check-runs" => {
                Some(Request::CheckRuns { owner, name, commit, page: query.page()?, per_page: query.size()? })
            }
            _ => None,
        };
    }
    if let Some(tail) = tail.strip_prefix(b"actions/jobs/") {
        return match tail.strip_suffix(b"/logs") {
            Some(id) => Some(Request::JobLog { owner, name, id: unsigned(id)? }),
            None => Some(Request::Job { owner, name, id: unsigned(tail)? }),
        };
    }
    if tail == b"branches" {
        return Some(Request::Branches { owner, name, page: query.page()?, per_page: query.size()? });
    }
    if let Some(branch) = tail.strip_prefix(b"branches/") {
        return match branch.strip_suffix(b"/protection") {
            Some(branch) => Some(Request::ClassicProtection { owner, name, branch: request::unescape(branch)? }),
            None => Some(Request::Branch { owner, name, branch: request::unescape(branch)? }),
        };
    }
    if let Some(tail) = tail.strip_prefix(b"compare/") {
        let at = skein_lib::bytes::find(tail, b"...")?;
        return Some(Request::Compare {
            owner,
            name,
            before: id::commit(tail.get(..at)?)?,
            after: id::commit(tail.get(at.checked_add(3)?..)?)?,
            per_page: query.size()?,
        });
    }
    if let Some(path) = tail.strip_prefix(b"contents/") {
        return Some(Request::Raw {
            owner,
            name,
            path: request::unescape(path)?,
            commit: id::commit(query.get(b"ref")?)?,
        });
    }
    if let Some(branch) = tail.strip_prefix(b"rules/branches/") {
        return Some(Request::RulesForBranch { owner, name, branch: request::unescape(branch)?, page: query.page()? });
    }
    if let Some(number) = tail.strip_prefix(b"rulesets/") {
        return Some(Request::Ruleset { owner, name, id: unsigned(number)? });
    }
    if tail == b"collaborators" {
        return Some(Request::Collaborators { owner, name, page: query.page()?, per_page: query.size()? });
    }
    if let Some(login) = tail.strip_prefix(b"collaborators/") {
        return Some(Request::Permission {
            owner,
            name,
            login: request::unescape(login.strip_suffix(b"/permission")?)?,
        });
    }
    None
}
fn item_route(owner: Box<[u8]>, name: Box<[u8]>, tail: &[u8], query: &Query) -> Option<Request> {
    if tail == b"issues" {
        return Some(Request::Issues {
            owner,
            name,
            since: match query.get(b"since") {
                Some(value) => time::read(value)?,
                None => skein_lib::Wall::from_nanos(0),
            },
            page: query.page()?,
            per_page: query.size()?,
        });
    }
    if let Some(tail) = tail.strip_prefix(b"issues/") {
        let (number, suffix) = split(tail, b'/');
        let number = unsigned(number)?;
        return match suffix {
            b"" => Some(Request::Issue { owner, name, number }),
            b"comments" => {
                Some(Request::Comments { owner, name, number, page: query.page()?, per_page: query.size()? })
            }
            _ => None,
        };
    }
    if tail == b"pulls" {
        let head = query.get(b"head")?;
        let prefix = head.get(..owner.len())?;
        if prefix != owner.as_ref() || head.get(owner.len()) != Some(&b':') {
            return None;
        }
        return Some(Request::PullsFor {
            head: skein_lib::bytes::copy_of(head.get(owner.len().checked_add(1)?..)?),
            base: skein_lib::bytes::copy_of(query.get(b"base")?),
            open: query.get(b"state")? == b"open",
            owner,
            name,
        });
    }
    if let Some(tail) = tail.strip_prefix(b"pulls/") {
        let (number, suffix) = split(tail, b'/');
        let number = unsigned(number)?;
        return match suffix {
            b"" => Some(Request::Pull { owner, name, number }),
            b"files" => Some(Request::PullFiles { owner, name, number, page: query.page()?, per_page: query.size()? }),
            b"reviews" => Some(Request::Reviews { owner, name, number, page: query.page()?, per_page: query.size()? }),
            _ => {
                let tail = suffix.strip_prefix(b"reviews/")?;
                let review = unsigned(tail.strip_suffix(b"/comments")?)?;
                Some(Request::ReviewComments {
                    owner,
                    name,
                    number,
                    review,
                    page: query.page()?,
                    per_page: query.size()?,
                })
            }
        };
    }
    None
}

fn split(bytes: &[u8], delimiter: u8) -> (&[u8], &[u8]) {
    for (at, byte) in bytes.iter().enumerate() {
        if *byte == delimiter {
            return (bytes.get(..at).expect("position"), bytes.get(at.saturating_add(1)..).expect("position"));
        }
    }
    (bytes, b"")
}
#[derive(Debug)]
struct Query(List<Pair>);
#[derive(Debug)]
struct Pair {
    key: Box<[u8]>,
    value: Box<[u8]>,
}
impl Query {
    fn read(bytes: &[u8]) -> Option<Self> {
        let mut pairs = List::with_capacity(8);
        let mut rest = bytes;
        for _ in 0..bytes.len() {
            if rest.is_empty() {
                break;
            }
            let (pair, tail) = split(rest, b'&');
            rest = tail;
            let (key, value) = split(pair, b'=');
            pairs.push(Pair { key: request::unescape(key)?, value: request::unescape(value)? }).ok()?;
        }
        Some(Self(pairs))
    }
    fn get(&self, name: &[u8]) -> Option<&[u8]> {
        for pair in &self.0 {
            if pair.key.as_ref() == name {
                return Some(&pair.value);
            }
        }
        None
    }
    fn page(&self) -> Option<u32> {
        u32::try_from(unsigned(self.get(b"page")?)?).ok()
    }
    fn size(&self) -> Option<u32> {
        u32::try_from(unsigned(self.get(b"per_page")?)?).ok()
    }
}
/// Writes the repository fields the GitHub decoder reads.
pub fn repository(row: &document::Repository, out: &mut Writer) -> Result<(), Overflow> {
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    repository_json(row, &mut measure);
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    repository_json(row, &mut json);
    out.put(&json.finish())
}
fn repository_json(row: &document::Repository, json: &mut Encoder) {
    json.object_start();
    json.key(b"id");
    json.unsigned(row.id);
    json.key(b"owner");
    json.object_start();
    json.key(b"login");
    json.string(&row.owner);
    json.key(b"id");
    json.unsigned(row.owner_id);
    json.key(b"type");
    json.string(if row.organization { b"Organization" } else { b"User" });
    json.object_end();
    json.key(b"name");
    json.string(&row.name);
    json.key(b"default_branch");
    json.string(&row.default_branch);
    json.key(b"html_url");
    json.string(&row.html_url);
    json.key(b"allow_merge_commit");
    json.boolean(row.merge);
    json.key(b"allow_squash_merge");
    json.boolean(row.squash);
    json.key(b"allow_rebase_merge");
    json.boolean(row.rebase);
    json.object_end();
}
/// Writes a branch's name and SHA-1 commit.
pub fn branch(row: &document::Branch, out: &mut Writer) -> Result<(), Overflow> {
    let commit = id::write_commit(&row.commit).ok_or(Overflow)?;
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    branch_json(&row.name, &commit, &mut measure);
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    branch_json(&row.name, &commit, &mut json);
    out.put(&json.finish())
}
fn branch_json(name: &[u8], commit: &[u8], json: &mut Encoder) {
    json.object_start();
    json.key(b"name");
    json.string(name);
    json.key(b"commit");
    json.object_start();
    json.key(b"sha");
    json.string(commit);
    json.object_end();
    json.object_end();
}
/// Writes the names in one branch page.
pub fn branches(names: &[&[u8]], out: &mut Writer) -> Result<(), Overflow> {
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    branches_json(names, &mut measure);
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    branches_json(names, &mut json);
    out.put(&json.finish())
}
fn branches_json(names: &[&[u8]], json: &mut Encoder) {
    json.array_start();
    for name in names {
        json.object_start();
        json.key(b"name");
        json.string(name);
        json.object_end();
    }
    json.array_end();
}
/// Writes the whole refusal message and each errors[].message.
pub fn refusal(message: &[u8], errors: &[&[u8]], out: &mut Writer) -> Result<(), Overflow> {
    encode_refusal(message, errors, ErrorForm::Objects, out)
}
/// Writes the whole refusal message and each errors[] string.
pub fn refusal_strings(message: &[u8], errors: &[&[u8]], out: &mut Writer) -> Result<(), Overflow> {
    encode_refusal(message, errors, ErrorForm::Strings, out)
}
#[derive(Clone, Copy, Debug)]
enum ErrorForm {
    Objects,
    Strings,
}
fn encode_refusal(message: &[u8], errors: &[&[u8]], form: ErrorForm, out: &mut Writer) -> Result<(), Overflow> {
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    refusal_json(message, errors, form, &mut measure);
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    refusal_json(message, errors, form, &mut json);
    out.put(&json.finish())
}
fn refusal_json(message: &[u8], errors: &[&[u8]], form: ErrorForm, json: &mut Encoder) {
    json.object_start();
    json.key(b"message");
    json.string(message);
    json.key(b"errors");
    json.array_start();
    for error in errors {
        match form {
            ErrorForm::Objects => {
                json.object_start();
                json.key(b"message");
                json.string(error);
                json.object_end();
            }
            ErrorForm::Strings => json.string(error),
        }
    }
    json.array_end();
    json.object_end();
}
fn output_limits(out: &Writer) -> Result<writer::Limits, Overflow> {
    let Ok(length) = u32::try_from(out.room()) else {
        return Err(Overflow);
    };
    Ok(writer::Limits { depth: DEPTH, length })
}
/// Writes GitHub's absolute next and last links for a page.
pub fn link(target: &[u8], next_page: u32, last_page: u32, out: &mut Writer) -> Result<(), Overflow> {
    if target.len() > out.room() {
        return Err(Overflow);
    }
    let next = page_target(target, next_page)?;
    let last = page_target(target, last_page)?;
    let length = next
        .len()
        .saturating_add(last.len())
        .saturating_add(2usize.saturating_mul(b"<https://api.github.com".len()))
        .saturating_add(b">; rel=\"next\", ".len())
        .saturating_add(b">; rel=\"last\"".len());
    if length > out.room() {
        return Err(Overflow);
    }
    out.put(b"<https://api.github.com")?;
    out.put(&next)?;
    out.put(b">; rel=\"next\", <https://api.github.com")?;
    out.put(&last)?;
    out.put(b">; rel=\"last\"")
}
fn page_target(target: &[u8], page: u32) -> Result<Box<[u8]>, Overflow> {
    let mut start = None;
    for (at, byte) in target.iter().enumerate() {
        if (*byte == b'?' || *byte == b'&') && target.get(at.saturating_add(1)..).unwrap_or(b"").starts_with(b"page=") {
            start = Some(at.saturating_add(6));
            break;
        }
    }
    let digits = Decimal::of(u64::from(page));
    let (prefix, suffix, separator) = match start {
        Some(at) => {
            let tail = target.get(at..).expect("page start");
            let (value, _) = split(tail, b'&');
            (target.get(..at).expect("page prefix"), tail.get(value.len()..).expect("page suffix"), b"".as_slice())
        }
        None => {
            (target, b"".as_slice(), if target.contains(&b'?') { b"&page=".as_slice() } else { b"?page=".as_slice() })
        }
    };
    let length = prefix
        .len()
        .checked_add(separator.len())
        .ok_or(Overflow)?
        .checked_add(digits.as_bytes().len())
        .ok_or(Overflow)?
        .checked_add(suffix.len())
        .ok_or(Overflow)?;
    let mut out = Writer::new(length);
    out.put(prefix)?;
    out.put(separator)?;
    out.put(digits.as_bytes())?;
    out.put(suffix)?;
    Ok(out.finish())
}

mod people;
pub use people::{collaborators, permission, user};

mod reviews;
pub use reviews::{remarks, reviews};

mod files;
pub use files::{compare, file_rows};

/// Bounded Actions enablement response (forge-github.md, 4.1).
pub fn actions_permissions(enabled: bool, out: &mut Writer) -> Result<(), Overflow> {
    out.put(if enabled { br#"{"enabled":true}"# } else { br#"{"enabled":false}"# })
}
/// The fake's bounded first workflow page (fake-forge.md, 10.1).
pub fn workflows(present: bool, out: &mut Writer) -> Result<(), Overflow> {
    out.put(if present {
        br#"{"total_count":1,"workflows":[{"id":1,"name":"CI","path":".github/workflows/ci.yml","state":"active"}]}"#
    } else {
        br#"{"total_count":0,"workflows":[]}"#
    })
}
