//! Delivery fields over the shared bounded walker, with no retained JSON tree.
use super::super::{
    delivery::{self, Account, Change, Ci, Delivery, Event, Pull},
    id,
    limits::Limits,
};
use super::{
    Malformed,
    walk::{Move, Payload, Walk},
};
use crate::forgejo::document::Body;
use alloc::boxed::Box;
use skein_json::{Token, tokenizer};
use skein_lib::{
    Env, Queue, Time, Wall, bytes,
    stream::{Down, Read, Up},
};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step {
    Demand(tokenizer::Request),
    Done(Delivery),
    Malformed(Malformed),
}
/// Retains the event's fields and the walker's bounded stack.
#[derive(Debug)]
pub struct Decoder {
    walk: Walk,
    event: Event,
    repository: u64,
    owner: Option<Box<[u8]>>,
    name: Option<Box<[u8]>>,
    number: u64,
    body: Body,
    action: Box<[u8]>,
    reason: Option<Box<[u8]>>,
    reference: Box<[u8]>,
    branch: bool,
    commit: crate::api::Commit,
    deleted: bool,
    sender: u64,
    installation: u64,
    account: Account,
    terminal: bool,
}
/// Unknown event names are verified by the caller without parsing their bodies.
#[must_use]
pub fn decoder(event: &[u8], limits: &Limits) -> Option<Decoder> {
    let event = delivery::event(event)?;
    Some(Decoder {
        walk: Walk::delivery(event, limits),
        event,
        repository: 0,
        owner: None,
        name: None,
        number: 0,
        // GitHub's absent and null descriptions both mean an empty description.
        body: Body::Kept(Box::from([])),
        action: Box::from([]),
        reason: None,
        reference: Box::from([]),
        branch: false,
        commit: [0; 32],
        deleted: false,
        sender: 0,
        installation: 0,
        account: Account { login: Box::from([]), id: 0 },
        terminal: false,
    })
}
/// The first demand of a fresh decoder.
#[must_use]
pub fn first(_decoder: &Decoder) -> tokenizer::Request {
    tokenizer::Request::Next
}
/// Consumes one tokenizer event. Limits are captured when the decoder starts.
pub fn step(decoder: &mut Decoder, _limits: &Limits, event: &tokenizer::Event) -> Step {
    decoder.step(event.clone())
}
impl Decoder {
    /// Takes the repository path after a successful finish, when both names were present.
    pub fn path(&mut self) -> Option<delivery::Path> {
        if !self.terminal {
            return None;
        }
        match (self.owner.take(), self.name.take()) {
            (Some(owner), Some(name)) => Some(delivery::Path { owner, name }),
            (Some(_) | None, None) | (None, Some(_)) => None,
        }
    }
    /// Owns the event's bounded strings instead of copying them.
    pub fn step(&mut self, event: tokenizer::Event) -> Step {
        if self.terminal {
            return Step::Malformed(Malformed::Shape { field: b"" });
        }
        let result = self.walk.step(event);
        let result = match result {
            Ok(movement) => self.movement(movement),
            Err(error) => Err(error),
        };
        match result {
            Ok(step) => step,
            Err(error) => {
                self.terminal = true;
                Step::Malformed(error)
            }
        }
    }
    fn movement(&mut self, movement: Move) -> Result<Step, Malformed> {
        match movement {
            Move::Demand(demand) => return Ok(Step::Demand(demand)),
            Move::PayloadScalar(node, key, token) => self.scalar(node, key, token)?,
            Move::BodyLong(None, length) => self.body = Body::Long(length),
            Move::PayloadEnd => {}
            Move::PayloadStart(_) | Move::BodyLong(Some(_), _) | Move::Scalar(..) | Move::End(_) => {
                unreachable!("delivery walker root")
            }
            Move::Done => {
                self.terminal = true;
                return Ok(Step::Done(self.finish()?));
            }
        }
        Ok(Step::Demand(tokenizer::Request::Next))
    }
    fn scalar(&mut self, node: Payload, key: &'static [u8], token: Token) -> Result<(), Malformed> {
        match (node, key, token) {
            (Payload::Repository, b"id", Token::Number(text)) => self.repository = unsigned(&text),
            (Payload::Repository, b"name", Token::String(text)) => self.name = Some(text),
            (Payload::RepositoryOwner, b"login", Token::String(text)) => self.owner = Some(text),
            (Payload::Item | Payload::Number | Payload::Root(Event::Pull), b"number", Token::Number(text)) => {
                self.number = unsigned(&text);
            }
            (Payload::Item | Payload::Body, b"body", Token::String(text)) => self.body = Body::Kept(text),
            (Payload::Root(_), b"action", Token::String(text)) => self.action = text,
            (Payload::Root(Event::Pull), b"reason", Token::String(text)) => self.reason = Some(text),
            (Payload::Root(Event::Push | Event::Ref(_)), b"ref", Token::String(text)) => self.reference = text,
            (Payload::Root(Event::Ref(_)), b"ref_type", Token::String(text)) => {
                self.branch = text.as_ref() == b"branch";
            }
            (Payload::Head, b"head_sha", Token::String(text))
            | (Payload::Root(Event::Push), b"after", Token::String(text))
            | (Payload::Root(Event::Ci(Ci::Status)), b"sha", Token::String(text)) => {
                self.commit = id::commit(&text).ok_or(Malformed::Id { field: key })?;
            }
            (Payload::Root(Event::Ci(Ci::Status)), b"state", Token::String(_)) => {}
            (Payload::Root(Event::Push), b"deleted", token) => self.deleted = token == Token::True,
            (Payload::Sender, b"id", Token::Number(text)) => self.sender = unsigned(&text),
            (Payload::Installation | Payload::InstallationId, b"id", Token::Number(text)) => {
                self.installation = unsigned(&text);
            }
            (Payload::Account, b"id", Token::Number(text)) => self.account.id = unsigned(&text),
            (Payload::Account, b"login", Token::String(text)) => self.account.login = text,
            (
                Payload::Root(_)
                | Payload::Repository
                | Payload::RepositoryOwner
                | Payload::Item
                | Payload::Number
                | Payload::Body
                | Payload::Head
                | Payload::Sender
                | Payload::Installation
                | Payload::InstallationId
                | Payload::Account,
                _,
                Token::ObjectStart
                | Token::ObjectEnd
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::Key(_)
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            ) => unreachable!("delivery field table"),
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Delivery, Malformed> {
        let repository = self.repository;
        let number = self.number;
        let body = core::mem::replace(&mut self.body, Body::Kept(Box::from([])));
        Ok(match self.event {
            Event::Issue => Delivery::Issue { repository, number, body },
            Event::Comment => Delivery::Comment { repository, number, body },
            Event::Review => Delivery::Review { repository, number, body },
            Event::Remark => Delivery::Remark { repository, number },
            Event::Pull => {
                if !delivery::pull_action(&self.action) {
                    return Ok(Delivery::Other);
                }
                let reason = if self.action.as_ref() == b"dequeued" {
                    Some(self.reason.take().ok_or(Malformed::Missing { field: b"reason" })?)
                } else {
                    None
                };
                Delivery::Pull {
                    repository,
                    number,
                    action: Pull { action: core::mem::replace(&mut self.action, Box::from([])), reason },
                }
            }
            Event::Push => {
                let Some(branch) = self.reference.strip_prefix(b"refs/heads/") else {
                    return Ok(Delivery::Other);
                };
                Delivery::Push {
                    repository,
                    branch: bytes::copy_of(branch),
                    after: self.commit,
                    deleted: self.deleted,
                    sender: self.sender,
                }
            }
            Event::Ref(created) => {
                if !self.branch {
                    return Ok(Delivery::Other);
                }
                Delivery::Ref { repository, branch: core::mem::replace(&mut self.reference, Box::from([])), created }
            }
            Event::Ci(mut event) => {
                match event {
                    Ci::Status => {}
                    Ci::ChecksRequested | Ci::Destroyed => {
                        event = match self.action.as_ref() {
                            b"checks_requested" => Ci::ChecksRequested,
                            b"destroyed" => Ci::Destroyed,
                            _ => return Ok(Delivery::Other),
                        };
                    }
                    Ci::CheckRun | Ci::CheckSuite | Ci::WorkflowJob | Ci::WorkflowRun => {
                        if self.action.as_ref() != b"completed" {
                            return Ok(Delivery::Other);
                        }
                    }
                }
                Delivery::Ci { repository, commit: self.commit, event }
            }
            Event::Installation | Event::Repositories | Event::Target => {
                let change = match (self.event, self.action.as_ref()) {
                    (Event::Installation, b"created") => Change::Created,
                    (Event::Installation, b"deleted") => Change::Deleted,
                    (Event::Installation, b"suspend") => Change::Suspended,
                    (Event::Installation, b"unsuspend") => Change::Unsuspended,
                    (Event::Repositories, b"added" | b"removed") => Change::Repositories,
                    (Event::Target, b"renamed") => Change::Renamed { login: bytes::copy_of(&self.account.login) },
                    (
                        Event::Issue
                        | Event::Comment
                        | Event::Pull
                        | Event::Review
                        | Event::Remark
                        | Event::Push
                        | Event::Ref(_)
                        | Event::Ci(_)
                        | Event::Installation
                        | Event::Repositories
                        | Event::Target,
                        _,
                    ) => return Ok(Delivery::Other),
                };
                let account = core::mem::replace(&mut self.account, Account { login: Box::from([]), id: 0 });
                Delivery::Installation { installation: self.installation, account, change }
            }
        })
    }
}
fn unsigned(text: &[u8]) -> u64 {
    crate::forgejo::id::unsigned(text).expect("walker checked integer")
}

/// Reads one whole payload with the same streaming decoder and bounded input pieces.
pub fn decode(event: &[u8], body: &[u8], limits: &Limits) -> Result<Delivery, Malformed> {
    let Some(mut decoder) = decoder(event, limits) else {
        return Ok(Delivery::Other);
    };
    if body.len() > usize::try_from(limits.json.length).expect("u32 fits usize") {
        return Err(Malformed::Json(tokenizer::Error::TooLong));
    }
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&limits.json);
    let mut above = Queue::uncapped();
    let mut below = Queue::uncapped();
    tokenizer::down(&mut tokenizer, &env, first(&decoder), &mut above, &mut below);
    let mut at = 0_usize;
    for _tick in 0..body.len().saturating_mul(8).saturating_add(64) {
        if let Some(event) = above.pop() {
            match decoder.step(event) {
                Step::Demand(demand) => tokenizer::down(&mut tokenizer, &env, demand, &mut above, &mut below),
                Step::Done(delivery) => return Ok(delivery),
                Step::Malformed(error) => return Err(error),
            }
        } else {
            let read = match below.pop().expect("tokenizer demand") {
                Down::Demand { read, .. } => read,
                Down::Send(_) | Down::Finish => unreachable!("tokenizer only reads"),
            };
            let remaining = body.get(at..).expect("input position");
            let count = match read {
                Read::Fill(count) => usize::try_from(count).expect("u32 fits usize"),
                Read::Scan { until, max } => {
                    let max = usize::try_from(max).expect("u32 fits usize");
                    let scanned = remaining.get(..remaining.len().min(max)).expect("bounded scan");
                    match bytes::find(scanned, until.as_bytes()) {
                        Some(position) => position.saturating_add(until.as_bytes().len()),
                        None => max,
                    }
                }
                Read::Nothing | Read::Line { .. } | Read::Arrived { .. } => unreachable!("tokenizer fills or scans"),
            };
            let delivery = if count <= remaining.len() {
                let bytes = bytes::copy_of(remaining.get(..count).expect("bounded piece"));
                at = at.checked_add(count).expect("position within body");
                Up::Bytes(bytes)
            } else {
                Up::End
            };
            tokenizer::up(&mut tokenizer, &env, delivery, &mut above, &mut below);
        }
    }
    Err(Malformed::Json(tokenizer::Error::Unexpected))
}
