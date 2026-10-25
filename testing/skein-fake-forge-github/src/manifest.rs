//! The browser's bounded manifest record (fake-forge.md, 10.1).
use alloc::boxed::Box;
use skein_forge::github::limits::Limits;
use skein_json::{Token, tokenizer};
use skein_lib::{
    Env, Intake, List, Queue, Time, Wall, bytes,
    stream::{Down, Up},
};

use skein_forge::github::document::Access;
pub use skein_forge::github::document::Permission;
/// The hook attributes posted by the browser.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hook {
    pub url: Option<Box<[u8]>>,
    pub active: bool,
}
/// The manifest kept by state; no conversion code or App secrets live here.
#[derive(Debug)]
pub struct Manifest {
    pub name: Box<[u8]>,
    pub url: Box<[u8]>,
    pub public: bool,
    pub redirect_url: Box<[u8]>,
    pub setup_url: Option<Box<[u8]>>,
    pub setup_on_update: bool,
    pub hook_attributes: Option<Hook>,
    pub default_permissions: List<Permission>,
    pub default_events: List<Box<[u8]>>,
    pub account: Box<[u8]>,
}

pub(crate) fn read(input: &[u8], account: &[u8], limits: &Limits) -> Option<Manifest> {
    let tokens = tokenize(input, limits)?;
    validate(&tokens, limits)?;
    let mut at = 0;
    if take(&tokens, &mut at)? != &Token::ObjectStart {
        return None;
    }
    let mut name = None;
    let mut url = None;
    let mut redirect_url = None;
    let mut manifest = Manifest {
        name: Box::from([]),
        url: Box::from([]),
        public: false,
        redirect_url: Box::from([]),
        setup_url: None,
        setup_on_update: false,
        hook_attributes: None,
        default_permissions: List::with_capacity(limits.inner),
        default_events: List::with_capacity(limits.inner),
        account: bytes::copy_of(account),
    };
    for _field in 0..tokens.len() {
        let key = match take(&tokens, &mut at)? {
            Token::Key(key) => key,
            Token::ObjectEnd => break,
            Token::ObjectStart
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => return None,
        };
        match key.as_ref() {
            b"name" => name = Some(text(take(&tokens, &mut at)?)?),
            b"url" => url = Some(text(take(&tokens, &mut at)?)?),
            b"redirect_url" => redirect_url = Some(text(take(&tokens, &mut at)?)?),
            b"setup_url" => manifest.setup_url = Some(text(take(&tokens, &mut at)?)?),
            b"public" => manifest.public = boolean(take(&tokens, &mut at)?)?,
            b"setup_on_update" => manifest.setup_on_update = boolean(take(&tokens, &mut at)?)?,
            b"hook_attributes" => manifest.hook_attributes = Some(hook(&tokens, &mut at)?),
            b"default_permissions" => manifest.default_permissions = permissions(&tokens, &mut at, limits.inner)?,
            b"default_events" => manifest.default_events = events(&tokens, &mut at, limits.inner)?,
            _ => skip(&tokens, &mut at)?,
        }
    }
    manifest.name = name?;
    manifest.url = url?;
    manifest.redirect_url = redirect_url?;
    if at != tokens.len() || !absolute(&manifest.redirect_url) {
        return None;
    }
    if let Some(url) = &manifest.setup_url
        && !absolute(url)
    {
        return None;
    }
    Some(manifest)
}

fn take<'a>(tokens: &'a List<Token>, at: &mut u32) -> Option<&'a Token> {
    let token = tokens.get(*at)?;
    *at = at.checked_add(1)?;
    Some(token)
}
fn text(token: &Token) -> Option<Box<[u8]>> {
    match token {
        Token::String(value) => Some(value.clone()),
        Token::Key(_)
        | Token::Number(_)
        | Token::ObjectStart
        | Token::ObjectEnd
        | Token::ArrayStart
        | Token::ArrayEnd
        | Token::True
        | Token::False
        | Token::Null => None,
    }
}
fn boolean(token: &Token) -> Option<bool> {
    match token {
        Token::True => Some(true),
        Token::False => Some(false),
        Token::Key(_)
        | Token::String(_)
        | Token::Number(_)
        | Token::ObjectStart
        | Token::ObjectEnd
        | Token::ArrayStart
        | Token::ArrayEnd
        | Token::Null => None,
    }
}
fn hook(tokens: &List<Token>, at: &mut u32) -> Option<Hook> {
    if take(tokens, at)? != &Token::ObjectStart {
        return None;
    }
    let mut hook = Hook { url: None, active: false };
    for _field in 0..tokens.len() {
        match take(tokens, at)? {
            Token::Key(key) => match key.as_ref() {
                b"url" => hook.url = Some(text(take(tokens, at)?)?),
                b"active" => hook.active = boolean(take(tokens, at)?)?,
                _ => skip(tokens, at)?,
            },
            Token::ObjectEnd => return Some(hook),
            Token::ObjectStart
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => return None,
        }
    }
    None
}
fn permissions(tokens: &List<Token>, at: &mut u32, bound: u32) -> Option<List<Permission>> {
    if take(tokens, at)? != &Token::ObjectStart {
        return None;
    }
    let mut kept = List::with_capacity(bound);
    for _field in 0..tokens.len() {
        match take(tokens, at)? {
            Token::Key(key) => {
                let access = Access::read(&text(take(tokens, at)?)?)?;
                kept.push(Permission { name: key.clone(), access }).ok()?;
            }
            Token::ObjectEnd => return Some(kept),
            Token::ObjectStart
            | Token::ArrayStart
            | Token::ArrayEnd
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null => return None,
        }
    }
    None
}
fn events(tokens: &List<Token>, at: &mut u32, bound: u32) -> Option<List<Box<[u8]>>> {
    if take(tokens, at)? != &Token::ArrayStart {
        return None;
    }
    let mut kept = List::with_capacity(bound);
    for _event in 0..tokens.len() {
        let token = take(tokens, at)?;
        if token == &Token::ArrayEnd {
            return Some(kept);
        }
        kept.push(text(token)?).ok()?;
    }
    None
}
fn skip(tokens: &List<Token>, at: &mut u32) -> Option<()> {
    let mut depth = 0_u32;
    for _token in 0..tokens.len() {
        match take(tokens, at)? {
            Token::ObjectStart | Token::ArrayStart => depth = depth.checked_add(1)?,
            Token::ObjectEnd | Token::ArrayEnd => depth = depth.checked_sub(1)?,
            Token::Key(_) | Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
        }
        if depth == 0 {
            return Some(());
        }
    }
    None
}
// A nonempty authority and URI bytes are required; return addresses carry no query or fragment.
pub(crate) fn absolute(url: &[u8]) -> bool {
    let Some(rest) = url.strip_prefix(b"https://").or(url.strip_prefix(b"http://")) else {
        return false;
    };
    if rest.is_empty() || rest.starts_with(b"/") {
        return false;
    }
    for byte in url {
        if *byte <= b' ' || *byte == 127 || b"?#\\".contains(byte) {
            return false;
        }
    }
    true
}
#[derive(Debug)]
struct Container {
    array: bool,
    count: u32,
}
fn validate(tokens: &List<Token>, limits: &Limits) -> Option<()> {
    let mut stack: List<Container> = List::with_capacity(limits.json.depth);
    let string = usize::try_from(limits.string_bytes).ok()?;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Key(key) => {
                if key.len() > string {
                    return None;
                }
                // Walk only this object's earlier keys, skipping nested containers.
                let mut depth = 0_u32;
                for prior in tokens.as_slice().get(..index)?.iter().rev() {
                    match prior {
                        Token::ObjectEnd | Token::ArrayEnd => depth = depth.checked_add(1)?,
                        Token::ObjectStart | Token::ArrayStart => {
                            if depth == 0 {
                                break;
                            }
                            depth = depth.checked_sub(1)?;
                        }
                        Token::Key(prior) => {
                            if depth == 0 && prior == key {
                                return None;
                            }
                        }
                        Token::String(_) | Token::Number(_) | Token::True | Token::False | Token::Null => {}
                    }
                }
                continue;
            }
            Token::String(value) => {
                if value.len() > string {
                    return None;
                }
            }
            Token::ObjectEnd | Token::ArrayEnd => {
                stack.truncate(stack.len().checked_sub(1)?);
                continue;
            }
            Token::ObjectStart | Token::ArrayStart | Token::Number(_) | Token::True | Token::False | Token::Null => {}
        }
        if !stack.is_empty() {
            let last = stack.len().checked_sub(1)?;
            let parent = stack.get_mut(last)?;
            if parent.array {
                parent.count = parent.count.checked_add(1)?;
                if parent.count > limits.inner {
                    return None;
                }
            }
        }
        match token {
            Token::ObjectStart => stack.push(Container { array: false, count: 0 }).ok()?,
            Token::ArrayStart => stack.push(Container { array: true, count: 0 }).ok()?,
            Token::Key(_)
            | Token::String(_)
            | Token::Number(_)
            | Token::True
            | Token::False
            | Token::Null
            | Token::ObjectEnd
            | Token::ArrayEnd => {}
        }
    }
    Some(())
}
pub(crate) fn tokenize(input: &[u8], limits: &Limits) -> Option<List<Token>> {
    let bound = u32::try_from(input.len()).ok()?;
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut machine = tokenizer::Tokenizer::new(&limits.json);
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake = Intake::with_capacity(bound.max(tokenizer::largest_demand(&limits.json)));
    intake.append(input).ok()?;
    let mut tokens = List::with_capacity(bound);
    tokenizer::down(&mut machine, &env, tokenizer::Request::Next, &mut above, &mut below);
    let ticks = input.len().checked_mul(8)?.checked_add(64)?;
    for _tick in 0..ticks {
        match above.pop() {
            Some(tokenizer::Event::Token(token)) => {
                tokens.push(token).ok()?;
                tokenizer::down(&mut machine, &env, tokenizer::Request::Next, &mut above, &mut below);
            }
            Some(tokenizer::Event::Done) => return Some(tokens),
            Some(
                tokenizer::Event::Failed(_)
                | tokenizer::Event::Long(_)
                | tokenizer::Event::Skipped(_)
                | tokenizer::Event::Closed,
            ) => return None,
            None => {
                let Some(Down::Demand { read, room: 0 }) = below.pop() else {
                    return None;
                };
                let delivery = match intake.meet(read) {
                    Some(bytes) => Up::Bytes(bytes),
                    None => Up::End,
                };
                tokenizer::up(&mut machine, &env, delivery, &mut above, &mut below);
            }
        }
    }
    None
}

pub(crate) fn worst_case(limits: &Limits, manifest_bytes: u32) -> Option<u64> {
    let strings = u64::from(limits.string_bytes);
    let lists =
        List::<Permission>::worst_case(limits.inner)?.checked_add(List::<Box<[u8]>>::worst_case(limits.inner)?)?;
    // Scalar fields, account, and two strings per permission/event pair, plus token scratch.
    lists
        .checked_add(strings.checked_mul(u64::from(limits.inner).checked_mul(2)?.checked_add(8)?)?)?
        .checked_add(List::<Token>::worst_case(manifest_bytes)?)?
        .checked_add(u64::from(manifest_bytes))
}
