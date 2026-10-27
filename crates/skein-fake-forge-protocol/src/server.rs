//! One bounded HTTP connection over Forgejo documents and the fake's calls.
mod git;
use crate::{Error, Hosted, Limits, Service, errors, response, routes, service::Call};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as domain, api};
use skein_forge::forgejo::{
    document,
    failure::Refusal,
    request::{self, Request},
    server as documents,
};
use skein_http::{Header, Method, server as http};
use skein_json::tokenizer::{self as json, Tokenizer};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Env, Id, List, Queue, ReplyTo, Token, bytes};
pub const MAX_UP: u32 = 3;
pub const MAX_DOWN: u32 = 18;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    New,
    Reading,
    Receiving,
    Body,
    Waiting,
    Streaming,
    Closing,
    Closed,
}
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    Domain(domain::Event),
    Close,
    Closed,
}
#[expect(clippy::struct_excessive_bools, reason = "independent Forgejo scope grants and the request write mode")]
struct Scopes {
    user: bool,
    repository: bool,
    admin: bool,
    organization: bool,
    writing: bool,
}
#[expect(missing_debug_implementations, reason = "HTTP queues temporarily hold authorization headers")]
pub struct Server {
    owner: Token,
    state: State,
    http: http::Server,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    json: Tokenizer,
    json_started: bool,
    json_events: Queue<json::Event>,
    json_down: Queue<Down>,
    decoder: Option<documents::BodyDecoder>,
    git: Option<crate::git::Name>,
    git_decoder: Option<crate::git::server::BodyDecoder>,
    user: u64,
    scopes: Scopes,
    request: Option<Request>,
    select: response::Select,
    range: Result<Option<response::Range>, api::Error>,
    call: Option<Id<Call>>,
    followed: bool,
    listing: Option<crate::setup::repos::Listing>,
    head: Option<http::Call>,
    reply: Option<Box<[u8]>>,
    offset: usize,
    send: u32,
}
impl Server {
    pub fn new(owner: Token, limits: &Limits) -> Result<Self, Error> {
        if http::worst_case(&limits.http).is_none()
            || limits.calls == 0
            || limits.connections == 0
            || limits.raw_bytes == 0
            || json::largest_demand(&limits.documents.json) > limits.http.read
            || skein_forge::forgejo::limits::check(&limits.documents).is_err()
        {
            return Err(Error::Limits);
        }
        Ok(Self {
            owner,
            state: State::New,
            http: http::Server::new(&limits.http),
            events: Queue::with_capacity(4),
            requests: Queue::with_capacity(4),
            json: Tokenizer::new(&limits.documents.json),
            json_started: false,
            json_events: Queue::with_capacity(1),
            json_down: Queue::with_capacity(1),
            decoder: None,
            git: None,
            git_decoder: None,
            user: 0,
            scopes: Scopes { user: false, repository: false, admin: false, organization: false, writing: false },
            request: None,
            select: response::Select::Document,
            range: Ok(None),
            call: None,
            followed: false,
            listing: None,
            head: None,
            reply: None,
            offset: 0,
            send: limits.http.send,
        })
    }
    /// Takes the sanitized head captured at the last request entrance.
    pub fn take_head(&mut self) -> Option<http::Call> {
        self.head.take()
    }
    /// Whether no accepted request remains to be answered on this connection.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.state == State::Reading
    }
    #[must_use]
    pub fn has_work(&self) -> bool {
        !self.events.is_empty()
            || !self.requests.is_empty()
            || !self.json_events.is_empty()
            || !self.json_down.is_empty()
    }
}
pub fn start(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state != State::New {
        return;
    }
    next(server, service, env, above, below);
    resume(server, service, env, above, below);
}
pub fn up(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    event: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    up_on(server, service, None, env, event, above, below);
}
/// Serves bytes with setup collaborator grants applied to the lent domain before replying.
pub fn up_with_domain(
    server: &mut Server,
    service: &mut Service,
    domain: Hosted<'_>,
    env: &Env<Limits>,
    event: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    up_on(server, service, Some(domain), env, event, above, below);
}
fn up_on(
    server: &mut Server,
    service: &mut Service,
    domain: Option<Hosted<'_>>,
    env: &Env<Limits>,
    event: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match server.state {
        State::New | State::Closing | State::Closed => return,
        State::Reading | State::Receiving | State::Body | State::Waiting | State::Streaming => {}
    }
    if server.state == State::Reading
        && let Up::Bytes(_) = &event
    {
        server.state = State::Receiving;
    }
    http::up(
        &mut server.http,
        &Env { now: env.now, wall: env.wall, limits: env.limits.http },
        event,
        &mut server.events,
        below,
    );
    resume_on(server, service, domain, env, above, below);
}
pub fn down(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    reply: domain::Request,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    let (to, result) = match reply {
        domain::Request::Reply { to, result } => (to, result),
        domain::Request::Hook { .. } => return,
    };
    let id = Id::<Call>::from_token(to.into_token());
    if server.state != State::Waiting || server.call != Some(id) {
        return;
    }
    let Some(call) = service.calls.get(id) else {
        return;
    };
    if !call.active || call.server != server.owner {
        return;
    }
    if server.listing.is_some() {
        match result {
            Ok(api::Answer::Permission(permission)) => {
                let accepted = match &mut server.listing {
                    Some(listing) => listing.accept(&service.setup, &service.config, server.user, permission),
                    None => Err(Error::Limits),
                };
                if accepted.is_err() {
                    server.listing = None;
                    retire(server, service);
                    fail(server, &env.limits, 503);
                } else {
                    user_repos_next(server, service, env, above);
                }
            }
            Ok(answer) => {
                drop(answer);
                server.listing = None;
                retire(server, service);
                fail(server, &env.limits, 500);
            }
            Err(error) => {
                server.listing = None;
                retire(server, service);
                error_reply(server, env, error);
            }
        }
        resume(server, service, env, above, below);
        return;
    }
    if server.git.is_some() {
        git::down(server, service, env, result, above, below);
        resume(server, service, env, above, below);
        return;
    }
    let request = server.request.as_ref().expect("waiting request");
    if !server.followed {
        if service.rng.chance(service.config.faults.drop) {
            close(server, service, env, above, below);
            return;
        }
        if service.rng.chance(service.config.faults.server_error) {
            retire(server, service);
            fail(server, &env.limits, 500);
            resume(server, service, env, above, below);
            return;
        }
        if let Ok(answer) = &result {
            match response::follow(request, answer) {
                Ok(Some((read, select))) => {
                    server.select = select;
                    server.followed = true;
                    above.push(Event::Domain(domain::Event::Call {
                        reply_to: ReplyTo::new(id.token()),
                        user: server.user,
                        repository: routes::repository(&service.config, request).expect("call repository"),
                        op: api::Op::Read(read),
                    }));
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    retire(server, service);
                    error_reply(server, env, error);
                    resume(server, service, env, above, below);
                    return;
                }
            }
        }
    }
    if result == Err(api::Error::Timeout) && service.rng.chance(500) {
        close(server, service, env, above, below);
        return;
    }
    let reply = match result {
        Ok(answer) => response::answer(&service.config, &service.setup, &env.limits, request, &server.select, &answer),
        Err(error) => Err(error),
    };
    retire(server, service);
    match reply {
        Ok(reply) => respond(server, reply),
        Err(error) => error_reply(server, env, error),
    }
    resume(server, service, env, above, below);
}
pub fn resume(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    resume_on(server, service, None, env, above, below);
}
/// Resumes a pass with the same setup grant crossing as `up_with_domain`.
pub fn resume_with_domain(
    server: &mut Server,
    service: &mut Service,
    domain: Hosted<'_>,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    resume_on(server, service, Some(domain), env, above, below);
}
fn resume_on(
    server: &mut Server,
    service: &mut Service,
    mut domain: Option<Hosted<'_>>,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    for _ in 0..8u32 {
        if above.room() < 2 || below.room() < 2 {
            break;
        }
        if let Some(event) = server.events.pop() {
            http_event(server, service, domain.as_mut(), env, event, above, below);
        } else if let Some(event) = server.json_events.pop() {
            if let Some(decoder) = &mut server.git_decoder {
                match crate::git::server::body(decoder, &env.limits, &event) {
                    crate::git::server::BodyStep::Demand(request) => json_down(server, env, request),
                    crate::git::server::BodyStep::Decoded(call) => {
                        server.git_decoder = None;
                        git::decoded(server, service, domain.as_mut(), env, call, above, below);
                    }
                    crate::git::server::BodyStep::Malformed => {
                        server.git_decoder = None;
                        malformed(server, env);
                    }
                }
                continue;
            }
            let Some(decoder) = &mut server.decoder else {
                continue;
            };
            match documents::body(decoder, &env.limits.documents, &event) {
                documents::BodyStep::Demand(request) => json_down(server, env, request),
                documents::BodyStep::Decoded(request) => {
                    server.decoder = None;
                    decoded(server, service, domain.as_mut(), env, request, above);
                }
                documents::BodyStep::Malformed(_) => {
                    server.decoder = None;
                    malformed(server, env);
                }
            }
        } else if let Some(down) = server.json_down.pop() {
            http_down(server, env, http::Request::Body(down), below);
        } else if let Some(request) = server.requests.pop() {
            http_down(server, env, request, below);
        } else {
            break;
        }
    }
}
pub fn close(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closing || server.state == State::Closed {
        return;
    }
    retire(server, service);
    server.listing = None;
    server.request = None;
    server.reply = None;
    server.decoder = None;
    server.git_decoder = None;
    for _ in 0..server.events.capacity() {
        drop(server.events.pop());
    }
    for _ in 0..server.requests.capacity() {
        drop(server.requests.pop());
    }
    drop(server.json_events.pop());
    drop(server.json_down.pop());
    json_down(server, env, json::Request::Close);
    drop(server.json_events.pop());
    drop(server.json_down.pop());
    http_down(server, env, http::Request::Close, below);
    server.state = State::Closing;
    above.push(Event::Close);
}
pub fn closed(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closed {
        return;
    }
    if server.state != State::Closing {
        close(server, service, env, above, below);
    }
    server.state = State::Closed;
    above.push(Event::Closed);
}
fn retire(server: &mut Server, service: &mut Service) {
    if let Some(id) = server.call.take() {
        if let Some(call) = service.calls.get_mut(id) {
            call.active = false;
        }
        service.calls.retire(id);
    }
}
fn next(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if service.rng.chance(service.config.faults.reset) {
        close(server, service, env, above, below);
    } else {
        server.state = State::Reading;
        server.requests.push(http::Request::Next);
    }
}
fn http_down(server: &mut Server, env: &Env<Limits>, request: http::Request, below: &mut Queue<Down>) {
    http::down(
        &mut server.http,
        &Env { now: env.now, wall: env.wall, limits: env.limits.http },
        request,
        &mut server.events,
        below,
    );
}
fn json_down(server: &mut Server, env: &Env<Limits>, request: json::Request) {
    let limits = git::json_limits(server, &env.limits);
    json::down(
        &mut server.json,
        &Env { now: env.now, wall: env.wall, limits },
        request,
        &mut server.json_events,
        &mut server.json_down,
    );
}
fn http_event(
    server: &mut Server,
    service: &mut Service,
    domain: Option<&mut Hosted<'_>>,
    env: &Env<Limits>,
    event: http::Event,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match event {
        http::Event::Call(call) => entrance(server, service, domain, env, call, above),
        http::Event::Body(up) => {
            if server.state == State::Body {
                let limits = git::json_limits(server, &env.limits);
                json::up(
                    &mut server.json,
                    &Env { now: env.now, wall: env.wall, limits },
                    up,
                    &mut server.json_events,
                    &mut server.json_down,
                );
            }
        }
        http::Event::Reply(Up::Room) => {
            let reply = server.reply.as_ref().expect("reply body");
            let end =
                server.offset.saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits")).min(reply.len());
            let chunk = bytes::copy_of(reply.get(server.offset..end).expect("reply slice"));
            server.offset = end;
            server.requests.push(http::Request::Reply(Down::Send(chunk)));
            if end == reply.len() {
                server.reply = None;
                server.requests.push(http::Request::Reply(Down::Finish));
            } else {
                room(server, &env.limits);
            }
        }
        http::Event::Reply(Up::Bytes(_) | Up::End | Up::Failed(_))
        | http::Event::Refused(_)
        | http::Event::Failed(_)
        | http::Event::Ended => close(server, service, env, above, below),
        http::Event::Done(reuse) => {
            server.git = None;
            server.git_decoder = None;
            server.listing = None;
            server.request = None;
            server.select = response::Select::Document;
            match reuse {
                http::Reuse::Keep => next(server, service, env, above, below),
                http::Reuse::Close => close(server, service, env, above, below),
            }
        }
        http::Event::Closed => {}
    }
}
fn entrance(
    server: &mut Server,
    service: &mut Service,
    domain: Option<&mut Hosted<'_>>,
    env: &Env<Limits>,
    call: http::Call,
    above: &mut Queue<Event>,
) {
    assert!(server.head.is_none(), "the owner drains each captured head before reuse");
    let mut snapshot = call.clone();
    for header in &mut snapshot.headers {
        if header.is(b"authorization") {
            header.value = Box::new([]);
        }
    }
    server.head = Some(snapshot);
    if let Some(name) = crate::git::name(&call.target) {
        git::entrance(server, service, env, &call, name);
        return;
    }
    let user = service.user(call.header(b"authorization"));
    service.tally.count(None, user);
    server.range = response::range(call.header(b"range"));
    let Some(user) = user else {
        server.requests.push(http::Request::Discard);
        fail(server, &env.limits, 401);
        return;
    };
    server.user = user;
    server.scopes = Scopes {
        user: service.reads_user(call.header(b"authorization")),
        admin: service.accesses_admin(call.header(b"authorization"), call.method != Method::Get),
        organization: service.accesses_organization(call.header(b"authorization"), call.method != Method::Get),
        writing: call.method != Method::Get,
        repository: service.accesses_repository(call.header(b"authorization"), call.method != Method::Get),
    };
    let method = match call.method {
        Method::Get => request::Method::Get,
        Method::Post => request::Method::Post,
        Method::Put => request::Method::Put,
        Method::Patch => request::Method::Patch,
        Method::Delete => request::Method::Delete,
        Method::Head | Method::Options => {
            server.requests.push(http::Request::Discard);
            fail(server, &env.limits, 405);
            return;
        }
    };
    match documents::route(method, &call.target, &env.limits.documents) {
        Ok(documents::Route::Ready(request)) => {
            server.requests.push(http::Request::Discard);
            decoded(server, service, domain, env, request, above);
        }
        Ok(documents::Route::Body(decoder)) => {
            server.state = State::Body;
            if server.json_started {
                server.json.restart();
            }
            server.json_started = true;
            let first = documents::body_first(&decoder);
            server.decoder = Some(decoder);
            json_down(server, env, first);
        }
        Err(error) => {
            server.requests.push(http::Request::Discard);
            fail(
                server,
                &env.limits,
                match error {
                    documents::Unrouted::NotFound => 404,
                    documents::Unrouted::Method => 405,
                    documents::Unrouted::Malformed => 422,
                },
            );
        }
    }
}
fn decoded(
    server: &mut Server,
    service: &mut Service,
    domain: Option<&mut Hosted<'_>>,
    env: &Env<Limits>,
    request: Request,
    above: &mut Queue<Event>,
) {
    let route = crate::route(&request);
    service.tally.unrouted = service.tally.unrouted.saturating_sub(1);
    let count = service.tally.routes.get(&route).copied().unwrap_or(0).saturating_add(1);
    service.tally.routes.insert(route, count).expect("route capacity");
    server.followed = false;
    if let Request::Merge { style, .. } = &request
        && *style != document::MergeStyle::Squash
        && *style != document::MergeStyle::FastForwardOnly
    {
        respond(server, response::failure(&env.limits, errors::refusal(Refusal::MergeStyle)));
        return;
    }
    if let Some(message) = missing_scope(&request, &server.scopes) {
        respond(server, response::failure(&env.limits, errors::Failure { status: 403, message, retry_after: None }));
        return;
    }
    if let Some(reply) = service.setup.answer_with_domain(&request, server.user, &mut service.config, env, domain) {
        respond(server, reply);
        return;
    }
    if let Some(answer) = table(service, &request, server.user) {
        match documents::encode(&answer, &env.limits.documents) {
            Ok(body) => respond(server, response::Reply { status: 200, headers: Box::from([]), body }),
            Err(_) => fail(server, &env.limits, 413),
        }
        return;
    }
    if let Request::UserRepos { page } = &request {
        let Ok(id) = service.calls.insert(Call { server: server.owner, active: true }) else {
            fail(server, &env.limits, 503);
            return;
        };
        server.call = Some(id);
        server.listing = Some(crate::setup::repos::Listing::new(*page, &service.config));
        server.request = Some(request);
        server.state = State::Waiting;
        user_repos_next(server, service, env, above);
        return;
    }
    let op = routes::op(&service.config, &request);
    let select = match server.range {
        Ok(range) => response::select(&service.config, &request, range),
        Err(error) => Err(error),
    };
    let repository = routes::repository(&service.config, &request);
    server.request = Some(request);
    match (op, select, repository) {
        (Ok(op), Ok(select), Some(repository)) => {
            let Ok(id) = service.calls.insert(Call { server: server.owner, active: true }) else {
                fail(server, &env.limits, 503);
                return;
            };
            server.call = Some(id);
            server.select = select;
            server.state = State::Waiting;
            above.push(Event::Domain(domain::Event::Call {
                reply_to: ReplyTo::new(id.token()),
                user: server.user,
                repository,
                op,
            }));
        }
        (Err(error), _, _) | (_, Err(error), _) => error_reply(server, env, error),
        (Ok(_), Ok(_), None) => error_reply(server, env, api::Error::Missing(api::What::Repository)),
    }
}
fn missing_scope(request: &Request, scopes: &Scopes) -> Option<&'static [u8]> {
    let (granted, read, write): (bool, &[u8], &[u8]) = match request {
        Request::Repository { .. }
        | Request::Branch { .. }
        | Request::Raw { .. }
        | Request::CreateFile { .. }
        | Request::RepositoryById { .. }
        | Request::RepoSearch { .. }
        | Request::UserRepos { .. }
        | Request::Permission { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. } => (
            scopes.repository,
            b"token does not have at least one of required scope(s): [read:repository]",
            b"token does not have at least one of required scope(s): [write:repository]",
        ),
        Request::User | Request::UserOf { .. } => (
            scopes.user,
            b"token does not have at least one of required scope(s): [read:user]",
            b"token does not have at least one of required scope(s): [write:user]",
        ),
        Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::CreateUser { .. }
        | Request::CreateOrg { .. } => (
            scopes.admin,
            b"token does not have at least one of required scope(s): [read:admin]",
            b"token does not have at least one of required scope(s): [write:admin]",
        ),
        Request::Org { .. } | Request::CreateRepository { .. } => (
            scopes.organization,
            b"token does not have at least one of required scope(s): [read:organization]",
            b"token does not have at least one of required scope(s): [write:organization]",
        ),
        Request::ApiSettings
        | Request::UserSearch { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::Pulls { .. }
        | Request::CombinedStatus { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::AddMember { .. }
        | Request::DeleteBranch { .. } => return None,
    };
    if granted {
        None
    } else if scopes.writing {
        Some(write)
    } else {
        Some(read)
    }
}
fn table(service: &Service, request: &Request, user: u64) -> Option<documents::Answer> {
    match request {
        Request::User => Some(documents::Answer::User(service.setup.resolve_user(&service.config, user))),
        Request::ApiSettings => Some(documents::Answer::ApiSettings(document::ApiSettings {
            max_response_items: service.config.max_response_items,
            default_paging_num: service.config.default_paging_num,
        })),
        Request::UserSearch { uid, .. } => Some(documents::Answer::Search {
            ok: true,
            data: Box::new([service.setup.resolve_user(&service.config, *uid)]),
        }),
        Request::RepoSearch { .. }
        | Request::UserRepos { .. }
        | Request::RepositoryById { .. }
        | Request::Repository { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::Pulls { .. }
        | Request::CombinedStatus { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::Org { .. }
        | Request::CreateOrg { .. }
        | Request::CreateFile { .. }
        | Request::CreateRepository { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::AddMember { .. }
        | Request::CreateUser { .. }
        | Request::DeleteBranch { .. } => None,
    }
}
fn fail(server: &mut Server, limits: &Limits, status: u16) {
    respond(server, response::failure(limits, errors::Failure { status, message: b"", retry_after: None }));
}
fn error_reply(server: &mut Server, env: &Env<Limits>, error: api::Error) {
    let route = crate::route(server.request.as_ref().expect("decoded request"));
    respond(server, response::failure(&env.limits, errors::answer(error, route, env.now)));
}
fn respond(server: &mut Server, reply: response::Reply) {
    let mut headers =
        List::with_capacity(u32::try_from(reply.headers.len()).expect("bounded headers").saturating_add(1));
    for header in reply.headers {
        headers.push(header).expect("response headers");
    }
    if !reply.body.is_empty() {
        headers
            .push(Header {
                name: bytes::copy_of(b"content-type"),
                value: bytes::copy_of(match server.select {
                    response::Select::Raw(_) => b"text/plain",
                    response::Select::Document
                    | response::Select::Permission(_)
                    | response::Select::Branch(_)
                    | response::Select::Branches(_)
                    | response::Select::Review(_)
                    | response::Select::Status(_) => b"application/json",
                }),
            })
            .expect("content type");
    }
    server.state = State::Streaming;
    server.offset = 0;
    let length = u64::try_from(reply.body.len()).expect("bounded body");
    server.reply = if length == 0 { None } else { Some(reply.body) };
    server.requests.push(http::Request::Respond(http::Response {
        status: reply.status,
        headers: headers.into_boxed(),
        body: if reply.status == 204 { http::Body::None } else { http::Body::Length(length) },
        close: false,
    }));
    if length == 0 && reply.status != 204 {
        server.requests.push(http::Request::Reply(Down::Finish));
    } else if length > 0 {
        server.requests.push(http::Request::Reply(Down::Demand {
            read: Read::Nothing,
            room: u32::try_from(length).unwrap_or(u32::MAX).min(server.send),
        }));
    }
}
fn room(server: &mut Server, limits: &Limits) {
    let remaining = server.reply.as_ref().expect("reply").len().saturating_sub(server.offset);
    server.requests.push(http::Request::Reply(Down::Demand {
        read: Read::Nothing,
        room: u32::try_from(remaining).unwrap_or(u32::MAX).min(limits.http.send),
    }));
}

fn user_repos_next(server: &mut Server, service: &mut Service, env: &Env<Limits>, above: &mut Queue<Event>) {
    let Some(listing) = &mut server.listing else {
        return;
    };
    if let Some(repo) = listing.next(&service.config) {
        let Some(id) = server.call else {
            return;
        };
        listing.current = Some(repo.id);
        above.push(Event::Domain(domain::Event::Call {
            reply_to: ReplyTo::new(id.token()),
            user: server.user,
            repository: repo.full.clone(),
            op: api::Op::Read(api::Read::ListingPermission),
        }));
    } else {
        let reply = listing.reply(&env.limits);
        server.listing = None;
        retire(server, service);
        respond(server, reply);
    }
}

fn malformed(server: &mut Server, env: &Env<Limits>) {
    json_down(server, env, json::Request::Close);
    server.json = Tokenizer::new(&env.limits.documents.json);
    server.json_started = false;
    server.requests.push(http::Request::Discard);
    fail(server, &env.limits, 422);
}
