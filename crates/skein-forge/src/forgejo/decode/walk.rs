//! Static object tables, required masks and a bounded container stack.
use super::Malformed;
use crate::forgejo::client::actions::ActionKind;
use crate::forgejo::{document::Body, id::ObjectId, limits::Limits};
use alloc::boxed::Box;
use skein_json::{Token, tokenizer};
use skein_lib::{Stack, Wall};
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Item {
    Issue,
    Issues,
    Comment,
    Comments,
    Pull,
    Pulls,
    Head,
    Base,
    Status,
    Statuses,
    Combined,
    Label,
    Labels,
    Reviewers,
    Presence,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Error,
    Delivery(crate::forgejo::client::delivery::DeliveryKind),
    Write(crate::forgejo::server::writes::WriteKind),
    User,
    Setup(crate::forgejo::client::setup::SetupKind),
    Settings,
    Repository,
    Search,
    Reachable(crate::forgejo::client::reachable::ReachableKind),
    Users,
    File(crate::forgejo::client::files::FileKind),
    Action(ActionKind),
    Item(Item),
    Review(crate::forgejo::client::reviews::ReviewKind),
    Branch(crate::forgejo::client::branches::BranchKind),
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Read {
    Number,
    Text,
    Boolean,
    Body,
    Time,
    Id { empty: bool },
    Object(Kind),
    Array(Kind),
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Field {
    pub key: &'static [u8],
    pub read: Read,
    pub required: bool,
}
const fn field(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: true }
}
const fn optional(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: false }
}
fn fields(kind: Kind) -> &'static [Field] {
    match kind {
        Kind::Setup(kind) => crate::forgejo::client::setup::fields(kind),
        Kind::Reachable(kind) => crate::forgejo::client::reachable::fields(kind),
        Kind::Error => error_fields(),
        Kind::Delivery(kind) => crate::forgejo::client::delivery::fields(kind),
        Kind::Write(kind) => crate::forgejo::server::writes::fields(kind),
        Kind::File(kind) => crate::forgejo::client::files::fields(kind),
        Kind::Action(kind) => crate::forgejo::client::actions::fields(kind),
        Kind::Review(kind) => crate::forgejo::client::reviews::fields(kind),
        Kind::Branch(kind) => crate::forgejo::client::branches::fields(kind),
        Kind::User => const { &[field(b"id", Read::Number), field(b"login", Read::Text)] },
        Kind::Settings => settings_fields(),
        Kind::Repository => repository_fields(),
        Kind::Search => const { &[field(b"data", Read::Array(Kind::Users)), field(b"ok", Read::Boolean)] },
        Kind::Item(Item::Issue) => {
            const {
                &[
                    field(b"number", Read::Number),
                    optional(b"pull_request", Read::Object(Kind::Item(Item::Presence))),
                    field(b"state", Read::Text),
                    field(b"user", Read::Object(Kind::User)),
                    field(b"labels", Read::Array(Kind::Item(Item::Labels))),
                    field(b"updated_at", Read::Time),
                    field(b"title", Read::Text),
                    field(b"body", Read::Body),
                ]
            }
        }
        Kind::Item(Item::Comment) => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"user", Read::Object(Kind::User)),
                    optional(b"created_at", Read::Time),
                    optional(b"updated_at", Read::Time),
                    field(b"body", Read::Body),
                ]
            }
        }
        Kind::Item(Item::Pull) => {
            const {
                &[
                    field(b"number", Read::Number),
                    field(b"state", Read::Text),
                    field(b"head", Read::Object(Kind::Item(Item::Head))),
                    field(b"base", Read::Object(Kind::Item(Item::Base))),
                    field(b"merged", Read::Boolean),
                    optional(b"merge_commit_sha", Read::Id { empty: false }),
                    optional(b"merge_base", Read::Id { empty: false }),
                    field(b"mergeable", Read::Boolean),
                    field(b"requested_reviewers", Read::Array(Kind::Item(Item::Reviewers))),
                ]
            }
        }
        Kind::Item(Item::Head) => pull_branch(false),
        Kind::Item(Item::Base) => pull_branch(true),
        Kind::Item(Item::Status) => {
            const {
                &[
                    field(b"creator", Read::Object(Kind::User)),
                    field(b"updated_at", Read::Time),
                    field(b"context", Read::Text),
                    field(b"status", Read::Text),
                    field(b"description", Read::Text),
                    field(b"target_url", Read::Text),
                ]
            }
        }
        Kind::Item(Item::Combined) => {
            const {
                &[
                    field(b"state", Read::Text),
                    field(b"total_count", Read::Number),
                    field(b"statuses", Read::Array(Kind::Item(Item::Statuses))),
                ]
            }
        }
        Kind::Item(Item::Label) => const { &[field(b"name", Read::Text)] },
        Kind::Users
        | Kind::Item(
            Item::Pulls
            | Item::Issues
            | Item::Comments
            | Item::Statuses
            | Item::Labels
            | Item::Reviewers
            | Item::Presence,
        ) => const { &[] },
    }
}
fn error_fields() -> &'static [Field] {
    const { &[field(b"message", Read::Body)] }
}
fn settings_fields() -> &'static [Field] {
    const { &[field(b"max_response_items", Read::Number), field(b"default_paging_num", Read::Number)] }
}
fn pull_branch(empty: bool) -> &'static [Field] {
    if empty {
        const { &[field(b"ref", Read::Text), field(b"sha", Read::Id { empty: true }), field(b"repo_id", Read::Number)] }
    } else {
        const { &[field(b"ref", Read::Text), field(b"sha", Read::Id { empty: false }), field(b"repo_id", Read::Number)] }
    }
}
fn element(kind: Kind) -> Option<Kind> {
    match kind {
        Kind::Write(kind) => crate::forgejo::server::writes::element(kind),
        Kind::File(kind) => crate::forgejo::client::files::element(kind),
        Kind::Action(kind) => crate::forgejo::client::actions::element(kind),
        Kind::Review(kind) => crate::forgejo::client::reviews::element(kind),
        Kind::Branch(kind) => crate::forgejo::client::branches::element(kind),
        Kind::Item(Item::Pulls) => Some(Kind::Item(Item::Pull)),
        Kind::Reachable(crate::forgejo::client::reachable::ReachableKind::Listing) => {
            Some(Kind::Reachable(crate::forgejo::client::reachable::ReachableKind::Row))
        }
        Kind::Reachable(crate::forgejo::client::reachable::ReachableKind::FoundListing) => {
            Some(Kind::Reachable(crate::forgejo::client::reachable::ReachableKind::Found))
        }
        Kind::Users | Kind::Item(Item::Reviewers) => Some(Kind::User),
        Kind::Item(Item::Issues) => Some(Kind::Item(Item::Issue)),
        Kind::Item(Item::Comments) => Some(Kind::Item(Item::Comment)),
        Kind::Item(Item::Statuses) => Some(Kind::Item(Item::Status)),
        Kind::Item(Item::Labels) => Some(Kind::Item(Item::Label)),
        Kind::Setup(kind) => crate::forgejo::client::setup::element(kind),
        Kind::Reachable(_)
        | Kind::Delivery(_)
        | Kind::Error
        | Kind::User
        | Kind::Settings
        | Kind::Repository
        | Kind::Search
        | Kind::Item(
            Item::Issue
            | Item::Comment
            | Item::Pull
            | Item::Head
            | Item::Base
            | Item::Status
            | Item::Combined
            | Item::Label
            | Item::Presence,
        ) => None,
    }
}
#[derive(Debug)]
struct Frame {
    kind: Kind,
    touched: u64,
    seen: u64,
}
#[derive(Debug)]
enum Want {
    Start(Kind),
    Input(Box<[u8]>),
    Key,
    Value { field: Field, bit: u64 },
    Skip,
    Element,
    Done,
    Terminal,
    Cut,
}
#[derive(Debug)]
pub(crate) struct Walk {
    open: Stack<Frame>,
    want: Want,
    skip_row: bool,
    skipped: u32,
}
#[derive(Debug)]
pub(crate) enum Value {
    Input(crate::api::Input),
    Number(u64),
    Text(Box<[u8]>),
    Boolean(bool),
    Body(Body),
    Time(Wall),
    Id(Option<ObjectId>),
}
#[derive(Debug)]
pub(crate) enum Move {
    Demand(tokenizer::Request),
    Scalar { kind: Kind, key: &'static [u8], value: Value },
    End(Kind),
    Done,
}
impl Walk {
    pub(crate) fn skipped(&self) -> u32 {
        self.skipped
    }
    pub(crate) fn new(kind: Kind, limits: &Limits) -> Self {
        Self { open: Stack::with_capacity(limits.json.depth), want: Want::Start(kind), skip_row: false, skipped: 0 }
    }
    pub(crate) fn parent(&self) -> Option<Kind> {
        let frame = self.open.top()?;
        Some(frame.kind)
    }
    pub(crate) fn skip_comment(&mut self) {
        self.skip_row = true;
    }
    pub(crate) fn cut(&mut self) -> tokenizer::Request {
        assert!(
            self.parent() == Some(Kind::Item(Item::Comments))
                || self.parent() == Some(Kind::Review(crate::forgejo::client::reviews::ReviewKind::Comments)),
            "cut at an unpaged thread's row boundary"
        );
        self.want = Want::Cut;
        tokenizer::Request::Skip
    }
    pub(crate) fn step(&mut self, event: &tokenizer::Event, limits: &Limits) -> Result<Move, Malformed> {
        match event {
            tokenizer::Event::Failed(error) => return Err(Malformed::Json(*error)),
            tokenizer::Event::Closed => return Err(Malformed::Shape { field: b"" }),
            tokenizer::Event::Token(_)
            | tokenizer::Event::Long(_)
            | tokenizer::Event::Skipped(_)
            | tokenizer::Event::Done => {}
        }
        let want = core::mem::replace(&mut self.want, Want::Terminal);
        match want {
            Want::Start(kind) => self.start(kind, event),
            Want::Input(name) => {
                let value = scalar(field(b"inputs", Read::Text), event)?;
                let Value::Text(value) = value else {
                    return Err(Malformed::Shape { field: b"inputs" });
                };
                self.want = Want::Key;
                Ok(Move::Scalar {
                    kind: self.parent().expect("inputs object"),
                    key: b"inputs",
                    value: Value::Input(crate::api::Input { name, value }),
                })
            }
            Want::Key => self.key(event, limits),
            Want::Value { field, bit } => self.value(field, bit, event),
            Want::Skip => {
                match event {
                    tokenizer::Event::Skipped(_) => {}
                    tokenizer::Event::Token(_)
                    | tokenizer::Event::Long(_)
                    | tokenizer::Event::Done
                    | tokenizer::Event::Failed(_)
                    | tokenizer::Event::Closed => return Err(Malformed::Shape { field: b"" }),
                }
                self.want = Want::Key;
                Ok(Move::Demand(tokenizer::Request::Next))
            }
            Want::Element => {
                if event == &tokenizer::Event::Token(Token::ArrayEnd) {
                    self.end()
                } else {
                    let kind = element(self.parent().expect("array element")).expect("array kind");
                    if kind == Kind::Setup(crate::forgejo::client::setup::SetupKind::Event)
                        || kind == Kind::Setup(crate::forgejo::client::setup::SetupKind::Scope)
                        || kind == Kind::Write(crate::forgejo::server::writes::WriteKind::Scope)
                        || kind == Kind::Branch(crate::forgejo::client::branches::BranchKind::Context)
                        || kind == Kind::Write(crate::forgejo::server::writes::WriteKind::Login)
                    {
                        let key = if kind == Kind::Setup(crate::forgejo::client::setup::SetupKind::Scope)
                            || kind == Kind::Write(crate::forgejo::server::writes::WriteKind::Scope)
                        {
                            b"scopes".as_slice()
                        } else if kind == Kind::Setup(crate::forgejo::client::setup::SetupKind::Event) {
                            b"events".as_slice()
                        } else if kind == Kind::Write(crate::forgejo::server::writes::WriteKind::Login) {
                            b"reviewers".as_slice()
                        } else {
                            b"status_check_contexts".as_slice()
                        };
                        let value = scalar(field(key, Read::Text), event)?;
                        self.want = Want::Element;
                        Ok(Move::Scalar { kind, key, value })
                    } else {
                        self.start(kind, event)
                    }
                }
            }
            Want::Cut => match event {
                tokenizer::Event::Token(Token::ArrayEnd) => self.end(),
                tokenizer::Event::Skipped(_) => {
                    self.skipped = self.skipped.saturating_add(1);
                    self.want = Want::Cut;
                    Ok(Move::Demand(tokenizer::Request::Skip))
                }
                tokenizer::Event::Token(_)
                | tokenizer::Event::Long(_)
                | tokenizer::Event::Done
                | tokenizer::Event::Failed(_)
                | tokenizer::Event::Closed => Err(Malformed::Shape { field: b"" }),
            },
            Want::Done => {
                if event == &tokenizer::Event::Done {
                    Ok(Move::Done)
                } else {
                    Err(Malformed::Shape { field: b"" })
                }
            }
            Want::Terminal => Err(Malformed::Shape { field: b"" }),
        }
    }
    fn start(&mut self, kind: Kind, event: &tokenizer::Event) -> Result<Move, Malformed> {
        let array = element(kind).is_some();
        let expected = if array { Token::ArrayStart } else { Token::ObjectStart };
        if event != &tokenizer::Event::Token(expected) {
            return Err(Malformed::Shape { field: b"" });
        }
        if self.open.push(Frame { kind, touched: 0, seen: 0 }).is_err() {
            return Err(Malformed::Json(tokenizer::Error::TooDeep));
        }
        if kind == Kind::Item(Item::Comment) {
            self.skip_row = false;
        }
        self.want = if array { Want::Element } else { Want::Key };
        Ok(Move::Demand(tokenizer::Request::Next))
    }
    fn key(&mut self, event: &tokenizer::Event, limits: &Limits) -> Result<Move, Malformed> {
        match event {
            tokenizer::Event::Token(Token::ObjectEnd) => self.end(),
            tokenizer::Event::Token(Token::Key(key)) => {
                let frame = self.open.top_mut().expect("key inside object");
                if self.skip_row && frame.kind == Kind::Item(Item::Comment) {
                    self.want = Want::Skip;
                    return Ok(Move::Demand(tokenizer::Request::Skip));
                }
                if frame.kind == Kind::Write(crate::forgejo::server::writes::WriteKind::Inputs) {
                    self.want = Want::Input(key.clone());
                    return Ok(Move::Demand(tokenizer::Request::Text(limits.string_bytes)));
                }
                for (index, field) in fields(frame.kind).iter().enumerate() {
                    if field.key == &**key {
                        let bit =
                            1u64.checked_shl(u32::try_from(index).expect("small field table")).expect("field mask");
                        if frame.touched & bit != 0 {
                            return Err(Malformed::Shape { field: field.key });
                        }
                        frame.touched |= bit;
                        self.want = Want::Value { field: *field, bit };
                        let demand = match field.read {
                            Read::Text | Read::Time | Read::Id { .. } => tokenizer::Request::Text(limits.string_bytes),
                            Read::Body => tokenizer::Request::Text(
                                if frame.kind == Kind::Error || frame.kind == Kind::Action(ActionKind::Step) {
                                    limits.string_bytes
                                } else {
                                    limits.body_bytes
                                },
                            ),
                            Read::Number | Read::Boolean | Read::Object(_) | Read::Array(_) => tokenizer::Request::Next,
                        };
                        return Ok(Move::Demand(demand));
                    }
                }
                self.want = Want::Skip;
                Ok(Move::Demand(tokenizer::Request::Skip))
            }
            tokenizer::Event::Token(
                Token::ObjectStart
                | Token::ArrayStart
                | Token::ArrayEnd
                | Token::String(_)
                | Token::Number(_)
                | Token::True
                | Token::False
                | Token::Null,
            )
            | tokenizer::Event::Long(_)
            | tokenizer::Event::Skipped(_)
            | tokenizer::Event::Done
            | tokenizer::Event::Failed(_)
            | tokenizer::Event::Closed => Err(Malformed::Shape { field: b"" }),
        }
    }
    fn value(&mut self, field: Field, bit: u64, event: &tokenizer::Event) -> Result<Move, Malformed> {
        let frame = self.open.top_mut().expect("value inside object");
        let kind = frame.kind;
        if event == &tokenizer::Event::Token(Token::Null) {
            if let Kind::Delivery(_) = kind {
                return Err(Malformed::Shape { field: field.key });
            }
            if field.key == b"statuses"
                || field.key == b"creator"
                || (kind == Kind::File(crate::forgejo::client::files::FileKind::Tree) && field.key == b"tree")
            {
                frame.seen |= bit;
            }
            self.want = Want::Key;
            return Ok(Move::Demand(tokenizer::Request::Next));
        }
        frame.seen |= bit;
        match field.read {
            Read::Object(child) | Read::Array(child) => match self.start(child, event) {
                Ok(result) => Ok(result),
                Err(Malformed::Shape { .. }) => Err(Malformed::Shape { field: field.key }),
                Err(error) => Err(error),
            },
            Read::Number | Read::Text | Read::Boolean | Read::Body | Read::Time | Read::Id { .. } => {
                let value = scalar(field, event)?;
                self.want = Want::Key;
                Ok(Move::Scalar { kind, key: field.key, value })
            }
        }
    }
    fn end(&mut self) -> Result<Move, Malformed> {
        let frame = self.open.pop().expect("container end");
        if !(self.skip_row && frame.kind == Kind::Item(Item::Comment)) {
            for (index, field) in fields(frame.kind).iter().enumerate() {
                let bit = 1u64.checked_shl(u32::try_from(index).expect("field index")).expect("field mask");
                if field.required && frame.seen & bit == 0 {
                    return Err(Malformed::Missing { field: field.key });
                }
            }
        }
        self.want = match self.parent() {
            None => Want::Done,
            Some(kind) => {
                if element(kind).is_some() {
                    Want::Element
                } else {
                    Want::Key
                }
            }
        };
        Ok(Move::End(frame.kind))
    }
}
fn scalar(field: Field, event: &tokenizer::Event) -> Result<Value, Malformed> {
    let wrong = Malformed::Shape { field: field.key };
    match event {
        tokenizer::Event::Token(Token::Number(text)) => {
            if field.read != Read::Number {
                return Err(wrong);
            }
            Ok(Value::Number(crate::forgejo::id::unsigned(text).ok_or(Malformed::Range { field: field.key })?))
        }
        tokenizer::Event::Token(Token::String(text)) => match field.read {
            Read::Text => Ok(Value::Text(text.clone())),
            Read::Body => Ok(Value::Body(Body::Kept(text.clone()))),
            Read::Time => {
                Ok(Value::Time(crate::forgejo::time::read(text).ok_or(Malformed::Time { field: field.key })?))
            }
            Read::Id { empty } => {
                let value = if empty && text.is_empty() {
                    None
                } else {
                    Some(crate::forgejo::id::read(text).ok_or(Malformed::Id { field: field.key })?)
                };
                Ok(Value::Id(value))
            }
            Read::Number | Read::Boolean | Read::Object(_) | Read::Array(_) => Err(wrong),
        },
        tokenizer::Event::Token(Token::True | Token::False) => {
            if field.read != Read::Boolean {
                return Err(wrong);
            }
            Ok(Value::Boolean(event == &tokenizer::Event::Token(Token::True)))
        }
        tokenizer::Event::Long(length) => {
            if field.read == Read::Body {
                Ok(Value::Body(Body::Long(*length)))
            } else {
                Err(Malformed::TooLong { field: field.key })
            }
        }
        tokenizer::Event::Token(
            Token::ObjectStart | Token::ObjectEnd | Token::ArrayStart | Token::ArrayEnd | Token::Key(_) | Token::Null,
        )
        | tokenizer::Event::Skipped(_)
        | tokenizer::Event::Done
        | tokenizer::Event::Failed(_)
        | tokenizer::Event::Closed => Err(wrong),
    }
}

fn repository_fields() -> &'static [Field] {
    const {
        &[
            field(b"id", Read::Number),
            field(b"owner", Read::Object(Kind::User)),
            field(b"name", Read::Text),
            field(b"object_format_name", Read::Text),
            field(b"default_branch", Read::Text),
            field(b"default_merge_style", Read::Text),
            field(b"allow_merge_commits", Read::Boolean),
            field(b"allow_squash_merge", Read::Boolean),
            field(b"allow_rebase", Read::Boolean),
            field(b"html_url", Read::Text),
        ]
    }
}
