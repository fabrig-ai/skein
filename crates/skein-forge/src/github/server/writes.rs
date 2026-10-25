//! Bounded write request bodies and minimal mutation answers.
use super::{Limits, Request, output_limits};
use alloc::boxed::Box;
use skein_json::{
    Token,
    tokenizer::{self, Event},
    writer::Encoder,
};
use skein_lib::{
    Env, Intake, List, Overflow, Queue, Time, Wall, Writer,
    stream::{Down, Up},
};

/// Repository write paths carry placeholders until `write_body` decodes the body.
pub(super) fn route(method: super::Method, path: &[u8]) -> Option<Request> {
    let path = path.strip_prefix(b"/repos/")?;
    let (owner, tail) = super::split(path, b'/');
    let (name, tail) = super::split(tail, b'/');
    let owner = super::request::unescape(owner)?;
    let name = super::request::unescape(name)?;
    if method == super::Method::Post && tail == b"git/refs" {
        return Some(Request::CreateBranch { owner, name, branch: Box::from([]), commit: [0; 32] });
    }
    if method == super::Method::Delete
        && let Some(branch) = tail.strip_prefix(b"git/refs/heads/")
    {
        return Some(Request::DeleteBranch { owner, name, branch: super::request::unescape(branch)? });
    }
    if method == super::Method::Post
        && let Some(commit) = tail.strip_prefix(b"statuses/")
    {
        return Some(Request::Status {
            owner,
            name,
            commit: super::id::commit(commit)?,
            context: Box::from([]),
            check: crate::api::Check::Pending,
        });
    }
    if method == super::Method::Post && tail == b"issues" {
        return Some(Request::CreateIssue { owner, name, title: Box::from([]), body: Box::from([]) });
    }
    if method == super::Method::Post && tail == b"pulls" {
        return Some(Request::OpenPull {
            owner,
            name,
            title: Box::from([]),
            body: Box::from([]),
            head: Box::from([]),
            base: Box::from([]),
        });
    }
    if let Some(tail) = tail.strip_prefix(b"pulls/") {
        let (number, suffix) = super::split(tail, b'/');
        let number = super::unsigned(number)?;
        return match suffix {
            b"merge" if method == super::Method::Put => Some(Request::Merge {
                owner,
                name,
                number,
                head: [0; 32],
                method: crate::github::plans::tables::Method::Squash,
            }),
            b"update-branch" if method == super::Method::Put => {
                Some(Request::Update { owner, name, number, head: [0; 32] })
            }
            b"reviews" if method == super::Method::Post => Some(Request::Review {
                owner,
                name,
                number,
                verdict: crate::api::Verdict::Comment,
                body: Box::from([]),
            }),
            b"requested_reviewers" if method == super::Method::Post || method == super::Method::Delete => {
                Some(Request::Reviewers {
                    owner,
                    name,
                    number,
                    add: method == super::Method::Post,
                    logins: Box::from([]),
                })
            }
            _ => None,
        };
    }
    let tail = tail.strip_prefix(b"issues/")?;
    let (number, suffix) = super::split(tail, b'/');
    let number = super::unsigned(number)?;
    match suffix {
        b"" if method == super::Method::Patch => Some(Request::Edit { owner, name, number, title: None, body: None }),
        b"comments" if method == super::Method::Post => {
            Some(Request::Post { owner, name, number, body: Box::from([]) })
        }
        _ => None,
    }
}
/// Writes the minimal JSON object accepted by Done-result mutations.
pub fn done(out: &mut Writer) -> Result<(), Overflow> {
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    measure.object_start();
    measure.object_end();
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    json.object_start();
    json.object_end();
    out.put(&json.finish())
}
#[derive(Debug)]
struct Field {
    key: &'static [u8],
    body: bool,
    required: bool,
}
#[derive(Debug)]
struct Value {
    key: &'static [u8],
    text: Box<[u8]>,
}
#[derive(Debug)]
struct Fields {
    values: List<Value>,
    logins: Option<List<Box<[u8]>>>,
}
impl Fields {
    fn get(&self, key: &[u8]) -> Option<&[u8]> {
        for value in &self.values {
            if value.key == key {
                return Some(&value.text);
            }
        }
        None
    }
}
#[derive(Debug)]
enum Want {
    Start,
    Key,
    Value(&'static [u8], u32),
    VariablesStart,
    ListStart,
    ListItem,
    Unknown(u32),
    End,
    Done,
}
#[derive(Debug)]
struct Parser {
    want: Want,
    fields: Fields,
    variables: bool,
    has_variables: bool,
}
impl Parser {
    #[expect(clippy::too_many_lines, reason = "the bounded tokenizer states exhaustively handle every event")]
    fn event(&mut self, event: Event, specs: &[Field], limits: &Limits) -> Option<tokenizer::Request> {
        let want = core::mem::replace(&mut self.want, Want::Done);
        match want {
            Want::Start => {
                if event != Event::Token(Token::ObjectStart) {
                    return None;
                }
                self.want = Want::Key;
            }
            Want::Key => match event {
                Event::Token(Token::ObjectEnd) => {
                    if self.variables {
                        self.variables = false;
                        self.want = Want::Key;
                        return Some(tokenizer::Request::Next);
                    }
                    if specs.first()?.key == b"query" && !self.has_variables {
                        return None;
                    }
                    for field in specs {
                        if field.required
                            && self.fields.get(field.key).is_none()
                            && !(field.key == b"reviewers" && self.fields.logins.is_some())
                        {
                            return None;
                        }
                    }
                    self.want = Want::End;
                }
                Event::Token(Token::Key(key)) => {
                    if specs.first()?.key == b"query" && key.as_ref() == b"variables" {
                        if self.has_variables || self.variables {
                            return None;
                        }
                        self.has_variables = true;
                        self.want = Want::VariablesStart;
                        return Some(tokenizer::Request::Next);
                    }
                    for field in specs {
                        if field.key == key.as_ref() {
                            if specs.first()?.key == b"query" && (field.key != b"query") != self.variables {
                                return None;
                            }
                            if self.fields.get(field.key).is_some()
                                || (field.key == b"reviewers" && self.fields.logins.is_some())
                            {
                                return None;
                            }
                            if field.key == b"reviewers" {
                                self.want = Want::ListStart;
                                return Some(tokenizer::Request::Next);
                            }
                            let cap = if field.body { limits.body_bytes } else { limits.string_bytes };
                            self.want = Want::Value(field.key, cap);
                            return Some(tokenizer::Request::Text(cap));
                        }
                    }
                    self.want = Want::Unknown(0);
                    return Some(tokenizer::Request::Text(0));
                }
                Event::Token(
                    Token::ObjectStart
                    | Token::ArrayStart
                    | Token::ArrayEnd
                    | Token::String(_)
                    | Token::Number(_)
                    | Token::True
                    | Token::False
                    | Token::Null,
                )
                | Event::Long(_)
                | Event::Skipped(_)
                | Event::Done
                | Event::Failed(_)
                | Event::Closed => return None,
            },
            Want::Value(key, cap) => {
                let text = match event {
                    Event::Token(Token::String(text)) => text,
                    Event::Token(
                        Token::ObjectStart
                        | Token::ObjectEnd
                        | Token::ArrayStart
                        | Token::ArrayEnd
                        | Token::Key(_)
                        | Token::Number(_)
                        | Token::True
                        | Token::False
                        | Token::Null,
                    )
                    | Event::Long(_)
                    | Event::Skipped(_)
                    | Event::Done
                    | Event::Failed(_)
                    | Event::Closed => return None,
                };
                if u64::try_from(text.len()).ok()? > u64::from(cap) {
                    return None;
                }
                self.fields.values.push(Value { key, text }).ok()?;
                self.want = Want::Key;
            }
            Want::VariablesStart => {
                if event != Event::Token(Token::ObjectStart) {
                    return None;
                }
                self.variables = true;
                self.want = Want::Key;
            }
            Want::ListStart => {
                if event != Event::Token(Token::ArrayStart) {
                    return None;
                }
                self.fields.logins = Some(List::with_capacity(limits.inner));
                self.want = Want::ListItem;
                return Some(tokenizer::Request::Text(limits.string_bytes));
            }
            Want::ListItem => match event {
                Event::Token(Token::ArrayEnd) => self.want = Want::Key,
                Event::Token(Token::String(login)) => {
                    if u64::try_from(login.len()).ok()? > u64::from(limits.string_bytes) {
                        return None;
                    }
                    self.fields.logins.as_mut()?.push(login).ok()?;
                    self.want = Want::ListItem;
                    return Some(tokenizer::Request::Text(limits.string_bytes));
                }
                Event::Token(
                    Token::ObjectStart
                    | Token::ObjectEnd
                    | Token::ArrayStart
                    | Token::Key(_)
                    | Token::Number(_)
                    | Token::True
                    | Token::False
                    | Token::Null,
                )
                | Event::Long(_)
                | Event::Skipped(_)
                | Event::Done
                | Event::Failed(_)
                | Event::Closed => return None,
            },
            Want::Unknown(depth) => {
                let depth = match event {
                    Event::Token(Token::ObjectStart | Token::ArrayStart) => depth.checked_add(1)?,
                    Event::Token(Token::ObjectEnd | Token::ArrayEnd) => depth.checked_sub(1)?,
                    Event::Token(Token::Key(_)) => {
                        self.want = Want::Unknown(depth);
                        return Some(tokenizer::Request::Text(0));
                    }
                    Event::Token(Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null)
                    | Event::Long(_) => depth,
                    Event::Skipped(_) | Event::Done | Event::Failed(_) | Event::Closed => return None,
                };
                if depth == 0 {
                    self.want = Want::Key;
                } else {
                    self.want = Want::Unknown(depth);
                    return Some(tokenizer::Request::Text(0));
                }
            }
            Want::End => {
                if event != Event::Done {
                    return None;
                }
                self.want = Want::Done;
                return Some(tokenizer::Request::Close);
            }
            Want::Done => return None,
        }
        Some(tokenizer::Request::Next)
    }
}
fn parse(body: &[u8], specs: &[Field], limits: &Limits) -> Option<Fields> {
    if u64::try_from(body.len()).ok()? > u64::from(limits.body_bytes) {
        return None;
    }
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut parser = Parser {
        want: Want::Start,
        variables: false,
        has_variables: false,
        fields: Fields { values: List::with_capacity(u32::try_from(specs.len()).ok()?), logins: None },
    };
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake =
        Intake::with_capacity(u32::try_from(body.len()).ok()?.max(tokenizer::largest_demand(&limits.json)));
    intake.append(body).ok()?;
    tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
    for _ in 0..body.len().checked_mul(8)?.checked_add(64)? {
        if let Some(event) = above.pop() {
            let demand = parser.event(event, specs, limits)?;
            if demand == tokenizer::Request::Close {
                return Some(parser.fields);
            }
            tokenizer::down(&mut tokenizer, &env, demand, &mut above, &mut below);
        } else {
            let read = match below.pop()? {
                Down::Demand { read, room: 0 } => read,
                Down::Demand { .. } | Down::Send(_) | Down::Finish => return None,
            };
            let event = match intake.meet(read) {
                Some(bytes) => Up::Bytes(bytes),
                None => Up::End,
            };
            tokenizer::up(&mut tokenizer, &env, event, &mut above, &mut below);
        }
    }
    None
}
/// Decodes a write's body separately from route dispatch; path fields come from the caller.
#[must_use]
#[expect(clippy::too_many_lines, reason = "the dispatcher exhaustively names each write request body")]
pub fn write_body(request: Request, body: &[u8], limits: &Limits) -> Option<Request> {
    match request {
        Request::Merge { owner, name, number, .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"sha", body: true, required: true },
                    Field { key: b"merge_method", body: true, required: true },
                ],
                limits,
            )?;
            let method = match fields.get(b"merge_method")? {
                b"merge" => crate::github::plans::tables::Method::Merge,
                b"squash" => crate::github::plans::tables::Method::Squash,
                b"rebase" => crate::github::plans::tables::Method::Rebase,
                _ => return None,
            };
            Some(Request::Merge { owner, name, number, head: super::id::commit(fields.get(b"sha")?)?, method })
        }
        Request::Enqueue { .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"query", body: true, required: true },
                    Field { key: b"id", body: false, required: true },
                    Field { key: b"head", body: true, required: true },
                ],
                limits,
            )?;
            if fields.get(b"query")? != super::document::ENQUEUE {
                return None;
            }
            Some(Request::Enqueue { id: Box::from(fields.get(b"id")?), head: super::id::commit(fields.get(b"head")?)? })
        }
        Request::Reviewers { owner, name, number, add, .. } => {
            let fields = parse(body, &[Field { key: b"reviewers", body: false, required: true }], limits)?;
            Some(Request::Reviewers { owner, name, number, add, logins: fields.logins?.into_boxed() })
        }
        Request::Update { owner, name, number, .. } => {
            let fields = parse(body, &[Field { key: b"expected_head_sha", body: true, required: true }], limits)?;
            Some(Request::Update { owner, name, number, head: super::id::commit(fields.get(b"expected_head_sha")?)? })
        }
        Request::Review { owner, name, number, .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"event", body: true, required: true },
                    Field { key: b"body", body: true, required: true },
                ],
                limits,
            )?;
            let verdict = match fields.get(b"event")? {
                b"APPROVE" => crate::api::Verdict::Approve,
                b"REQUEST_CHANGES" => crate::api::Verdict::RequestChanges,
                b"COMMENT" => crate::api::Verdict::Comment,
                _ => return None,
            };
            Some(Request::Review { owner, name, number, verdict, body: Box::from(fields.get(b"body")?) })
        }
        Request::Post { owner, name, number, .. } => {
            let fields = parse(body, &[Field { key: b"body", body: true, required: true }], limits)?;
            Some(Request::Post { owner, name, number, body: Box::from(fields.get(b"body")?) })
        }
        Request::CreateIssue { owner, name, .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"title", body: false, required: true },
                    Field { key: b"body", body: true, required: true },
                ],
                limits,
            )?;
            Some(Request::CreateIssue {
                owner,
                name,
                title: Box::from(fields.get(b"title")?),
                body: Box::from(fields.get(b"body")?),
            })
        }
        Request::OpenPull { owner, name, .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"title", body: false, required: true },
                    Field { key: b"body", body: true, required: true },
                    Field { key: b"head", body: false, required: true },
                    Field { key: b"base", body: false, required: true },
                ],
                limits,
            )?;
            Some(Request::OpenPull {
                owner,
                name,
                title: Box::from(fields.get(b"title")?),
                body: Box::from(fields.get(b"body")?),
                head: Box::from(fields.get(b"head")?),
                base: Box::from(fields.get(b"base")?),
            })
        }
        Request::CreateBranch { owner, name, .. } => {
            let fields = parse(
                body,
                &[Field { key: b"ref", body: true, required: true }, Field { key: b"sha", body: true, required: true }],
                limits,
            )?;
            let branch = fields.get(b"ref")?.strip_prefix(b"refs/heads/")?;
            if u64::try_from(branch.len()).ok()? > u64::from(limits.string_bytes) {
                return None;
            }
            Some(Request::CreateBranch {
                owner,
                name,
                branch: Box::from(branch),
                commit: super::id::commit(fields.get(b"sha")?)?,
            })
        }
        Request::Status { owner, name, commit, .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"state", body: false, required: true },
                    Field { key: b"context", body: false, required: true },
                ],
                limits,
            )?;
            let check = match fields.get(b"state")? {
                b"pending" => crate::api::Check::Pending,
                b"success" => crate::api::Check::Passed,
                b"failure" => crate::api::Check::Failed,
                _ => return None,
            };
            Some(Request::Status { owner, name, commit, context: Box::from(fields.get(b"context")?), check })
        }
        Request::Edit { owner, name, number, .. } => {
            let fields = parse(
                body,
                &[
                    Field { key: b"state", body: false, required: false },
                    Field { key: b"title", body: false, required: false },
                    Field { key: b"body", body: true, required: false },
                ],
                limits,
            )?;
            if let Some(state) = fields.get(b"state") {
                if fields.get(b"title").is_some() || fields.get(b"body").is_some() {
                    return None;
                }
                return match state {
                    b"open" => Some(Request::Reopen { owner, name, number }),
                    b"closed" => Some(Request::Close { owner, name, number }),
                    _ => None,
                };
            }
            let title = copy_optional(fields.get(b"title"));
            let body = copy_optional(fields.get(b"body"));
            Some(Request::Edit { owner, name, number, title, body })
        }
        Request::Reopen { owner, name, number } => {
            let fields = parse(body, &[Field { key: b"state", body: false, required: true }], limits)?;
            if fields.get(b"state")? != b"open" {
                return None;
            }
            Some(Request::Reopen { owner, name, number })
        }
        Request::Close { owner, name, number } => {
            let fields = parse(body, &[Field { key: b"state", body: false, required: true }], limits)?;
            if fields.get(b"state")? != b"closed" {
                return None;
            }
            Some(Request::Close { owner, name, number })
        }
        Request::DeleteBranch { .. } => {
            if body.is_empty() {
                Some(request)
            } else {
                None
            }
        }
        Request::LogPage { .. }
        | Request::LogDelivery { .. }
        | Request::Installations { .. }
        | Request::Installation { .. }
        | Request::Mint { .. }
        | Request::Reachable { .. }
        | Request::PullGraphql { .. }
        | Request::Viewer
        | Request::App
        | Request::HookConfig
        | Request::SetHook { .. }
        | Request::Uninstall { .. }
        | Request::Conversion { .. }
        | Request::BotUser { .. }
        | Request::Repository { .. }
        | Request::ActionsPermissions { .. }
        | Request::Workflows { .. }
        | Request::Locate { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullsFor { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::CombinedStatus { .. }
        | Request::CheckRuns { .. }
        | Request::Job { .. }
        | Request::JobLog { .. }
        | Request::Followed { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Raw { .. }
        | Request::RulesForBranch { .. }
        | Request::Ruleset { .. }
        | Request::ClassicProtection { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::User { .. } => None,
    }
}

#[expect(clippy::manual_map, reason = "step code spells out optional copies without closure combinators")]
fn copy_optional(text: Option<&[u8]>) -> Option<Box<[u8]>> {
    match text {
        Some(text) => Some(Box::from(text)),
        None => None,
    }
}

/// Writes the creation's exact issue or pull number.
pub fn number(number: u64, out: &mut Writer) -> Result<(), Overflow> {
    numeric(b"number", number, out)
}
fn numeric(field: &[u8], value: u64, out: &mut Writer) -> Result<(), Overflow> {
    let limits = output_limits(out)?;
    let mut measure = Encoder::measure(&limits);
    numeric_json(field, value, &mut measure);
    let Ok(length) = measure.measured() else {
        return Err(Overflow);
    };
    let mut json = Encoder::write(length, &limits);
    numeric_json(field, value, &mut json);
    out.put(&json.finish())
}
fn numeric_json(field: &[u8], value: u64, json: &mut Encoder) {
    json.object_start();
    json.key(field);
    json.unsigned(value);
    json.object_end();
}

/// Writes the mutation's exact comment or review id.
pub fn id(id: u64, out: &mut Writer) -> Result<(), Overflow> {
    numeric(b"id", id, out)
}
