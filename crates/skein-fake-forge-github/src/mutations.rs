//! Person-side mutations decoded locally; the shared domain enforces permission and bounds.
use crate::{
    answers::{self, Body},
    auth::Access,
    reads,
    service::{Outcome, Pending, PendingKind, Response, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::github::request::Method;
use skein_json::Token;
use skein_lib::{List, ReplyTo, bytes};

const PULL_REQUEST_FIELDS: u32 = 1 | 2 | 4 | 8 | 16;
const STATUS_CONTEXTS: u32 = 32;
const STATUS_STRICT: u32 = 64;
const STATUS_FIELDS: u32 = STATUS_CONTEXTS | STATUS_STRICT;
const QUEUE_TIMEOUT: u32 = 128;
const QUEUE_GROUPING: u32 = 256;
const QUEUE_BUILD: u32 = 512;
const QUEUE_MERGE: u32 = 1024;
const QUEUE_METHOD: u32 = 2048;
const QUEUE_MIN: u32 = 4096;
const QUEUE_WAIT: u32 = 8192;
const QUEUE_FIELDS: u32 =
    QUEUE_TIMEOUT | QUEUE_GROUPING | QUEUE_BUILD | QUEUE_MERGE | QUEUE_METHOD | QUEUE_MIN | QUEUE_WAIT;
const QUEUE_TIMEOUT_MAX: u64 = 360;
const QUEUE_ENTRIES_MAX: u64 = 100;
const QUEUE_WAIT_MAX: u64 = 360;

#[derive(Debug)]
pub(crate) enum Route {
    CreateRules { repository: Box<[u8]> },
    DeleteRules { repository: Box<[u8]>, id: u64 },
}
#[derive(Debug)]
pub(crate) struct Mutation {
    repository: Box<[u8]>,
    creating: bool,
}
pub(crate) fn route(method: Method, target: &[u8]) -> Option<Route> {
    let tail = target.strip_prefix(b"/repos/")?;
    let end = bytes::find(tail, b"/rulesets")?;
    let repository = tail.get(..end)?;
    let suffix = tail.get(end.checked_add(9)?..)?;
    if method == Method::Post && suffix.is_empty() {
        return Some(Route::CreateRules { repository: bytes::copy_of(repository) });
    }
    if method == Method::Delete {
        let id = skein_forge::forgejo::id::unsigned(suffix.strip_prefix(b"/")?)?;
        return Some(Route::DeleteRules { repository: bytes::copy_of(repository), id });
    }
    None
}
pub(crate) fn call(service: &Service, route: Route, access: Access, body: &[u8], reply_to: ReplyTo) -> Outcome {
    if let Access::Installation { .. } = access {
        return Outcome::Reply(answers::refusal(403, crate::texts::RULESET_INTEGRATION, &service.limits));
    }
    let (repository, creating, id) = match route {
        Route::CreateRules { repository } => (repository, true, service.ruleset_id),
        Route::DeleteRules { repository, id } => (repository, false, id),
    };
    let mut found = false;
    let mut organization = false;
    for (_, row) in &service.repositories {
        if let Some(full) = reads::full(row)
            && full == repository
        {
            found = true;
            organization = row.organization;
        }
    }
    if !found || id != service.ruleset_id {
        return Outcome::Reply(answers::missing(&service.limits));
    }
    let protection = if creating {
        let mut missing_parameters = None;
        let Some(rule) = protection(service, body, &mut missing_parameters) else {
            let message = match missing_parameters {
                Some(index) => {
                    let index = skein_lib::Decimal::of(u64::from(index));
                    let Some(prefix) =
                        crate::service::joined(b"Invalid request.\n\nInvalid property /rules/", index.as_bytes())
                    else {
                        return Outcome::Reply(answers::failed());
                    };
                    let Some(message) =
                        crate::service::joined(&prefix, b": data matches no possible input. See `documentation_url`.")
                    else {
                        return Outcome::Reply(answers::failed());
                    };
                    message
                }
                None => bytes::copy_of(b"Validation Failed"),
            };
            return Outcome::Reply(answers::refusal(422, &message, &service.limits));
        };
        if rule.queue && !organization {
            // Inferred: merge queues require an organization; live's refusal text is unknown.
            return Outcome::Reply(answers::refusal(422, b"Validation Failed", &service.limits));
        }
        Some(rule)
    } else {
        if !body.is_empty() {
            return Outcome::Reply(answers::refusal(422, b"Validation Failed", &service.limits));
        }
        None
    };
    let event = fake::Event::Call {
        reply_to,
        user: access.user(),
        repository: repository.clone(),
        op: a::Op::Write(a::Write::SetProtection { protection }),
    };
    Outcome::Call(Pending { rate: None, kind: PendingKind::Mutation(Mutation { repository, creating }) }, event)
}
pub(crate) fn finish(
    service: &Service,
    pending: Mutation,
    result: Result<a::Answer, a::Error>,
    domain: &fake::Domain,
) -> Response {
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => return crate::writes::refused_mutation(error, &service.limits),
    };
    let a::Answer::Done = answer else { return answers::failed() };
    if !pending.creating {
        return crate::auth::empty();
    }
    let Some(rule) = domain.inspect_repository_protection(&pending.repository) else { return answers::failed() };
    let bypass = if rule.bypassing_users.contains(&service.bot) {
        skein_forge::api::Bypass::Always
    } else {
        skein_forge::api::Bypass::Never
    };
    let mut response = answers::response(201, &Body::Ruleset(bypass), &service.limits);
    let id = skein_lib::Decimal::of(service.ruleset_id);
    let Some(size) = response.body.len().checked_add(id.as_bytes().len()) else { return answers::failed() };
    let Some(size) = size.checked_add(6) else { return answers::failed() };
    if size > usize::try_from(service.limits.response).expect("u32 fits") {
        return answers::failed();
    }
    let Some(tail) = response.body.strip_prefix(b"{") else { return answers::failed() };
    let mut out = skein_lib::Writer::new(size);
    for part in [b"{\"id\":".as_slice(), id.as_bytes(), b",", tail] {
        if out.put(part).is_err() {
            return answers::failed();
        }
    }
    response.body = out.finish();
    response
}

enum Key {
    Name(Box<[u8]>),
    End,
}
struct Cursor {
    tokens: List<Token>,
    at: u32,
}
impl Cursor {
    fn take(&mut self) -> Option<&Token> {
        let at = self.at;
        self.at = self.at.checked_add(1)?;
        self.tokens.get(at)
    }
    fn want(&mut self, token: &Token) -> Option<()> {
        if self.take()? != token {
            return None;
        }
        Some(())
    }
    fn text(&mut self) -> Option<Box<[u8]>> {
        if let Token::String(value) = self.take()? { Some(value.clone()) } else { None }
    }
    fn number(&mut self) -> Option<u64> {
        if let Token::Number(value) = self.take()? { skein_forge::forgejo::id::unsigned(value) } else { None }
    }
    fn key(&mut self) -> Option<Key> {
        let token = self.take()?;
        if token == &Token::ObjectEnd {
            return Some(Key::End);
        }
        if let Token::Key(value) = token {
            return Some(Key::Name(value.clone()));
        }
        None
    }
    fn end(&self) -> bool {
        self.at == self.tokens.len()
    }
}
fn once(seen: &mut u32, bit: u32) -> Option<()> {
    if *seen & bit != 0 {
        return None;
    }
    *seen |= bit;
    Some(())
}
// Request subset from the documented repository-ruleset schema (fake-forge.md, 10.1).
fn protection(service: &Service, body: &[u8], missing: &mut Option<u32>) -> Option<a::Protection> {
    if !service.body_fits(body) {
        return None;
    }
    let mut cursor = Cursor { tokens: crate::manifest::tokenize(body, &service.limits.github)?, at: 0 };
    let mut rule = a::Protection {
        branch: Box::new([]),
        contexts: Box::new([]),
        approvals: 0,
        dismiss_stale: false,
        bypassing_users: Box::new([]),
        queue: false,
        linear: false,
        code_owners: false,
    };
    cursor.want(&Token::ObjectStart)?;
    let mut seen = 0_u32;
    for _ in 0..cursor.tokens.len() {
        let Key::Name(key) = cursor.key()? else { break };
        match key.as_ref() {
            b"name" => {
                once(&mut seen, 1)?;
                if cursor.text()?.is_empty() {
                    return None;
                }
            }
            b"target" => {
                once(&mut seen, 2)?;
                if cursor.text()?.as_ref() != b"branch" {
                    return None;
                }
            }
            b"enforcement" => {
                once(&mut seen, 4)?;
                if cursor.text()?.as_ref() != b"active" {
                    return None;
                }
            }
            b"conditions" => {
                once(&mut seen, 8)?;
                rule.branch = branch(&mut cursor)?;
            }
            b"bypass_actors" => {
                once(&mut seen, 16)?;
                rule.bypassing_users = actors(&mut cursor, service)?;
            }
            b"rules" => {
                once(&mut seen, 32)?;
                rules(&mut cursor, &mut rule, service.limits.github.inner, missing)?;
            }
            _ => return None,
        }
    }
    if seen != 63 || !cursor.end() {
        return None;
    }
    Some(rule)
}
fn branch(c: &mut Cursor) -> Option<Box<[u8]>> {
    c.want(&Token::ObjectStart)?;
    c.want(&Token::Key(bytes::copy_of(b"ref_name")))?;
    c.want(&Token::ObjectStart)?;
    c.want(&Token::Key(bytes::copy_of(b"include")))?;
    c.want(&Token::ArrayStart)?;
    let branch = c.text()?;
    let branch = bytes::copy_of(branch.strip_prefix(b"refs/heads/")?);
    if branch.is_empty() {
        return None;
    }
    c.want(&Token::ArrayEnd)?;
    c.want(&Token::Key(bytes::copy_of(b"exclude")))?;
    c.want(&Token::ArrayStart)?;
    c.want(&Token::ArrayEnd)?;
    c.want(&Token::ObjectEnd)?;
    c.want(&Token::ObjectEnd)?;
    Some(branch)
}
fn actors(c: &mut Cursor, service: &Service) -> Option<Box<[u64]>> {
    let mut actors = List::with_capacity(service.limits.github.inner);
    c.want(&Token::ArrayStart)?;
    for _ in 0..c.tokens.len() {
        if c.tokens.get(c.at)? == &Token::ArrayEnd {
            c.want(&Token::ArrayEnd)?;
            return Some(actors.into_boxed());
        }
        c.want(&Token::ObjectStart)?;
        let (mut id, mut kind, mut mode) = (None, None, None);
        for _ in 0..c.tokens.len() {
            let Key::Name(key) = c.key()? else { break };
            match key.as_ref() {
                b"actor_id" if id.is_none() => id = Some(c.number()?),
                b"actor_type" if kind.is_none() => kind = Some(c.text()?),
                b"bypass_mode" if mode.is_none() => mode = Some(c.text()?),
                _ => return None,
            }
        }
        if mode?.as_ref() != b"always" {
            return None;
        }
        let id = id?;
        let user = match kind?.as_ref() {
            b"Integration" if id == service.app.id => service.bot,
            b"User" => id,
            _ => return None,
        };
        actors.push(user).ok()?;
    }
    None
}
fn rules(c: &mut Cursor, rule: &mut a::Protection, bound: u32, missing: &mut Option<u32>) -> Option<()> {
    c.want(&Token::ArrayStart)?;
    let mut seen = 0_u32;
    for index in 0..c.tokens.len() {
        if c.tokens.get(c.at)? == &Token::ArrayEnd {
            c.want(&Token::ArrayEnd)?;
            return Some(());
        }
        c.want(&Token::ObjectStart)?;
        let (mut kind, mut parameters) = (None, None);
        for _ in 0..c.tokens.len() {
            let Key::Name(key) = c.key()? else { break };
            match key.as_ref() {
                b"type" if kind.is_none() => kind = Some(c.text()?),
                b"parameters" if parameters.is_none() => parameters = Some(parameter(c, bound)?),
                _ => return None,
            }
        }
        let kind = kind?;
        let (bit, required) = match kind.as_ref() {
            b"pull_request" => (1, PULL_REQUEST_FIELDS),
            b"required_status_checks" => (2, STATUS_FIELDS),
            b"merge_queue" => (4, QUEUE_FIELDS),
            _ => return None,
        };
        let Some(parameters) = parameters else {
            *missing = Some(index);
            return None;
        };
        if parameters.fields & required != required {
            *missing = Some(index);
            return None;
        }
        if parameters.fields != required {
            return None;
        }
        once(&mut seen, bit)?;
        match kind.as_ref() {
            b"pull_request" => rule.approvals = parameters.approvals,
            b"required_status_checks" => rule.contexts = parameters.contexts?,
            b"merge_queue" => rule.queue = true,
            _ => return None,
        }
    }
    None
}

#[derive(Debug)]
struct Parameter {
    fields: u32,
    approvals: u32,
    contexts: Option<Box<[Box<[u8]>]>>,
}
fn parameter(c: &mut Cursor, bound: u32) -> Option<Parameter> {
    c.want(&Token::ObjectStart)?;
    let mut result = Parameter { fields: 0, approvals: 0, contexts: None };
    for _ in 0..c.tokens.len() {
        let Key::Name(key) = c.key()? else { break };
        match key.as_ref() {
            b"required_approving_review_count" => {
                once(&mut result.fields, 1)?;
                let count = c.number()?;
                result.approvals = u32::try_from(count).ok()?;
            }
            b"dismiss_stale_reviews_on_push"
            | b"require_code_owner_review"
            | b"require_last_push_approval"
            | b"required_review_thread_resolution" => {
                let bit = match key.as_ref() {
                    b"dismiss_stale_reviews_on_push" => 2,
                    b"require_code_owner_review" => 4,
                    b"require_last_push_approval" => 8,
                    b"required_review_thread_resolution" => 16,
                    _ => return None,
                };
                once(&mut result.fields, bit)?;
                c.want(&Token::False)?;
            }
            b"required_status_checks" => {
                once(&mut result.fields, STATUS_CONTEXTS)?;
                result.contexts = Some(contexts(c, bound)?);
            }
            b"strict_required_status_checks_policy" => {
                once(&mut result.fields, STATUS_STRICT)?;
                c.want(&Token::False)?;
            }
            b"check_response_timeout_minutes" => {
                once(&mut result.fields, QUEUE_TIMEOUT)?;
                let value = c.number()?;
                if value == 0 || value > QUEUE_TIMEOUT_MAX {
                    return None;
                }
            }
            b"max_entries_to_build"
            | b"max_entries_to_merge"
            | b"min_entries_to_merge"
            | b"min_entries_to_merge_wait_minutes" => {
                let (bit, max) = match key.as_ref() {
                    b"max_entries_to_build" => (QUEUE_BUILD, QUEUE_ENTRIES_MAX),
                    b"max_entries_to_merge" => (QUEUE_MERGE, QUEUE_ENTRIES_MAX),
                    b"min_entries_to_merge" => (QUEUE_MIN, QUEUE_ENTRIES_MAX),
                    b"min_entries_to_merge_wait_minutes" => (QUEUE_WAIT, QUEUE_WAIT_MAX),
                    _ => return None,
                };
                once(&mut result.fields, bit)?;
                if c.number()? > max {
                    return None;
                }
            }
            b"grouping_strategy" => {
                once(&mut result.fields, QUEUE_GROUPING)?;
                if ![b"ALLGREEN".as_slice(), b"HEADGREEN"].contains(&c.text()?.as_ref()) {
                    return None;
                }
            }
            b"merge_method" => {
                once(&mut result.fields, QUEUE_METHOD)?;
                if ![b"MERGE".as_slice(), b"SQUASH", b"REBASE"].contains(&c.text()?.as_ref()) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(result)
}

fn contexts(c: &mut Cursor, bound: u32) -> Option<Box<[Box<[u8]>]>> {
    c.want(&Token::ArrayStart)?;
    let mut names = List::with_capacity(bound);
    for _ in 0..c.tokens.len() {
        if c.tokens.get(c.at)? == &Token::ArrayEnd {
            break;
        }
        c.want(&Token::ObjectStart)?;
        c.want(&Token::Key(bytes::copy_of(b"context")))?;
        names.push(c.text()?).ok()?;
        c.want(&Token::ObjectEnd)?;
    }
    c.want(&Token::ArrayEnd)?;
    Some(names.into_boxed())
}
