//! Streaming webhook body decoding; signature verification belongs to the caller.
use crate::forgejo::{
    decode::{
        Malformed,
        walk::{Field, Kind, Move, Read, Value, Walk},
    },
    delivery::{self, Delivery, Event, RefType},
    document::Body,
    id::{ObjectFormat, ObjectId},
    limits::Limits,
};
use alloc::boxed::Box;
use skein_json::tokenizer;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DeliveryKind {
    Issue,
    Pull,
    Comment,
    Review,
    Push,
    Ref,
    Job,
    Repo,
    Pusher,
    IssueBody,
    IssueNumber,
    CommentBody,
    ReviewContent,
    Run,
}
const fn field(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: true }
}
const fn object(kind: DeliveryKind) -> Read {
    Read::Object(Kind::Delivery(kind))
}
pub(crate) fn fields(kind: DeliveryKind) -> &'static [Field] {
    use DeliveryKind as D;
    match kind {
        D::Issue => {
            const {
                &[
                    field(b"repository", object(D::Repo)),
                    field(b"number", Read::Number),
                    field(b"issue", object(D::IssueBody)),
                ]
            }
        }
        D::Pull => const { &[field(b"repository", object(D::Repo)), field(b"number", Read::Number)] },
        D::Comment => {
            const {
                &[
                    field(b"repository", object(D::Repo)),
                    field(b"issue", object(D::IssueNumber)),
                    field(b"comment", object(D::CommentBody)),
                ]
            }
        }
        D::Review => {
            const {
                &[
                    field(b"repository", object(D::Repo)),
                    field(b"number", Read::Number),
                    field(b"review", object(D::ReviewContent)),
                ]
            }
        }
        D::Push => {
            const {
                &[
                    field(b"repository", object(D::Repo)),
                    field(b"ref", Read::Text),
                    field(b"after", Read::Id { empty: false }),
                    field(b"pusher", object(D::Pusher)),
                ]
            }
        }
        D::Ref => {
            const { &[field(b"repository", object(D::Repo)), field(b"ref", Read::Text), field(b"ref_type", Read::Text)] }
        }
        D::Job => const { &[field(b"run", object(D::Run))] },
        D::Run => const { &[field(b"repository", object(D::Repo)), field(b"commit_sha", Read::Id { empty: false })] },
        D::Repo | D::Pusher => const { &[field(b"id", Read::Number)] },
        D::IssueNumber => const { &[field(b"number", Read::Number)] },
        D::IssueBody | D::CommentBody => const { &[field(b"body", Read::Body)] },
        D::ReviewContent => const { &[field(b"content", Read::Body)] },
    }
}
/// A delivery decoder, selected by the event header.
#[derive(Debug)]
pub struct Decoder {
    walk: Walk,
    event: Event,
    repository: u64,
    number: u64,
    body: Body,
    reference: Box<[u8]>,
    after: ObjectId,
    pusher: u64,
    ref_type: RefType,
}
/// One tokenizer event's demand or terminal result.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step {
    Demand(tokenizer::Request),
    Done(Delivery),
    Malformed(Malformed),
}
/// No decoder for an unrecognised event; the caller still verifies its signature.
#[must_use]
pub fn decoder(header: &[u8], limits: &Limits) -> Option<Decoder> {
    use DeliveryKind as D;
    let event = delivery::event(header)?;
    let kind = match event {
        Event::Issue => D::Issue,
        Event::Pull => D::Pull,
        Event::Comment => D::Comment,
        Event::Review(_) => D::Review,
        Event::Push => D::Push,
        Event::Ref(_) => D::Ref,
        Event::Job(_) => D::Job,
    };
    Some(Decoder {
        walk: Walk::new(Kind::Delivery(kind), limits),
        event,
        repository: 0,
        number: 0,
        body: Body::Kept(Box::from([])),
        reference: Box::from([]),
        after: ObjectId { format: ObjectFormat::Sha1, bytes: [0; 32] },
        pusher: 0,
        ref_type: RefType::Branch,
    })
}
/// The initial tokenizer demand.
#[must_use]
pub fn first(_decoder: &Decoder) -> tokenizer::Request {
    tokenizer::Request::Next
}
/// Advances by one tokenizer event.
pub fn step(decoder: &mut Decoder, limits: &Limits, event: &tokenizer::Event) -> Step {
    match decoder.walk.step(event, limits) {
        Err(error) => Step::Malformed(error),
        Ok(Move::Demand(demand)) => Step::Demand(demand),
        Ok(Move::End(_)) => Step::Demand(tokenizer::Request::Next),
        Ok(Move::Scalar { kind, key, value }) => match decoder.scalar(kind, key, value) {
            Ok(()) => Step::Demand(tokenizer::Request::Next),
            Err(error) => Step::Malformed(error),
        },
        Ok(Move::Done) => Step::Done(decoder.finish()),
    }
}
impl Decoder {
    fn scalar(&mut self, kind: Kind, key: &'static [u8], value: Value) -> Result<(), Malformed> {
        match value {
            Value::Input(_) => unreachable!("dispatch input table"),
            Value::Number(n) => {
                if kind == Kind::Delivery(DeliveryKind::Repo) {
                    self.repository = n;
                } else if kind == Kind::Delivery(DeliveryKind::Pusher) {
                    self.pusher = n;
                } else {
                    self.number = n;
                }
            }
            Value::Text(text) => {
                if key == b"ref" {
                    self.reference = text;
                } else {
                    self.ref_type = match text.as_ref() {
                        b"branch" => RefType::Branch,
                        b"tag" => RefType::Tag,
                        _ => return Err(Malformed::Value { field: key }),
                    };
                }
            }
            Value::Body(body) => self.body = body,
            Value::Id(id) => self.after = id.expect("required object id"),
            Value::Boolean(_) | Value::Time(_) => unreachable!("delivery field table"),
        }
        Ok(())
    }
    fn finish(&mut self) -> Delivery {
        let repository = self.repository;
        let number = self.number;
        let body = core::mem::replace(&mut self.body, Body::Kept(Box::from([])));
        let reference = core::mem::replace(&mut self.reference, Box::from([]));
        match self.event {
            Event::Issue => Delivery::Issue { repository, number, body },
            Event::Pull => Delivery::Pull { repository, number },
            Event::Comment => Delivery::Comment { repository, number, body },
            Event::Review(event) => Delivery::Review { event, repository, number, body },
            Event::Push => Delivery::Push { repository, reference, after: self.after, pusher: self.pusher },
            Event::Ref(created) => Delivery::Ref { created, repository, reference, ref_type: self.ref_type },
            Event::Job(end) => Delivery::Job { end, repository, commit: self.after },
        }
    }
}
