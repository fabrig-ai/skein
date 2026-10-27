use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Field, Kind, Move, Read, Value},
    },
    document::{self, Document, Row},
    limits::Limits,
};
use alloc::boxed::Box;
use skein_json::tokenizer;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReachableKind {
    Listing,
    Search,
    FoundListing,
    Found,
    Row,
    Owner,
    Permissions,
}
const fn field(key: &'static [u8], read: Read, required: bool) -> Field {
    Field { key, read, required }
}
pub(crate) fn fields(kind: ReachableKind) -> &'static [Field] {
    match kind {
        ReachableKind::Listing | ReachableKind::FoundListing => &[],
        ReachableKind::Search => {
            const {
                &[
                    field(b"ok", Read::Boolean, true),
                    field(b"data", Read::Array(Kind::Reachable(ReachableKind::FoundListing)), true),
                ]
            }
        }
        ReachableKind::Found => {
            const {
                &[
                    field(b"id", Read::Number, true),
                    field(b"name", Read::Text, true),
                    field(b"private", Read::Boolean, true),
                    field(b"owner", Read::Object(Kind::Reachable(ReachableKind::Owner)), true),
                ]
            }
        }
        ReachableKind::Row => {
            const {
                &[
                    field(b"id", Read::Number, true),
                    field(b"name", Read::Text, true),
                    field(b"private", Read::Boolean, true),
                    field(b"owner", Read::Object(Kind::Reachable(ReachableKind::Owner)), true),
                    field(b"permissions", Read::Object(Kind::Reachable(ReachableKind::Permissions)), true),
                ]
            }
        }
        ReachableKind::Owner => const { &[field(b"id", Read::Number, true), field(b"login", Read::Text, true)] },
        ReachableKind::Permissions => {
            const {
                &[
                    field(b"push", Read::Boolean, true),
                    field(b"pull", Read::Boolean, false),
                    field(b"admin", Read::Boolean, false),
                ]
            }
        }
    }
}
fn empty() -> document::Reachable {
    document::Reachable {
        repository: 0,
        owner: 0,
        login: Box::from([]),
        name: Box::from([]),
        private: false,
        push: false,
    }
}
#[derive(Debug)]
pub(super) struct Reachable {
    row: document::Reachable,
    count: u32,
    search: bool,
    ok: bool,
}
impl Reachable {
    pub(super) fn new(kind: ReachableKind) -> Self {
        Self { row: empty(), count: 0, search: kind == ReachableKind::Search, ok: false }
    }
    pub(super) fn step(&mut self, movement: Move, limits: &Limits) -> Step {
        match movement {
            Move::Demand(next) => return Step::Demand(next),
            Move::Done => {
                return Step::Done(if self.search {
                    Document::RepoSearch { ok: self.ok }
                } else {
                    Document::Listed { skipped: 0 }
                });
            }
            Move::End(kind) => {
                if kind == Kind::Reachable(ReachableKind::Row) || kind == Kind::Reachable(ReachableKind::Found) {
                    if self.count >= limits.inner {
                        return Step::Malformed(Malformed::Bound { field: b"repositories" });
                    }
                    self.count = self.count.saturating_add(1);
                    let mut row = core::mem::replace(&mut self.row, empty());
                    let row = if self.search {
                        row.push = true;
                        Row::Found(row)
                    } else {
                        Row::Reachable(row)
                    };
                    return Step::Row(row, tokenizer::Request::Next);
                }
            }
            Move::Scalar { kind, key, value } => match value {
                Value::Input(_) => unreachable!("dispatch input table"),
                Value::Number(n) => {
                    if kind == Kind::Reachable(ReachableKind::Owner) {
                        self.row.owner = n;
                    } else {
                        self.row.repository = n;
                    }
                }
                Value::Text(text) => {
                    if key == b"login" {
                        self.row.login = text;
                    } else {
                        self.row.name = text;
                    }
                }
                Value::Boolean(value) => {
                    if key == b"ok" {
                        self.ok = value;
                    } else if key == b"private" {
                        self.row.private = value;
                    } else if key == b"push" {
                        self.row.push = value;
                    }
                }
                Value::Body(_) | Value::Time(_) | Value::Id(_) => {
                    return Step::Malformed(Malformed::Shape { field: key });
                }
            },
        }
        Step::Demand(tokenizer::Request::Next)
    }
}
