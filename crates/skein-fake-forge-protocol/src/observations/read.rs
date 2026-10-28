use alloc::boxed::Box;
use skein_fake_forge_domain::{Branches, Limits, Observation, Operation, api as a, promises::Recorded};
use skein_json::{
    Token,
    tokenizer::{self as json, Tokenizer},
};
use skein_lib::{
    Env, List, Queue, Time, Wall, base64,
    bytes::copy_of,
    stream::{Down, Held},
};

/// A refused member, and its observation variant when the tag is known.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Refusal {
    pub variant: Option<Box<[u8]>>,
    pub member: Box<[u8]>,
    pub reason: Reason,
}

/// Why the named member could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reason {
    Unknown,
    Repeated,
    Missing,
    Type,
    Value,
    Limit,
    Json,
}

fn refuse(variant: &[u8], member: &[u8], reason: Reason) -> Refusal {
    Refusal { variant: if variant.is_empty() { None } else { Some(copy_of(variant)) }, member: copy_of(member), reason }
}

/// Reads one object without its newline, bounded by its u32 wire length.
/// Nesting is at most three, numbers twenty bytes, strings at most the line length.
/// Use `decode_with_limits` to also admit against the producing domain's limits.
pub fn decode(line: &[u8]) -> Result<Recorded, Refusal> {
    let size = u32::try_from(line.len()).or(Err(refuse(b"", b"observation", Reason::Limit)))?;
    // Every payload and list is bounded by the already present line. These
    // envelope limits admit any domain's representable line without guessing
    // the scenario's configuration; a configured reader supplies its limits.
    let limits = Limits {
        repositories: 0,
        users: size,
        bypassing_users: 0,
        labels: size,
        items: 0,
        comments: 0,
        dependencies: size,
        reviews: 0,
        branches: 0,
        commits: 0,
        files: 0,
        statuses: 0,
        contexts: 0,
        pages: 0,
        name_bytes: size,
        title_bytes: size,
        body_bytes: size,
        content_bytes: size,
        page_size: 0,
        calls: 0,
        hooks: 0,
        observations: 0,
    };
    decode_with_limits(line, &limits)
}

/// Reads with domain-sized bounds: depth 3, number 20, and at most
/// 64 + 2*labels + dependencies + users tokens, capped by the wire length.
/// Wire bytes are bounded by `1024 + 6*(4*name_bytes + labels*(name_bytes+3))`
/// plus six times the three base64 payload bounds and `22*(dependencies+users)`.
/// Strings are bounded by the largest name, base64 payload or key.
pub fn decode_with_limits(line: &[u8], limits: &Limits) -> Result<Recorded, Refusal> {
    let size = u32::try_from(line.len()).or(Err(refuse(b"", b"observation", Reason::Limit)))?;
    let names = u64::from(limits.name_bytes)
        .saturating_mul(4)
        .saturating_add(u64::from(limits.labels).saturating_mul(u64::from(limits.name_bytes).saturating_add(3)));
    let mut length = 1024_u64
        .saturating_add(names.saturating_mul(6))
        .saturating_add(u64::from(limits.dependencies).saturating_add(u64::from(limits.users)).saturating_mul(22));
    let mut string = limits.name_bytes.max(32);
    for bytes in [limits.title_bytes, limits.body_bytes, limits.content_bytes] {
        let encoded = u64::from(bytes).div_ceil(3).saturating_mul(4);
        string = string.max(u32::try_from(encoded).unwrap_or(u32::MAX));
        length = length.saturating_add(encoded.saturating_mul(6));
    }
    if u64::from(size) > length {
        return Err(refuse(b"", b"observation", Reason::Limit));
    }
    let count = 64_u64
        .saturating_add(u64::from(limits.labels).saturating_mul(2))
        .saturating_add(u64::from(limits.dependencies))
        .saturating_add(u64::from(limits.users));
    let tokenizer = json::Limits { depth: 3, number: 20, string: string.min(size), chunk: 1, length: size };
    let (tokens, known) =
        tokenize(line, tokenizer, u32::try_from(count.min(u64::from(size))).expect("bounded by wire"))?;
    let root = object(&tokens, 0, &known, b"observation")?;
    if let Some(at) = root.find(&tokens, b"observations_lost") {
        let variant = tag(&tokens, &root);
        check_members(&tokens, &root, &[b"observations_lost"], variant)?;
        return Ok(Recorded::Lost(number(&tokens, at, b"observations_lost", b"")?));
    }
    let at = root.at(&tokens, b"observation", b"")?;
    let variant = text(&tokens, at, b"observation", b"")?;
    let observation = observation(&tokens, &root, variant, limits)?;
    let forward = match &observation {
        Observation::Moved { .. } => boolean(&tokens, root.at(&tokens, b"forward", variant)?, b"forward", variant)?,
        Observation::Deleted { .. }
        | Observation::Opened { .. }
        | Observation::Closed { .. }
        | Observation::Reopened { .. }
        | Observation::Labelled { .. }
        | Observation::Revised { .. }
        | Observation::Depends { .. }
        | Observation::Requested { .. }
        | Observation::Defined { .. }
        | Observation::Commented { .. }
        | Observation::Edited { .. }
        | Observation::Removed { .. }
        | Observation::Reviewed { .. }
        | Observation::Reported { .. }
        | Observation::Merged { .. }
        | Observation::Enqueued { .. }
        | Observation::Group { .. }
        | Observation::Dequeued { .. }
        | Observation::Refused { .. }
        | Observation::Rejected { .. }
        | Observation::Wiki { .. } => None,
    };
    Ok(Recorded::Observed { observation, forward })
}

fn tokenize(line: &[u8], limits: json::Limits, count: u32) -> Result<(List<Token>, Box<[u8]>), Refusal> {
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut machine = Tokenizer::new(&limits);
    let mut input = Held::new(copy_of(line));
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut tokens = List::with_capacity(count);
    let mut member = copy_of(b"observation");
    let mut variant = copy_of(b"");
    let mut depth = 0_u32;
    json::down(&mut machine, &env, json::Request::Next, &mut above, &mut below);
    for _ in 0..line.len().saturating_mul(6).saturating_add(32) {
        match above.pop() {
            Some(json::Event::Token(token)) => {
                if let Token::Key(key) = &token {
                    member.clone_from(key);
                }
                if depth == 1
                    && member.as_ref() == b"observation"
                    && let Token::String(name) = &token
                {
                    variant.clone_from(name);
                }
                match &token {
                    Token::ObjectStart | Token::ArrayStart => depth = depth.saturating_add(1),
                    Token::ObjectEnd | Token::ArrayEnd => depth = depth.saturating_sub(1),
                    Token::Key(_) | Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
                }
                tokens.push(token).or(Err(refuse(&variant, &member, Reason::Limit)))?;
                json::down(&mut machine, &env, json::Request::Next, &mut above, &mut below);
            }
            Some(json::Event::Done) => return Ok((tokens, variant)),
            Some(json::Event::Failed(_)) => return Err(refuse(&variant, &member, Reason::Json)),
            Some(json::Event::Long(_) | json::Event::Skipped(_) | json::Event::Closed) => {
                return Err(refuse(&variant, &member, Reason::Json));
            }
            None => {
                let Some(Down::Demand { read, .. }) = below.pop() else {
                    return Err(refuse(&variant, &member, Reason::Json));
                };
                let delivery = input.answer(read).ok_or(refuse(&variant, &member, Reason::Json))?;
                json::up(&mut machine, &env, delivery, &mut above, &mut below);
            }
        }
    }
    Err(refuse(&variant, &member, Reason::Limit))
}

#[derive(Debug)]
struct Object {
    members: List<(u32, u32)>,
}
impl Object {
    fn find(&self, tokens: &List<Token>, name: &[u8]) -> Option<u32> {
        for (key, value) in &self.members {
            if let Some(Token::Key(bytes)) = tokens.get(*key)
                && bytes.as_ref() == name
            {
                return Some(*value);
            }
        }
        None
    }
    fn at(&self, tokens: &List<Token>, name: &[u8], variant: &[u8]) -> Result<u32, Refusal> {
        self.find(tokens, name).ok_or(refuse(variant, name, Reason::Missing))
    }
}
fn tag<'a>(tokens: &'a List<Token>, root: &Object) -> &'a [u8] {
    if let Some(at) = root.find(tokens, b"observation")
        && let Some(Token::String(name)) = tokens.get(at)
    {
        return name;
    }
    b""
}
fn object(tokens: &List<Token>, start: u32, variant: &[u8], member: &[u8]) -> Result<Object, Refusal> {
    if tokens.get(start) != Some(&Token::ObjectStart) {
        return Err(refuse(variant, member, Reason::Type));
    }
    let mut result = Object { members: List::with_capacity(12) };
    let mut at = start.saturating_add(1);
    for _ in 0..tokens.len() {
        match tokens.get(at) {
            Some(Token::ObjectEnd) => return Ok(result),
            Some(Token::Key(key)) => {
                let value = at.saturating_add(1);
                // Retain repeats so the schema check can name them with the
                // tag even when observation follows the repeated member.
                result.members.push((at, value)).or(Err(refuse(variant, key, Reason::Limit)))?;
                at = after(tokens, value, variant, key)?;
            }
            Some(
                Token::ObjectStart
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            )
            | None => return Err(refuse(variant, member, Reason::Type)),
        }
    }
    Err(refuse(variant, member, Reason::Json))
}
fn after(tokens: &List<Token>, at: u32, variant: &[u8], member: &[u8]) -> Result<u32, Refusal> {
    let mut depth = 0_u32;
    for index in at..tokens.len() {
        match tokens.get(index) {
            Some(Token::ObjectStart | Token::ArrayStart) => depth = depth.saturating_add(1),
            Some(Token::ObjectEnd | Token::ArrayEnd) => depth = depth.saturating_sub(1),
            Some(Token::Key(_) | Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null) => {}
            None => return Err(refuse(variant, member, Reason::Json)),
        }
        if depth == 0 {
            return Ok(index.saturating_add(1));
        }
    }
    Err(refuse(variant, member, Reason::Json))
}
fn check_members(tokens: &List<Token>, object: &Object, expected: &[&[u8]], variant: &[u8]) -> Result<(), Refusal> {
    let mut seen = List::with_capacity(12);
    for (key, _value) in &object.members {
        let Some(Token::Key(name)) = tokens.get(*key) else {
            unreachable!("object contains key positions");
        };
        if !expected.contains(&name.as_ref()) {
            return Err(refuse(variant, name, Reason::Unknown));
        }
        for previous in &seen {
            let Some(Token::Key(previous)) = tokens.get(*previous) else {
                unreachable!("seen keys");
            };
            if previous == name {
                return Err(refuse(variant, name, Reason::Repeated));
            }
        }
        seen.push(*key).expect("object's member bound");
    }
    for name in expected {
        object.at(tokens, name, variant)?;
    }
    Ok(())
}
fn text<'a>(tokens: &'a List<Token>, at: u32, member: &[u8], variant: &[u8]) -> Result<&'a [u8], Refusal> {
    if let Some(Token::String(bytes)) = tokens.get(at) { Ok(bytes) } else { Err(refuse(variant, member, Reason::Type)) }
}
fn number(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8]) -> Result<u64, Refusal> {
    let Some(Token::Number(bytes)) = tokens.get(at) else {
        return Err(refuse(variant, member, Reason::Type));
    };
    let mut value = 0_u64;
    for digit in bytes {
        if !digit.is_ascii_digit() {
            return Err(refuse(variant, member, Reason::Value));
        }
        let shifted = value.checked_mul(10).ok_or(refuse(variant, member, Reason::Value))?;
        value =
            shifted.checked_add(u64::from(digit.saturating_sub(b'0'))).ok_or(refuse(variant, member, Reason::Value))?;
    }
    Ok(value)
}
fn position(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8]) -> Result<u32, Refusal> {
    u32::try_from(number(tokens, at, member, variant)?).or(Err(refuse(variant, member, Reason::Value)))
}
fn optional_number(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8]) -> Result<Option<u64>, Refusal> {
    if tokens.get(at) == Some(&Token::Null) { Ok(None) } else { Ok(Some(number(tokens, at, member, variant)?)) }
}
fn boolean(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8]) -> Result<Option<bool>, Refusal> {
    match tokens.get(at) {
        Some(Token::True) => Ok(Some(true)),
        Some(Token::False) => Ok(Some(false)),
        Some(Token::Null) => Ok(None),
        Some(
            Token::ObjectStart
            | Token::ObjectEnd
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::Key(_)
            | Token::String(_)
            | Token::Number(_),
        )
        | None => Err(refuse(variant, member, Reason::Type)),
    }
}
fn content(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8], max: u32) -> Result<Box<[u8]>, Refusal> {
    base64::decode(base64::Alphabet::Standard, text(tokens, at, member, variant)?, max).or(Err(refuse(
        variant,
        member,
        Reason::Value,
    )))
}
fn optional_content(
    tokens: &List<Token>,
    at: u32,
    member: &[u8],
    variant: &[u8],
    max: u32,
) -> Result<Option<Box<[u8]>>, Refusal> {
    if tokens.get(at) == Some(&Token::Null) { Ok(None) } else { Ok(Some(content(tokens, at, member, variant, max)?)) }
}
fn branches(tokens: &List<Token>, at: u32, variant: &[u8], max: u32) -> Result<Option<Branches>, Refusal> {
    if tokens.get(at) == Some(&Token::Null) {
        return Ok(None);
    }
    let object = object(tokens, at, variant, b"branches")?;
    check_members(tokens, &object, &[b"head", b"base", b"commit"], variant)?;
    Ok(Some(Branches {
        head: name(tokens, object.at(tokens, b"head", variant)?, b"head", variant, max)?,
        base: name(tokens, object.at(tokens, b"base", variant)?, b"base", variant, max)?,
        commit: number(tokens, object.at(tokens, b"commit", variant)?, b"commit", variant)?,
    }))
}
fn names(
    tokens: &List<Token>,
    at: u32,
    member: &[u8],
    variant: &[u8],
    limits: &Limits,
) -> Result<Box<[Box<[u8]>]>, Refusal> {
    if tokens.get(at) != Some(&Token::ArrayStart) {
        return Err(refuse(variant, member, Reason::Type));
    }
    let mut result = List::with_capacity(limits.labels.min(tokens.len()));
    for index in at.saturating_add(1)..tokens.len() {
        if tokens.get(index) == Some(&Token::ArrayEnd) {
            return Ok(result.into_boxed());
        }
        result.push(name(tokens, index, member, variant, limits.name_bytes)?).or(Err(refuse(
            variant,
            member,
            Reason::Limit,
        )))?;
    }
    Err(refuse(variant, member, Reason::Json))
}
fn numbers(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8], max: u32) -> Result<Box<[u64]>, Refusal> {
    if tokens.get(at) != Some(&Token::ArrayStart) {
        return Err(refuse(variant, member, Reason::Type));
    }
    let mut result = List::with_capacity(max.min(tokens.len()));
    for index in at.saturating_add(1)..tokens.len() {
        if tokens.get(index) == Some(&Token::ArrayEnd) {
            return Ok(result.into_boxed());
        }
        result.push(number(tokens, index, member, variant)?).or(Err(refuse(variant, member, Reason::Limit)))?;
    }
    Err(refuse(variant, member, Reason::Json))
}

#[expect(clippy::too_many_lines, reason = "one schema and construction match covers the domain observation vocabulary")]
fn observation(tokens: &List<Token>, root: &Object, variant: &[u8], limits: &Limits) -> Result<Observation, Refusal> {
    let fields: &[&[u8]] = match variant {
        b"moved" => &[b"observation".as_slice(), b"repository", b"branch", b"from", b"to", b"by", b"forward"],
        b"deleted" => &[b"observation".as_slice(), b"repository", b"branch", b"at", b"by"],
        b"opened" => &[
            b"observation".as_slice(),
            b"repository",
            b"number",
            b"kind",
            b"title",
            b"body",
            b"labels",
            b"branches",
            b"by",
        ],
        b"closed" | b"reopened" => &[b"observation".as_slice(), b"repository", b"number", b"by"],
        b"labelled" => &[b"observation".as_slice(), b"repository", b"number", b"labels", b"by"],
        b"revised" => &[b"observation".as_slice(), b"repository", b"number", b"title", b"body", b"by"],
        b"depends" => &[b"observation".as_slice(), b"repository", b"number", b"dependencies", b"by"],
        b"requested" => &[b"observation".as_slice(), b"repository", b"number", b"reviewers", b"by"],
        b"defined" => &[b"observation".as_slice(), b"repository", b"label", b"by"],
        b"commented" | b"edited" => &[b"observation".as_slice(), b"repository", b"number", b"id", b"body", b"by"],
        b"removed" => &[b"observation".as_slice(), b"repository", b"number", b"id", b"by"],
        b"reviewed" => {
            &[b"observation".as_slice(), b"repository", b"number", b"id", b"commit", b"verdict", b"body", b"by"]
        }
        b"reported" => &[b"observation".as_slice(), b"repository", b"commit", b"context", b"state", b"by"],
        b"merged" => &[b"observation".as_slice(), b"repository", b"number", b"base", b"head", b"commit", b"by"],
        b"enqueued" => &[b"observation".as_slice(), b"repository", b"number", b"head", b"position", b"by"],
        b"group" => &[b"observation".as_slice(), b"repository", b"number", b"commit", b"ended", b"landed"],
        b"dequeued" => &[b"observation".as_slice(), b"repository", b"number", b"reason"],
        b"refused" => &[b"observation".as_slice(), b"repository", b"what", b"number", b"commit", b"error", b"by"],
        b"rejected" => &[b"observation".as_slice(), b"repository", b"branch", b"commit", b"by"],
        b"wiki" => &[b"observation".as_slice(), b"repository", b"name", b"content", b"revision", b"by"],
        _ => return Err(refuse(variant, b"observation", Reason::Value)),
    };
    check_members(tokens, root, fields, variant)?;
    Ok(match variant {
        b"moved" => Observation::Moved {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            branch: name(tokens, root.at(tokens, b"branch", variant)?, b"branch", variant, limits.name_bytes)?,
            from: optional_number(tokens, root.at(tokens, b"from", variant)?, b"from", variant)?,
            to: number(tokens, root.at(tokens, b"to", variant)?, b"to", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"deleted" => Observation::Deleted {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            branch: name(tokens, root.at(tokens, b"branch", variant)?, b"branch", variant, limits.name_bytes)?,
            at: number(tokens, root.at(tokens, b"at", variant)?, b"at", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"opened" => Observation::Opened {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            kind: kind(text(tokens, root.at(tokens, b"kind", variant)?, b"kind", variant)?).ok_or(refuse(
                variant,
                b"kind",
                Reason::Value,
            ))?,
            title: content(tokens, root.at(tokens, b"title", variant)?, b"title", variant, limits.title_bytes)?,
            body: content(tokens, root.at(tokens, b"body", variant)?, b"body", variant, limits.body_bytes)?,
            labels: names(tokens, root.at(tokens, b"labels", variant)?, b"labels", variant, limits)?,
            branches: branches(tokens, root.at(tokens, b"branches", variant)?, variant, limits.name_bytes)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"closed" => Observation::Closed {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"reopened" => Observation::Reopened {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"labelled" => Observation::Labelled {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            labels: names(tokens, root.at(tokens, b"labels", variant)?, b"labels", variant, limits)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"revised" => Observation::Revised {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            title: content(tokens, root.at(tokens, b"title", variant)?, b"title", variant, limits.title_bytes)?,
            body: content(tokens, root.at(tokens, b"body", variant)?, b"body", variant, limits.body_bytes)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"depends" => Observation::Depends {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            dependencies: numbers(
                tokens,
                root.at(tokens, b"dependencies", variant)?,
                b"dependencies",
                variant,
                limits.dependencies,
            )?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"requested" => Observation::Requested {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            reviewers: numbers(tokens, root.at(tokens, b"reviewers", variant)?, b"reviewers", variant, limits.users)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"defined" => Observation::Defined {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            label: name(tokens, root.at(tokens, b"label", variant)?, b"label", variant, limits.name_bytes)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"commented" => Observation::Commented {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            id: number(tokens, root.at(tokens, b"id", variant)?, b"id", variant)?,
            body: content(tokens, root.at(tokens, b"body", variant)?, b"body", variant, limits.body_bytes)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"edited" => Observation::Edited {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            id: number(tokens, root.at(tokens, b"id", variant)?, b"id", variant)?,
            body: content(tokens, root.at(tokens, b"body", variant)?, b"body", variant, limits.body_bytes)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"removed" => Observation::Removed {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            id: number(tokens, root.at(tokens, b"id", variant)?, b"id", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"reviewed" => Observation::Reviewed {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            id: number(tokens, root.at(tokens, b"id", variant)?, b"id", variant)?,
            commit: number(tokens, root.at(tokens, b"commit", variant)?, b"commit", variant)?,
            verdict: verdict(text(tokens, root.at(tokens, b"verdict", variant)?, b"verdict", variant)?)
                .ok_or(refuse(variant, b"verdict", Reason::Value))?,
            body: content(tokens, root.at(tokens, b"body", variant)?, b"body", variant, limits.body_bytes)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"reported" => Observation::Reported {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            commit: number(tokens, root.at(tokens, b"commit", variant)?, b"commit", variant)?,
            context: name(tokens, root.at(tokens, b"context", variant)?, b"context", variant, limits.name_bytes)?,
            state: check(text(tokens, root.at(tokens, b"state", variant)?, b"state", variant)?).ok_or(refuse(
                variant,
                b"state",
                Reason::Value,
            ))?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"merged" => Observation::Merged {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            base: name(tokens, root.at(tokens, b"base", variant)?, b"base", variant, limits.name_bytes)?,
            head: number(tokens, root.at(tokens, b"head", variant)?, b"head", variant)?,
            commit: number(tokens, root.at(tokens, b"commit", variant)?, b"commit", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"enqueued" => Observation::Enqueued {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            head: number(tokens, root.at(tokens, b"head", variant)?, b"head", variant)?,
            position: position(tokens, root.at(tokens, b"position", variant)?, b"position", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"group" => Observation::Group {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            commit: number(tokens, root.at(tokens, b"commit", variant)?, b"commit", variant)?,
            ended: boolean(tokens, root.at(tokens, b"ended", variant)?, b"ended", variant)?.ok_or(refuse(
                variant,
                b"ended",
                Reason::Type,
            ))?,
            landed: boolean(tokens, root.at(tokens, b"landed", variant)?, b"landed", variant)?.ok_or(refuse(
                variant,
                b"landed",
                Reason::Type,
            ))?,
        },
        b"dequeued" => Observation::Dequeued {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            number: number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            reason: queue_removal(text(tokens, root.at(tokens, b"reason", variant)?, b"reason", variant)?)
                .ok_or(refuse(variant, b"reason", Reason::Value))?,
        },
        b"refused" => Observation::Refused {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            what: operation(text(tokens, root.at(tokens, b"what", variant)?, b"what", variant)?).ok_or(refuse(
                variant,
                b"what",
                Reason::Value,
            ))?,
            number: optional_number(tokens, root.at(tokens, b"number", variant)?, b"number", variant)?,
            commit: optional_number(tokens, root.at(tokens, b"commit", variant)?, b"commit", variant)?,
            error: error(tokens, root.at(tokens, b"error", variant)?, variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"rejected" => Observation::Rejected {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            branch: name(tokens, root.at(tokens, b"branch", variant)?, b"branch", variant, limits.name_bytes)?,
            commit: number(tokens, root.at(tokens, b"commit", variant)?, b"commit", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        b"wiki" => Observation::Wiki {
            repository: name(
                tokens,
                root.at(tokens, b"repository", variant)?,
                b"repository",
                variant,
                limits.name_bytes,
            )?,
            name: name(tokens, root.at(tokens, b"name", variant)?, b"name", variant, limits.name_bytes)?,
            content: optional_content(
                tokens,
                root.at(tokens, b"content", variant)?,
                b"content",
                variant,
                limits.content_bytes,
            )?,
            revision: number(tokens, root.at(tokens, b"revision", variant)?, b"revision", variant)?,
            by: number(tokens, root.at(tokens, b"by", variant)?, b"by", variant)?,
        },
        _ => return Err(refuse(variant, b"observation", Reason::Value)),
    })
}
fn operation(value: &[u8]) -> Option<Operation> {
    Some(match value {
        b"create_issue" => Operation::CreateIssue,
        b"set_protection" => Operation::SetProtection,
        b"edit_item" => Operation::EditItem,
        b"set_dependencies" => Operation::SetDependencies,
        b"comment" => Operation::Comment,
        b"edit_comment" => Operation::EditComment,
        b"delete_comment" => Operation::DeleteComment,
        b"set_labels" => Operation::SetLabels,
        b"add_labels" => Operation::AddLabels,
        b"remove_labels" => Operation::RemoveLabels,
        b"define_label" => Operation::DefineLabel,
        b"open_pull" => Operation::OpenPull,
        b"set_reviewers" => Operation::SetReviewers,
        b"review" => Operation::Review,
        b"submit" => Operation::Submit,
        b"merge" => Operation::Merge,
        b"enqueue" => Operation::Enqueue,
        b"update" => Operation::Update,
        b"close" => Operation::Close,
        b"reopen" => Operation::Reopen,
        b"delete_branch" => Operation::DeleteBranch,
        b"status" => Operation::Status,
        b"put_page" => Operation::PutPage,
        b"delete_page" => Operation::DeletePage,
        b"clone" => Operation::Clone,
        b"fetch" => Operation::Fetch,
        b"push" => Operation::Push,
        b"create" => Operation::Create,
        _ => return None,
    })
}
fn kind(value: &[u8]) -> Option<a::Kind> {
    Some(match value {
        b"issue" => a::Kind::Issue,
        b"pull" => a::Kind::Pull,
        _ => return None,
    })
}
fn verdict(value: &[u8]) -> Option<a::Verdict> {
    Some(match value {
        b"approve" => a::Verdict::Approve,
        b"request_changes" => a::Verdict::RequestChanges,
        b"comment" => a::Verdict::Comment,
        _ => return None,
    })
}
fn check(value: &[u8]) -> Option<a::Check> {
    Some(match value {
        b"pending" => a::Check::Pending,
        b"skipped" => a::Check::Skipped,
        b"passed" => a::Check::Passed,
        b"failed" => a::Check::Failed,
        _ => return None,
    })
}
fn queue_removal(value: &[u8]) -> Option<a::QueueRemoval> {
    Some(match value {
        b"merge_conflict" => a::QueueRemoval::MergeConflict,
        b"ci_failure" => a::QueueRemoval::CiFailure,
        _ => return None,
    })
}
fn what(value: &[u8]) -> Option<a::What> {
    Some(match value {
        b"repository" => a::What::Repository,
        b"item" => a::What::Item,
        b"pull" => a::What::Pull,
        b"comment" => a::What::Comment,
        b"label" => a::What::Label,
        b"branch" => a::What::Branch,
        b"commit" => a::What::Commit,
        b"file" => a::What::File,
        b"page" => a::What::Page,
        b"review" => a::What::Review,
        b"job" => a::What::Job,
        _ => return None,
    })
}
fn error(tokens: &List<Token>, at: u32, variant: &[u8]) -> Result<a::Error, Refusal> {
    let object = object(tokens, at, variant, b"error")?;
    let name = text(tokens, object.at(tokens, b"name", variant)?, b"name", variant)?;
    let fields: &[&[u8]] = match name {
        b"rate_limited" => &[b"name", b"reset"],
        b"missing" => &[b"name", b"what"],
        b"already_queued" => &[b"name", b"number", b"head", b"position"],
        b"unavailable" | b"timeout" | b"forbidden" | b"too_large" | b"full" | b"exists" | b"circular"
        | b"nothing_to_merge" | b"empty" | b"closed" | b"stale" | b"conflict" | b"protected" | b"unreachable"
        | b"refused" => &[b"name"],
        _ => return Err(refuse(variant, b"name", Reason::Value)),
    };
    check_members(tokens, &object, fields, variant)?;
    Ok(match name {
        b"unavailable" => a::Error::Unavailable,
        b"timeout" => a::Error::Timeout,
        b"rate_limited" => a::Error::RateLimited {
            reset: Time::from_nanos(number(tokens, object.at(tokens, b"reset", variant)?, b"reset", variant)?),
        },
        b"forbidden" => a::Error::Forbidden,
        b"missing" => a::Error::Missing(
            what(text(tokens, object.at(tokens, b"what", variant)?, b"what", variant)?).ok_or(refuse(
                variant,
                b"what",
                Reason::Value,
            ))?,
        ),
        b"too_large" => a::Error::TooLarge,
        b"full" => a::Error::Full,
        b"exists" => a::Error::Exists,
        b"already_queued" => a::Error::AlreadyQueued(a::QueueEntry {
            number: number(tokens, object.at(tokens, b"number", variant)?, b"number", variant)?,
            head: number(tokens, object.at(tokens, b"head", variant)?, b"head", variant)?,
            position: position(tokens, object.at(tokens, b"position", variant)?, b"position", variant)?,
        }),
        b"circular" => a::Error::Circular,
        b"nothing_to_merge" => a::Error::NothingToMerge,
        b"empty" => a::Error::Empty,
        b"closed" => a::Error::Closed,
        b"stale" => a::Error::Stale,
        b"conflict" => a::Error::Conflict,
        b"protected" => a::Error::Protected,
        b"unreachable" => a::Error::Unreachable,
        b"refused" => a::Error::Refused,
        _ => return Err(refuse(variant, b"name", Reason::Value)),
    })
}

fn name(tokens: &List<Token>, at: u32, member: &[u8], variant: &[u8], max: u32) -> Result<Box<[u8]>, Refusal> {
    let bytes = text(tokens, at, member, variant)?;
    if bytes.len() > usize::try_from(max).expect("u32 fits usize") {
        return Err(refuse(variant, member, Reason::Limit));
    }
    Ok(copy_of(bytes))
}
