//! The private route's entrance, continuation and rendering.
use super::{Event, Server, State, close, fail, respond, retire};
use crate::{
    Hosted, Limits, Route, Service, errors,
    git::{self, Call, Name},
    response, service,
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as domain, api};
use skein_http::{Method, server as http};
use skein_json::tokenizer as json;
use skein_lib::{Env, Queue, ReplyTo, bytes, stream::Down};

pub(super) fn entrance(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    call: &http::Call,
    name: Result<Name, git::Unrouted>,
) {
    let name = match name {
        Ok(name) => name,
        Err(git::Unrouted::NotFound | git::Unrouted::Method | git::Unrouted::Malformed) => {
            service.tally.count_store(None);
            server.requests.push(http::Request::Discard);
            fail(server, &env.limits, 404);
            return;
        }
    };
    server.git = Some(name);
    let credential =
        if name.repository() { service.git_user(call.header(b"authorization"), name.writes()) } else { None };
    if name.repository() {
        match credential {
            Some((user, _)) => service.tally.count(Some(Route::Git(name)), Some(user)),
            None => service.tally.count(Some(Route::Git(name)), None),
        }
    } else {
        service.tally.count_store(Some(Route::Git(name)));
    }
    if call.method != Method::Post {
        server.requests.push(http::Request::Discard);
        fail(server, &env.limits, 405);
        return;
    }
    if name.repository() {
        match credential {
            None => {
                server.requests.push(http::Request::Discard);
                fail(server, &env.limits, 401);
                return;
            }
            Some((_, false)) => {
                server.requests.push(http::Request::Discard);
                fail(server, &env.limits, 403);
                return;
            }
            Some((user, true)) => server.user = user,
        }
    }
    server.state = State::Body;
    // A git body uses its wire bound as well as the tokenizer's grammar caps.
    server.json = json::Tokenizer::new(&json_limits(server, &env.limits));
    server.json_started = true;
    server.git_decoder = Some(git::server::decoder(name, &env.limits));
    super::json_down(server, env, json::Request::Next);
}
pub(super) fn json_limits(server: &Server, limits: &Limits) -> json::Limits {
    let mut json = limits.documents.json;
    if server.git.is_some() {
        json.length = json.length.min(limits.documents.body_bytes);
    }
    json
}
pub(super) fn decoded(
    server: &mut Server,
    service: &mut Service,
    hosted: Option<&mut Hosted<'_>>,
    env: &Env<Limits>,
    call: Call,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match &call {
        Call::Objects { commits } => {
            if commits.len() > usize::try_from(service.config.max_response_items).expect("u32 fits") {
                fail(server, &env.limits, 422);
                return;
            }
        }
        Call::Clone { .. }
        | Call::Fetch { .. }
        | Call::Create { .. }
        | Call::Push { .. }
        | Call::Branch { .. }
        | Call::Commit { .. }
        | Call::Merge { .. }
        | Call::CommitTree { .. }
        | Call::Ancestor { .. } => {}
    }
    let (repository, op) = match call {
        Call::Clone { repository } => (repository, api::Op::Git(api::Git::Clone)),
        Call::Fetch { repository, want } => (repository, api::Op::Git(api::Git::Fetch { want })),
        Call::Create { repository, branch, commit } => (repository, api::Op::Git(api::Git::Create { branch, commit })),
        Call::Push { repository, branch, commit, expected } => {
            (repository, api::Op::Git(api::Git::Push { branch, commit, expected }))
        }
        Call::Branch { repository, branch } => (repository, api::Op::Read(api::Read::Branch { branch })),
        call @ (Call::Objects { .. }
        | Call::Commit { .. }
        | Call::Merge { .. }
        | Call::CommitTree { .. }
        | Call::Ancestor { .. }) => {
            let Some(hosted) = hosted else {
                fail(server, &env.limits, 404);
                return;
            };
            let result = git::store::answer(hosted, call, &env.limits, service.config.max_response_items);
            if faults(server, service, env, above, below) {
                return;
            }
            match result {
                Ok(body) => respond(server, response::Reply { status: 200, headers: Box::new([]), body }),
                Err(error) => error_reply(server, env, error),
            }
            return;
        }
    };
    let Ok(id) = service.calls.insert(service::Call { server: server.owner, active: true }) else {
        fail(server, &env.limits, 503);
        return;
    };
    server.call = Some(id);
    server.state = State::Waiting;
    above.push(Event::Domain(domain::Event::Call {
        reply_to: ReplyTo::new(id.token()),
        user: server.user,
        repository,
        op,
    }));
}
pub(super) fn faults(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) -> bool {
    if service.rng.chance(service.config.faults.drop) {
        close(server, service, env, above, below);
        return true;
    }
    if service.rng.chance(service.config.faults.server_error) {
        retire(server, service);
        fail(server, &env.limits, 500);
        return true;
    }
    false
}
pub(super) fn down(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    result: Result<api::Answer, api::Error>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if faults(server, service, env, above, below) {
        return;
    }
    retire(server, service);
    let result = match result {
        Ok(api::Answer::Cloned { default, branches }) => Ok(git::Answer::Cloned { default, branches }),
        Ok(api::Answer::Commit(commit)) => Ok(git::Answer::Commit(commit)),
        Ok(api::Answer::Branch(created)) => Ok(git::Answer::Created(created)),
        Ok(api::Answer::Pushed(pushed)) => Ok(git::Answer::Pushed(pushed)),
        Err(error) => Err(error),
        Ok(
            api::Answer::Located { .. }
            | api::Answer::Items { .. }
            | api::Answer::Item { .. }
            | api::Answer::Pull(_)
            | api::Answer::Pulls(_)
            | api::Answer::Comments(_)
            | api::Answer::Runs(_)
            | api::Answer::Run(_)
            | api::Answer::Jobs(_)
            | api::Answer::Job { .. }
            | api::Answer::PullFiles { .. }
            | api::Answer::Comparison { .. }
            | api::Answer::Checks(_)
            | api::Answer::Protection(_)
            | api::Answer::Settings(_)
            | api::Answer::Collaborators(_)
            | api::Answer::Statuses(_)
            | api::Answer::Permission(_)
            | api::Answer::Comment { .. }
            | api::Answer::Dependencies(_)
            | api::Answer::Labels(_)
            | api::Answer::GitTree { .. }
            | api::Answer::Tree(_)
            | api::Answer::Artifacts(_)
            | api::Answer::Artifact { .. }
            | api::Answer::Commits { .. }
            | api::Answer::File(_)
            | api::Answer::Pages { .. }
            | api::Answer::Page(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Revision(_)
            | api::Answer::Done,
        ) => {
            fail(server, &env.limits, 500);
            return;
        }
    };
    match result {
        Ok(answer) => match git::server::encode(&answer, &env.limits) {
            Ok(body) => respond(server, response::Reply { status: 200, headers: Box::new([]), body }),
            Err(_) => fail(server, &env.limits, 413),
        },
        Err(error) => error_reply(server, env, error),
    }
}
pub(super) fn error_reply(server: &mut Server, env: &Env<Limits>, error: api::Error) {
    let route = Route::Git(server.git.expect("git entrance"));
    let failure = errors::answer(error, route, env.now);
    let mut headers = skein_lib::List::with_capacity(1);
    if let Some(seconds) = failure.retry_after {
        headers
            .push(skein_http::Header {
                name: bytes::copy_of(b"retry-after"),
                value: bytes::copy_of(skein_lib::Decimal::of(seconds).as_bytes()),
            })
            .expect("one header");
    }
    match git::server::error(error, &env.limits) {
        Ok(body) => respond(server, response::Reply { status: failure.status, headers: headers.into_boxed(), body }),
        Err(_) => fail(server, &env.limits, 413),
    }
}
