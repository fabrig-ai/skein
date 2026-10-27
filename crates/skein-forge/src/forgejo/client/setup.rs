use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Field, Kind, Move, Read, Value, Walk},
    },
    document::{self, AccessToken, Document, Row, User},
};
use skein_json::tokenizer;
use skein_lib::{List, Secret};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SetupKind {
    Hook,
    Hooks,
    Config,
    Events,
    Event,
    Account,
    Org,
    CreatedRepository,
    CreatedFile,
    FileCommit,
    Team,
    Teams,
    Units,
    Token,
    Tokens,
    Scopes,
    Scope,
}
pub(crate) fn fields(kind: SetupKind) -> &'static [Field] {
    match kind {
        SetupKind::Hook | SetupKind::Hooks | SetupKind::Config | SetupKind::Events | SetupKind::Event => {
            super::hooks::fields(kind)
        }
        SetupKind::Token => {
            const {
                &[
                    Field { key: b"id", read: Read::Number, required: true },
                    Field { key: b"name", read: Read::Text, required: true },
                    Field { key: b"scopes", read: Read::Array(Kind::Setup(SetupKind::Scopes)), required: true },
                    Field { key: b"sha1", read: Read::Text, required: false },
                    Field { key: b"token_last_eight", read: Read::Text, required: false },
                ]
            }
        }
        SetupKind::Tokens | SetupKind::Scopes | SetupKind::Scope => &[],
        SetupKind::Org | SetupKind::Team | SetupKind::Teams | SetupKind::Units => super::orgs::fields(kind),
        SetupKind::CreatedRepository => super::repositories::fields(),
        SetupKind::CreatedFile => {
            const { &[Field { key: b"commit", read: Read::Object(Kind::Setup(SetupKind::FileCommit)), required: true }] }
        }
        SetupKind::FileCommit => const { &[Field { key: b"sha", read: Read::Id { empty: false }, required: true }] },
        SetupKind::Account => {
            const {
                &[
                    Field { key: b"id", read: Read::Number, required: true },
                    Field { key: b"login", read: Read::Text, required: true },
                    Field { key: b"is_admin", read: Read::Boolean, required: false },
                    Field { key: b"full_name", read: Read::Text, required: false },
                ]
            }
        }
    }
}
#[derive(Debug)]
pub(super) struct Setup {
    user: User,
    token: AccessToken,
    scopes: List<Box<[u8]>>,
    kind: SetupKind,
}
impl Setup {
    pub(super) fn new(kind: SetupKind, limits: &Limits) -> Self {
        Self { user: document::user(), token: token(), scopes: List::with_capacity(limits.inner), kind }
    }
    pub(super) fn step(&mut self, movement: Move, limits: &Limits, walk: &Walk) -> Step {
        match movement {
            Move::Demand(demand) => {
                return Step::Demand(if walk.parent() == Some(Kind::Setup(SetupKind::Scopes)) {
                    tokenizer::Request::Text(limits.string_bytes)
                } else {
                    demand
                });
            }
            Move::Scalar { kind, key, value } if kind != Kind::Setup(SetupKind::Account) => match (key, value) {
                (b"id", Value::Number(id)) => self.token.id = id,
                (b"name", Value::Text(text)) => self.token.name = text,
                (b"sha1", Value::Text(text)) => {
                    self.token.value = if text.is_empty() { None } else { Some(Secret::new(text)) }
                }
                (b"token_last_eight", Value::Text(text)) => self.token.last_eight = text,
                (b"scopes", Value::Text(text)) => {
                    if self.scopes.push(text).is_err() {
                        return Step::Malformed(Malformed::Bound { field: key });
                    }
                }
                _ => return Step::Malformed(Malformed::Shape { field: key }),
            },
            Move::Scalar { key, value, .. } => match value {
                Value::Input(_) => unreachable!("dispatch input table"),
                Value::Number(value) => self.user.id = value,
                Value::Boolean(value) => self.user.is_admin = value,
                Value::Text(text) => {
                    if key == b"login" {
                        self.user.login = text;
                    } else {
                        self.user.full_name = text;
                    }
                }
                Value::Body(_) | Value::Time(_) | Value::Id(_) => {
                    return Step::Malformed(Malformed::Shape { field: key });
                }
            },
            Move::End(Kind::Setup(SetupKind::Scopes)) => {
                self.token.scopes =
                    core::mem::replace(&mut self.scopes, List::with_capacity(limits.inner)).into_boxed();
            }
            Move::End(Kind::Setup(SetupKind::Token)) if self.kind == SetupKind::Tokens => {
                return Step::Row(Row::Token(core::mem::replace(&mut self.token, token())), tokenizer::Request::Next);
            }
            Move::End(Kind::Setup(_)) => {}
            Move::End(
                Kind::User
                | Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Users
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_)
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_),
            ) => unreachable!("setup tables"),
            Move::Done => {
                return Step::Done(match self.kind {
                    SetupKind::Account => Document::User(core::mem::replace(&mut self.user, document::user())),
                    SetupKind::Token => Document::Token(core::mem::replace(&mut self.token, token())),
                    SetupKind::Tokens => Document::Listed { skipped: 0 },
                    SetupKind::Hook
                    | SetupKind::Hooks
                    | SetupKind::Config
                    | SetupKind::Events
                    | SetupKind::Event
                    | SetupKind::CreatedFile
                    | SetupKind::FileCommit
                    | SetupKind::CreatedRepository
                    | SetupKind::Org
                    | SetupKind::Team
                    | SetupKind::Teams
                    | SetupKind::Units
                    | SetupKind::Scopes
                    | SetupKind::Scope => unreachable!("setup root"),
                });
            }
        }
        Step::Demand(if walk.parent() == Some(Kind::Setup(SetupKind::Scopes)) {
            tokenizer::Request::Text(limits.string_bytes)
        } else {
            tokenizer::Request::Next
        })
    }
}

fn token() -> AccessToken {
    AccessToken { id: 0, name: Box::from([]), scopes: Box::from([]), last_eight: Box::from([]), value: None }
}
pub(crate) fn element(kind: SetupKind) -> Option<Kind> {
    match kind {
        SetupKind::Hooks => Some(Kind::Setup(SetupKind::Hook)),
        SetupKind::Events => Some(Kind::Setup(SetupKind::Event)),
        SetupKind::Teams => Some(Kind::Setup(SetupKind::Team)),
        SetupKind::Tokens => Some(Kind::Setup(SetupKind::Token)),
        SetupKind::Scopes => Some(Kind::Setup(SetupKind::Scope)),
        SetupKind::Hook
        | SetupKind::Config
        | SetupKind::Event
        | SetupKind::CreatedFile
        | SetupKind::FileCommit
        | SetupKind::CreatedRepository
        | SetupKind::Org
        | SetupKind::Team
        | SetupKind::Units
        | SetupKind::Account
        | SetupKind::Token
        | SetupKind::Scope => None,
    }
}

use super::{Encoded, Refusal, items::Target};
use crate::forgejo::{
    limits::Limits,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_json::writer::{self, Encoder};
use skein_lib::Writer;
pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let mut measure = Target { length: 0, writer: None };
    let method = target(&mut measure, request);
    if measure.length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let mut out = Target { length: 0, writer: Some(Writer::new(measure.length)) };
    target(&mut out, request);
    let target = out.writer.expect("writing pass").finish();
    if method != Method::Post {
        return Ok(Encoded { method, target, body: None });
    }
    let body = {
        let mut measure = Encoder::measure(&limits.writer);
        write(&mut measure, request);
        let length = match measure.measured() {
            Ok(length) => length,
            Err(writer::Refusal::Text) => return Err(Refusal::Text),
            Err(writer::Refusal::TooLong | writer::Refusal::TooDeep | writer::Refusal::Number) => {
                return Err(Refusal::TooLong);
            }
        };
        let mut out = Encoder::write(length, &limits.writer);
        write(&mut out, request);
        Some(out.finish())
    };
    Ok(Encoded { method, target, body })
}
fn write(json: &mut Encoder, request: &Request) {
    if let Request::CreateToken { name, scopes, .. } = request {
        json.object_start();
        json.key(b"name");
        json.string(name);
        json.key(b"scopes");
        json.array_start();
        for scope in scopes {
            json.string(scope);
        }
        json.array_end();
        json.object_end();
        return;
    }
    let Request::CreateUser { login, email, password, full_name, must_change_password } = request else {
        unreachable!("user body selection")
    };
    json.object_start();
    for (key, value) in [
        (b"username".as_slice(), login.as_ref()),
        (b"email", email),
        (b"password", password.expose()),
        (b"full_name", full_name),
    ] {
        json.key(key);
        json.string(value);
    }
    json.key(b"must_change_password");
    json.boolean(*must_change_password);
    json.object_end();
}

fn target(out: &mut Target, request: &Request) -> Method {
    if let Request::UserOf { login } = request {
        out.put(b"/api/v1/users/");
        out.segment(login);
        return Method::Get;
    }
    out.put(b"/api/v1/admin/users");
    if let Request::CreateUser { .. } = request {
        return Method::Post;
    }
    let (login, method) = if let Request::Tokens { login, .. } = request {
        (login, Method::Get)
    } else if let Request::CreateToken { login, .. } = request {
        (login, Method::Post)
    } else if let Request::DeleteToken { login, .. } = request {
        (login, Method::Delete)
    } else {
        unreachable!("setup request selection")
    };
    out.put(b"/");
    out.segment(login);
    out.put(b"/tokens");
    if let Request::Tokens { page, .. } = request {
        out.page(*page, false);
    }
    if let Request::DeleteToken { id, .. } = request {
        out.put(b"/");
        out.number(*id);
    }
    method
}
