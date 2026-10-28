use super::{BodyDecoder, Route, Unrouted, writes::WriteKind};
use crate::forgejo::{
    decode::walk::{Kind, Walk},
    limits::Limits,
    path,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_lib::{List, Secret};
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let target = path::read(bytes, limits).ok().ok_or(Unrouted::Malformed)?;
    match target.segments.as_ref() {
        [api, v1, users, login] if &**api == b"api" && &**v1 == b"v1" && &**users == b"users" => {
            if method != Method::Get {
                return Err(Unrouted::Method);
            }
            Ok(Route::Ready(Request::UserOf { login: login.clone() }))
        }
        [api, v1, admin, users]
            if &**api == b"api" && &**v1 == b"v1" && &**admin == b"admin" && &**users == b"users" =>
        {
            if method != Method::Post {
                return Err(Unrouted::Method);
            }
            Ok(Route::Body(BodyDecoder {
                walk: Walk::new(Kind::Write(WriteKind::NewUser), limits),
                request: Request::CreateUser {
                    login: Box::from([]),
                    email: Box::from([]),
                    full_name: Box::from([]),
                    password: Secret::new(Box::from([])),
                    must_change_password: false,
                },
                texts: List::with_capacity(limits.inner),
                numbers: List::with_capacity(0),
                inputs: List::with_capacity(0),
                terminal: false,
                committer: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
            }))
        }
        [api, v1, admin, users, login, tokens, tail @ ..]
            if &**api == b"api"
                && &**v1 == b"v1"
                && &**admin == b"admin"
                && &**users == b"users"
                && &**tokens == b"tokens" =>
        {
            let login = login.clone();
            match (method, tail) {
                (Method::Get, []) => {
                    Ok(Route::Ready(Request::Tokens { login, page: super::items::page(&target.query)? }))
                }
                (Method::Post, []) => Ok(Route::Body(BodyDecoder {
                    walk: Walk::new(Kind::Write(WriteKind::NewToken), limits),
                    request: Request::CreateToken { login, name: Box::from([]), scopes: Box::from([]) },
                    texts: List::with_capacity(limits.inner),
                    numbers: List::with_capacity(0),
                    inputs: List::with_capacity(0),
                    terminal: false,
                    committer: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
                })),
                (Method::Delete, [id]) => {
                    Ok(Route::Ready(Request::DeleteToken { login, id: super::items::number_value(id)? }))
                }
                (_, [] | [_]) => Err(Unrouted::Method),
                _ => Err(Unrouted::NotFound),
            }
        }
        _ => Err(Unrouted::NotFound),
    }
}

pub(super) fn token(json: &mut skein_json::writer::Encoder, token: &crate::forgejo::document::AccessToken) {
    json.object_start();
    json.key(b"id");
    json.unsigned(token.id);
    json.key(b"name");
    json.string(&token.name);
    json.key(b"scopes");
    json.array_start();
    for scope in &token.scopes {
        json.string(scope);
    }
    json.array_end();
    json.key(b"token_last_eight");
    json.string(&token.last_eight);
    if let Some(value) = &token.value {
        json.key(b"sha1");
        json.string(value.expose());
    }
    json.object_end();
}
