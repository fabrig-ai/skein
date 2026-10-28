//! The fake's App routes, bounded hook decoder and answer encoders.
use super::super::{
    decode,
    limits::Limits,
    request::{self, Method, Request, Scope},
};
use super::{document, output_limits};
use crate::forgejo::{id::unsigned, time};
use alloc::boxed::Box;
use skein_json::{tokenizer, writer::Encoder};
use skein_lib::{
    Env, Intake, Overflow, Queue, Time, Wall, Writer,
    stream::{Down, Up},
};

pub(super) fn route(method: Method, path: &[u8], page: Option<u32>, size: Option<u32>) -> Option<Request> {
    match (method, path) {
        (Method::Get, b"/app") => return Some(Request::App),
        (Method::Get, b"/app/hook/config") => return Some(Request::HookConfig),
        (Method::Patch, b"/app/hook/config") => return Some(Request::SetHook { url: Box::from([]) }),
        (Method::Get, b"/app/installations") => return Some(Request::Installations { page: page? }),
        (Method::Get, b"/installation/repositories") => {
            return Some(Request::Reachable { page: page?, per_page: size? });
        }
        (Method::Get | Method::Post | Method::Put | Method::Patch | Method::Delete, _) => {}
    }
    if let Some(tail) = path.strip_prefix(b"/app/installations/") {
        return match method {
            Method::Get => Some(Request::Installation { id: unsigned(tail)? }),
            Method::Delete => Some(Request::Uninstall { id: unsigned(tail)? }),
            Method::Post => Some(Request::Mint {
                installation: unsigned(tail.strip_suffix(b"/access_tokens")?)?,
                scope: Scope::Full,
            }),
            Method::Put | Method::Patch => None,
        };
    }
    if method == Method::Post {
        let code = path.strip_prefix(b"/app-manifests/")?.strip_suffix(b"/conversions")?;
        return Some(Request::Conversion { code: request::unescape(code)? });
    }
    if method == Method::Get {
        let slug = path.strip_prefix(b"/users/")?.strip_suffix(b"%5Bbot%5D")?;
        return Some(Request::BotUser { slug: request::unescape(slug)? });
    }
    None
}
/// Reads only the dialect's two mint bodies.
#[must_use]
pub fn mint_scope(body: &[u8]) -> Option<Scope> {
    match body {
        b"" => Some(Scope::Full),
        br#"{"permissions":{"contents":"write"}}"# => Some(Scope::Contents),
        _ => None,
    }
}
/// Reads the exact identity query body (forge-github.md, 4.3).
#[must_use]
pub fn graphql(body: &[u8]) -> Option<Request> {
    let query = body.strip_prefix(br#"{"query":""#)?.strip_suffix(br#""}"#)?;
    if query == document::VIEWER { Some(Request::Viewer) } else { None }
}
/// Reads a hook's url through the bounded document decoder.
#[must_use]
pub fn hook_url(body: &[u8], limits: &Limits) -> Option<Box<[u8]>> {
    if body.len() > usize::try_from(limits.body_bytes).ok()? {
        return None;
    }
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut decoder = decode::decoder(document::Kind::HookConfig, limits);
    let mut above = Queue::uncapped();
    let mut below = Queue::uncapped();
    let capacity = u32::try_from(body.len()).ok()?.max(tokenizer::largest_demand(&limits.json));
    let mut intake = Intake::with_capacity(capacity);
    intake.append(body).ok()?;
    tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
    for _tick in 0..body.len().saturating_mul(8).saturating_add(64) {
        if let Some(event) = above.pop() {
            let (demand, step) = decoder.step(event);
            match step {
                decode::Step::Demand => {}
                decode::Step::Done(document) => {
                    let document::Document::HookConfig(hook) = document else {
                        return None;
                    };
                    return Some(hook.url);
                }
                decode::Step::Row(_) | decode::Step::Malformed(_) => return None,
            }
            tokenizer::down(&mut tokenizer, &env, demand, &mut above, &mut below);
        } else {
            let read = match below.pop()? {
                Down::Demand { read, room: 0 } => read,
                Down::Demand { .. } | Down::Send(_) | Down::Finish => return None,
            };
            let delivery = match intake.meet(read) {
                Some(bytes) => Up::Bytes(bytes),
                None => Up::End,
            };
            tokenizer::up(&mut tokenizer, &env, delivery, &mut above, &mut below);
        }
    }
    None
}

#[derive(Debug)]
enum Answer<'a> {
    Installations(&'a [document::Installation]),
    Installation(&'a document::Installation),
    Minted(&'a document::Minted),
    Reachable(&'a [document::Reachable], u64),
    Viewer(&'a document::Viewer),
    Graphql(&'a [u8], &'a [u8]),
    App(&'a document::App),
    AppIdentity(&'a document::App),
    Hook(&'a document::Hook),
    Conversion(&'a document::Conversion),
    BotUser(u64, &'a [u8]),
}
fn encode(answer: &Answer<'_>, out: &mut Writer) -> Result<(), Overflow> {
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    answer_json(answer, &mut measure);
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    answer_json(answer, &mut json);
    out.put(&json.finish())
}

/// Writes a bare installation page.
pub fn installations(rows: &[document::Installation], out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Installations(rows), out)
}

/// Writes one installation's account and state.
pub fn installation(row: &document::Installation, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Installation(row), out)
}

/// Writes an installation token and its expiry.
pub fn minted(row: &document::Minted, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Minted(row), out)
}

/// Writes the reachable repository envelope.
pub fn reachable(rows: &[document::Reachable], total: u64, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Reachable(rows, total), out)
}

/// Writes the identity under data.viewer.
pub fn viewer(row: &document::Viewer, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Viewer(row), out)
}

/// Writes one GraphQL refusal.
pub fn graphql_refusal(kind: &[u8], message: &[u8], out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Graphql(kind, message), out)
}

/// Writes the App's identity, permissions and subscriptions.
pub fn app(row: &document::App, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::App(row), out)
}

/// Writes the App's identity with permissions and events absent.
/// The fake uses this for a scenario whose setup must diagnose missing grants.
pub fn app_identity(row: &document::App, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::AppIdentity(row), out)
}

/// Writes the App's hook address.
pub fn hook(row: &document::Hook, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Hook(row), out)
}

/// Writes the conversion's identity and secrets.
pub fn conversion(row: &document::Conversion, out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::Conversion(row), out)
}

/// Writes the bot's login, numeric id and account type.
pub fn bot_user(id: u64, slug: &[u8], out: &mut Writer) -> Result<(), Overflow> {
    encode(&Answer::BotUser(id, slug), out)
}

fn answer_json(answer: &Answer<'_>, json: &mut Encoder) {
    match answer {
        Answer::Installations(rows) => {
            json.array_start();
            for row in *rows {
                installation_json(row, json);
            }
            json.array_end();
        }
        Answer::Installation(row) => installation_json(row, json),
        Answer::Minted(row) => minted_json(row, json),
        Answer::Reachable(rows, total) => {
            json.object_start();
            json.key(b"total_count");
            json.unsigned(*total);
            json.key(b"repositories");
            json.array_start();
            for row in *rows {
                json.object_start();
                json.key(b"id");
                json.unsigned(row.id);
                json.key(b"owner");
                owner_json(&row.login, row.owner, json);
                json.key(b"name");
                json.string(&row.name);
                json.key(b"private");
                json.boolean(row.private);
                json.object_end();
            }
            json.array_end();
            json.object_end();
        }
        Answer::Viewer(row) => {
            json.object_start();
            json.key(b"data");
            json.object_start();
            json.key(b"viewer");
            json.object_start();
            json.key(b"login");
            json.string(&row.login);
            json.key(b"databaseId");
            json.unsigned(row.id);
            json.object_end();
            json.object_end();
            json.object_end();
        }
        Answer::Graphql(kind, message) => {
            json.object_start();
            json.key(b"errors");
            json.array_start();
            json.object_start();
            json.key(b"type");
            json.string(kind);
            json.key(b"message");
            json.string(message);
            json.object_end();
            json.array_end();
            json.object_end();
        }
        Answer::App(row) => app_json(row, true, json),
        Answer::AppIdentity(row) => app_json(row, false, json),
        Answer::Hook(row) => {
            json.object_start();
            json.key(b"url");
            json.string(&row.url);
            json.object_end();
        }
        Answer::Conversion(row) => {
            json.object_start();
            json.key(b"id");
            json.unsigned(row.id);
            json.key(b"slug");
            json.string(&row.slug);
            json.key(b"owner");
            owner_json(&row.owner, row.owner_id, json);
            json.key(b"pem");
            json.string(row.pem.expose());
            json.key(b"webhook_secret");
            json.string(row.webhook_secret.expose());
            json.object_end();
        }
        Answer::BotUser(id, slug) => {
            json.object_start();
            json.key(b"id");
            json.unsigned(*id);
            json.key(b"login");
            json.string_parts(&[slug, b"[bot]"]);
            json.key(b"type");
            json.string(b"Bot");
            json.object_end();
        }
    }
}
fn owner_json(login: &[u8], id: u64, json: &mut Encoder) {
    json.object_start();
    json.key(b"login");
    json.string(login);
    json.key(b"id");
    json.unsigned(id);
    json.object_end();
}
fn installation_json(row: &document::Installation, json: &mut Encoder) {
    json.object_start();
    json.key(b"id");
    json.unsigned(row.id);
    json.key(b"account");
    json.object_start();
    json.key(b"login");
    json.string(&row.login);
    json.key(b"id");
    json.unsigned(row.account);
    json.key(b"type");
    json.string(match row.kind {
        document::Account::User => b"User",
        document::Account::Organization => b"Organization",
    });
    json.object_end();
    json.key(b"created_at");
    json.string(&time::write(row.created));
    json.key(b"suspended_at");
    if row.suspended {
        json.string(&time::write(row.created));
    } else {
        json.null();
    }
    json.object_end();
}

fn app_json(row: &document::App, grants: bool, json: &mut Encoder) {
    json.object_start();
    json.key(b"id");
    json.unsigned(row.id);
    json.key(b"slug");
    json.string(&row.slug);
    if grants {
        json.key(b"permissions");
        json.object_start();
        for permission in &row.permissions {
            json.key(&permission.name);
            json.string(permission.access.text());
        }
        json.object_end();
        json.key(b"events");
        json.array_start();
        for event in &row.events {
            json.string(event);
        }
        json.array_end();
    }
    json.object_end();
}

fn minted_json(row: &document::Minted, json: &mut Encoder) {
    json.object_start();
    json.key(b"token");
    json.string(row.token.expose());
    json.key(b"expires_at");
    json.string(&time::write(row.expires));
    json.key(b"permissions");
    json.object_start();
    json.key(b"contents");
    json.string(row.permissions.contents.text());
    json.key(b"pull_requests");
    json.string(row.permissions.pull_requests.text());
    json.object_end();
    json.object_end();
}
