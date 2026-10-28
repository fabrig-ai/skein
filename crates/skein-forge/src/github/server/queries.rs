//! Bounded GraphQL envelopes for the dialect's two byte-matched documents.
use super::{Limits, document};
use crate::forgejo::id::unsigned;
use alloc::boxed::Box;
use skein_json::{
    Token,
    tokenizer::{self, Event},
};
use skein_lib::{
    Env, Intake, List, Queue, Time, Wall,
    stream::{Down, Up},
};

/// The two query documents understood by the fake.
#[derive(Debug)]
pub enum Graphql {
    /// The authenticated viewer.
    Viewer,
    /// A pull's opaque identity, merge commit and queue state.
    Pull { owner: Box<[u8]>, name: Box<[u8]>, number: u64 },
}

/// Decodes only VIEWER and PULL; other query documents are unsupported.
/// JSON envelope whitespace and member order do not change query identity.
#[must_use]
pub fn graphql_body(body: &[u8], limits: &Limits) -> Option<Graphql> {
    let tokens = tokens(body, limits)?;
    let mut tokens = tokens.iter();
    if tokens.next()? != &Token::ObjectStart {
        return None;
    }
    let mut query = None;
    let mut variables = None;
    for _field in 0..2_u32 {
        if tokens.as_slice().first()? == &Token::ObjectEnd {
            break;
        }
        let Token::Key(key) = tokens.next()? else {
            return None;
        };
        match key.as_ref() {
            b"query" => {
                if query.is_some() {
                    return None;
                }
                let Token::String(text) = tokens.next()? else {
                    return None;
                };
                query = Some(text.as_ref());
            }
            b"variables" => {
                if variables.is_some() {
                    return None;
                }
                variables = Some(read_variables(&mut tokens, limits)?);
            }
            _ => return None,
        }
    }
    if tokens.next()? != &Token::ObjectEnd || tokens.next().is_some() {
        return None;
    }
    match query? {
        document::VIEWER if variables.is_none() => Some(Graphql::Viewer),
        document::PULL => {
            let Variables { owner, name, number } = variables?;
            Some(Graphql::Pull { owner, name, number })
        }
        _ => None,
    }
}
#[derive(Debug)]
struct Variables {
    owner: Box<[u8]>,
    name: Box<[u8]>,
    number: u64,
}
fn read_variables(tokens: &mut core::slice::Iter<'_, Token>, limits: &Limits) -> Option<Variables> {
    if tokens.next()? != &Token::ObjectStart {
        return None;
    }
    let mut owner = None;
    let mut name = None;
    let mut number = None;
    for _field in 0..3_u32 {
        let Token::Key(key) = tokens.next()? else {
            return None;
        };
        match key.as_ref() {
            b"owner" if owner.is_none() => owner = Some(string(tokens.next()?, limits)?),
            b"name" if name.is_none() => name = Some(string(tokens.next()?, limits)?),
            b"number" if number.is_none() => {
                let Token::Number(text) = tokens.next()? else {
                    return None;
                };
                number = Some(unsigned(text)?);
            }
            _ => return None,
        }
    }
    if tokens.next()? != &Token::ObjectEnd {
        return None;
    }
    Some(Variables { owner: owner?, name: name?, number: number? })
}
fn string(token: &Token, limits: &Limits) -> Option<Box<[u8]>> {
    let Token::String(text) = token else {
        return None;
    };
    if text.is_empty() || u64::try_from(text.len()).ok()? > u64::from(limits.string_bytes) {
        return None;
    }
    Some(text.clone())
}
fn tokens(body: &[u8], limits: &Limits) -> Option<List<Token>> {
    if u64::try_from(body.len()).ok()? > u64::from(limits.body_bytes) {
        return None;
    }
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut above = Queue::uncapped();
    let mut below = Queue::uncapped();
    let mut tokens = List::with_capacity(16);
    let mut intake =
        Intake::with_capacity(u32::try_from(body.len()).ok()?.max(tokenizer::largest_demand(&limits.json)));
    intake.append(body).ok()?;
    tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
    for _tick in 0..body.len().checked_mul(8)?.checked_add(64)? {
        if let Some(event) = above.pop() {
            match event {
                Event::Token(token) => tokens.push(token).ok()?,
                Event::Done => return Some(tokens),
                Event::Long(_) | Event::Skipped(_) | Event::Failed(_) | Event::Closed => return None,
            }
            tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
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
