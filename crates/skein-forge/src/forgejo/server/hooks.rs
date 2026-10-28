use super::{
    Answer, BodyDecoder, Route, Unrouted,
    items::{number_value, page},
    writes::WriteKind,
};
use crate::forgejo::{
    client::{hooks, setup::SetupKind},
    decode::{
        Malformed,
        walk::{Field, Kind, Read, Walk},
    },
    limits::Limits,
    path,
    request::{HookSpec, Method, Request},
};
use alloc::boxed::Box;
use skein_json::writer::Encoder;
use skein_lib::{List, Secret};
pub(super) fn fields() -> &'static [Field] {
    const {
        &[
            Field { key: b"type", read: Read::Text, required: false },
            Field { key: b"config", read: Read::Object(Kind::Write(WriteKind::HookConfig)), required: true },
            Field { key: b"events", read: Read::Array(Kind::Setup(SetupKind::Events)), required: false },
            Field { key: b"active", read: Read::Boolean, required: false },
        ]
    }
}
pub(super) fn config() -> &'static [Field] {
    const {
        &[
            Field { key: b"url", read: Read::Text, required: true },
            Field { key: b"content_type", read: Read::Text, required: false },
            Field { key: b"secret", read: Read::Text, required: true },
        ]
    }
}
pub(super) fn text(hook: &mut HookSpec, key: &'static [u8], text: Box<[u8]>) -> Result<(), Malformed> {
    match key {
        b"url" => hook.url = text,
        b"secret" => hook.secret = Secret::new(text),
        b"content_type" if text.as_ref() == b"json" => {}
        b"type" if text.as_ref() == b"forgejo" => {}
        _ => return Err(Malformed::Value { field: key }),
    }
    Ok(())
}
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let target = path::read(bytes, limits).ok().ok_or(Unrouted::Malformed)?;
    let [api, v1, orgs, org, hooks, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if api.as_ref() != b"api" || v1.as_ref() != b"v1" || orgs.as_ref() != b"orgs" || hooks.as_ref() != b"hooks" {
        return Err(Unrouted::NotFound);
    }
    let hook =
        HookSpec { url: Box::from([]), secret: Secret::new(Box::from([])), events: Box::from([]), active: false };
    let request = match (method, tail) {
        (Method::Get, []) => {
            return Ok(Route::Ready(Request::OrgHooks { org: org.clone(), page: page(&target.query)? }));
        }
        (Method::Post, []) => Request::CreateOrgHook { org: org.clone(), hook },
        (Method::Patch, [id]) => Request::EditOrgHook { org: org.clone(), id: number_value(id)?, hook },
        (Method::Delete, [id]) => {
            return Ok(Route::Ready(Request::DeleteOrgHook { org: org.clone(), id: number_value(id)? }));
        }
        (_, [] | [_]) => return Err(Unrouted::Method),
        _ => return Err(Unrouted::NotFound),
    };
    Ok(Route::Body(BodyDecoder {
        walk: Walk::new(Kind::Write(WriteKind::NewHook), limits),
        request,
        texts: List::with_capacity(limits.inner),
        numbers: List::with_capacity(0),
        inputs: List::with_capacity(0),
        terminal: false,
        committer: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
    }))
}
pub(super) fn write(json: &mut Encoder, answer: &Answer) {
    if let Answer::Hook(value) = answer {
        hook(json, value);
    } else if let Answer::Hooks(values) = answer {
        json.array_start();
        for value in values {
            hook(json, value);
        }
        json.array_end();
    } else {
        unreachable!("hook answer selection");
    }
}
fn hook(json: &mut Encoder, value: &crate::forgejo::document::Hook) {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    json.key(b"config");
    json.object_start();
    json.key(b"url");
    json.string(&value.url);
    json.key(b"content_type");
    json.string(b"json");
    json.object_end();
    hooks::events(json, &value.events, value.active);
    json.object_end();
}

/// Repository setup paths share the organization hook body and answer codec.
pub(super) fn repository_route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    use crate::forgejo::request::{Grant, Repo};
    let target = path::read(bytes, limits).ok().ok_or(Unrouted::Malformed)?;
    let [api, v1, repos, owner, name, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if api.as_ref() != b"api" || v1.as_ref() != b"v1" || repos.as_ref() != b"repos" {
        return Err(Unrouted::NotFound);
    }
    let at = Repo { owner: owner.clone(), name: name.clone() };
    let hook =
        HookSpec { url: Box::from([]), secret: Secret::new(Box::from([])), events: Box::from([]), active: false };
    let (kind, request) = match tail {
        [collaborators, login] if collaborators.as_ref() == b"collaborators" => {
            if method != Method::Put {
                return Err(Unrouted::Method);
            }
            (WriteKind::Collaborator, Request::AddCollaborator { at, login: login.clone(), permission: Grant::Read })
        }
        [hooks, rest @ ..] if hooks.as_ref() == b"hooks" => {
            let request = match (method, rest) {
                (Method::Get, []) => return Ok(Route::Ready(Request::RepoHooks { at, page: page(&target.query)? })),
                (Method::Post, []) => Request::CreateRepoHook { at, hook },
                (Method::Patch, [id]) => Request::EditRepoHook { at, id: number_value(id)?, hook },
                (Method::Delete, [id]) => {
                    return Ok(Route::Ready(Request::DeleteRepoHook { at, id: number_value(id)? }));
                }
                (_, [] | [_]) => return Err(Unrouted::Method),
                _ => return Err(Unrouted::NotFound),
            };
            (WriteKind::NewHook, request)
        }
        _ => return Err(Unrouted::NotFound),
    };
    Ok(Route::Body(BodyDecoder {
        walk: Walk::new(Kind::Write(kind), limits),
        request,
        texts: List::with_capacity(limits.inner),
        numbers: List::with_capacity(0),
        inputs: List::with_capacity(0),
        terminal: false,
        committer: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
    }))
}
