//! Inferred comment-edit request and answer shapes, unrecorded in live sessions.
use crate::{
    answers::{self, Body},
    auth::Access,
    reads,
    service::{Outcome, Pending, PendingKind, Response, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    forgejo::document::Body as Text,
    github::{document as d, request::Method},
};
use skein_json::Token;
use skein_lib::{ReplyTo, Wall, bytes};
#[derive(Debug)]
pub(crate) struct Route {
    repository: Box<[u8]>,
    id: u64,
}
#[derive(Debug)]
pub(crate) struct Edit {
    repository: Box<[u8]>,
    id: u64,
    user: u64,
    reading: bool,
}
pub(crate) fn route(method: Method, target: &[u8]) -> Option<Route> {
    if method != Method::Patch {
        return None;
    }
    let tail = target.strip_prefix(b"/repos/")?;
    let at = bytes::find(tail, b"/issues/comments/")?;
    let id = skein_forge::forgejo::id::unsigned(tail.get(at.checked_add(17)?..)?)?;
    Some(Route { repository: bytes::copy_of(tail.get(..at)?), id })
}
pub(crate) fn call(service: &Service, route: Route, access: Access, body: &[u8], reply_to: ReplyTo) -> Outcome {
    if let Access::Person(_) = access {
        return Outcome::Reply(answers::refusal(403, crate::texts::PERSON, &service.limits));
    }
    if access.contents() {
        return Outcome::Reply(answers::refusal(403, crate::texts::INTEGRATION, &service.limits));
    }
    let mut reachable = false;
    for (id, row) in &service.repositories {
        if let Some(full) = reads::full(row)
            && full == route.repository
            && access.reaches(service, *id)
        {
            reachable = true;
        }
    }
    if !reachable {
        return Outcome::Reply(answers::missing(&service.limits));
    }
    let Some(body) = decode(service, body) else {
        return Outcome::Reply(answers::refusal(422, b"Validation Failed", &service.limits));
    };
    let edit = Edit { repository: route.repository, id: route.id, user: access.user(), reading: false };
    call_domain(edit, a::Op::Write(a::Write::EditComment { id: route.id, body }), reply_to)
}
fn decode(service: &Service, body: &[u8]) -> Option<Box<[u8]>> {
    if !service.body_fits(body) {
        return None;
    }
    let tokens = crate::manifest::tokenize(body, &service.limits.github)?;
    if tokens.len() != 4 || tokens.get(0)? != &Token::ObjectStart || tokens.get(3)? != &Token::ObjectEnd {
        return None;
    }
    let Token::Key(key) = tokens.get(1)? else { return None };
    if key.as_ref() != b"body" {
        return None;
    }
    let Token::String(body) = tokens.get(2)? else { return None };
    Some(body.clone())
}
fn call_domain(edit: Edit, op: a::Op, reply_to: ReplyTo) -> Outcome {
    let event = fake::Event::Call { reply_to, user: edit.user, repository: edit.repository.clone(), op };
    Outcome::Call(Pending { rate: None, kind: PendingKind::CommentEdit(edit) }, event)
}
pub(crate) fn finish(
    service: &Service,
    mut edit: Edit,
    result: Result<a::Answer, a::Error>,
    reply_to: ReplyTo,
) -> Outcome {
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => return Outcome::Reply(crate::writes::refused_mutation(error, &service.limits)),
    };
    if !edit.reading {
        let a::Answer::Done = answer else { return Outcome::Reply(answers::failed()) };
        edit.reading = true;
        let id = edit.id;
        return call_domain(edit, a::Op::Read(a::Read::Comment { id }), reply_to);
    }
    Outcome::Reply(render(service, answer))
}
fn render(service: &Service, answer: a::Answer) -> Response {
    let a::Answer::Comment { comment, .. } = answer else { return answers::failed() };
    let created = Wall::from_nanos(comment.created.as_nanos());
    let updated = match comment.edited {
        Some(at) => Wall::from_nanos(at.as_nanos()),
        None => created,
    };
    let row = d::Comment {
        id: comment.id,
        author: comment.author,
        body: Text::Kept(comment.body),
        created_at: Some(created),
        updated_at: Some(updated),
    };
    let mut response = answers::response(200, &Body::Comments(&[row]), &service.limits);
    let Some(single) = response.body.strip_prefix(b"[") else { return answers::failed() };
    let Some(single) = single.strip_suffix(b"]") else { return answers::failed() };
    response.body = bytes::copy_of(single);
    response
}
