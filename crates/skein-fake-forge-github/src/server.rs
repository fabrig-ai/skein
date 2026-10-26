//! One bounded HTTP connection over GitHub's API/browser or blob listener.
use crate::{
    answers, pages,
    service::{Limits, Outcome, Pending, Problem, Request, Response, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain as domain;
use skein_fake_forge_protocol::{Hosted, git};
use skein_forge::github::request::Method;
use skein_http::{Header, Method as HttpMethod, server as http};
use skein_lib::{
    Env, Intake, List, Queue, ReplyTo, Token, bytes,
    stream::{Down, Read, Up},
};

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
#[derive(Debug)]
pub enum Event {
    Domain(domain::Event),
    Rate { key: domain::RateKey, limit: u32, window: skein_lib::Duration },
    Close,
    Closed,
}
#[expect(missing_debug_implementations, reason = "HTTP heads and body intake can contain credentials")]
pub struct Server {
    owner: Token,
    blob: bool,
    first_byte: bool,
    issue_write: bool,
    state: State,
    http: http::Server,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    call: Option<http::Call>,
    head: Option<http::Call>,
    body: Intake,
    pending: Option<Pending>,
    admission: Option<crate::rates::Admission>,
    reply: Option<Box<[u8]>>,
    offset: usize,
}
impl Server {
    pub fn new(owner: Token, limits: &Limits) -> Result<Self, Problem> {
        worst_case(limits).ok_or(Problem::Limits)?;
        Ok(Self {
            owner,
            blob: false,
            first_byte: false,
            issue_write: false,
            state: State::New,
            http: http::Server::new(&limits.http),
            events: Queue::with_capacity(4),
            requests: Queue::with_capacity(4),
            call: None,
            head: None,
            body: Intake::with_capacity(limits.body),
            pending: None,
            admission: None,
            reply: None,
            offset: 0,
        })
    }
    /// A connection on the peer's credential-free blob listener.
    pub fn blob(owner: Token, limits: &Limits) -> Result<Self, Problem> {
        let mut server = Self::new(owner, limits)?;
        server.blob = true;
        Ok(server)
    }
    /// Takes the head with every Authorization value removed.
    pub fn take_head(&mut self) -> Option<http::Call> {
        self.head.take()
    }
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.state == State::Reading
    }
    #[must_use]
    pub fn has_work(&self) -> bool {
        !self.events.is_empty() || !self.requests.is_empty()
    }
}
/// One connection's queues, heads, intake, decoded continuation fields and responses.
/// Domain answers moved into continuations remain in the host's domain-reply budget.
/// The shared service counts decoding scratch, including reviewer login/id lists.
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    if limits.body == 0 || limits.response < 128 {
        return None;
    }
    http::worst_case(&limits.http)?
        .checked_add(Queue::<http::Event>::worst_case(4)?)?
        .checked_add(Queue::<http::Request>::worst_case(4)?)?
        // One Respond owns rate fields, a validator, Link, Location and media.
        .checked_add(List::<Header>::worst_case(11)?)?
        // A call moves from events into call; its redacted clone lives in head.
        // One exchange at a time and take_head before reuse allow two field lists
        // and two copies of their target/name/value bytes, each within http.head.
        .checked_add(List::<Header>::worst_case(limits.http.headers)?.checked_mul(2)?)?
        .checked_add(u64::from(limits.http.head).checked_mul(2)?)?
        .checked_add(Intake::worst_case(limits.body)?)?
        .checked_add(crate::git::worst_case(limits)?)?
        // A conditional repository read retains its admission and a quoted digest.
        .checked_add(u64::try_from(size_of::<crate::rates::Admission>()).ok()?)?
        .checked_add(u64::try_from(crate::reads::REPOSITORY_ETAG_BYTES).ok()?)?
        // dispatch's extracted body and the authenticated admission copy, or
        // writes::Stage::ReviewAuthor.body, coexist; service counts decoding.
        .checked_add(u64::from(limits.body).checked_mul(2)?)?
        // One read continuation's target (Checks, Reviews, Collaborators,
        // PullFiles, Issues, Comments, Branches) or Branch's decoded name.
        .checked_add(u64::from(limits.github.target_bytes).checked_mul(2)?)?
        .checked_add(1)?
        // writes::Pending.repository plus writes::Stage::OpenPull's head and base.
        // Service validates the repository's full URL within string_bytes, so
        // its owner/name path fits one. Read::Repository's Row has two strings.
        .checked_add(u64::from(limits.github.string_bytes).checked_mul(3)?)?
        // Server.reply and Respond.headers' Link (API) or Location (page/log).
        // The response head encoded by HTTP is in http::worst_case above.
        .checked_add(u64::from(limits.response).checked_mul(2)?)?
        // Bounded decimal rate values, their field names, validator framing,
        // Link, Location and the longest media name/value fit this allowance.
        .checked_add(576)?
        // One outstanding body demand: one queued/local Body(Bytes) delivery.
        .checked_add(u64::from(limits.http.read))?
        // One reply room grant: one copied chunk in requests' Reply(Send).
        .checked_add(u64::from(limits.http.send))
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
    up_inner(server, service, env, event, None, above, below);
}
/// Receives bytes while borrowing the domain for private git store calls.
pub fn up_with_domain(
    server: &mut Server,
    service: &mut Service,
    hosted: Hosted<'_>,
    env: &Env<Limits>,
    event: Up,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    up_inner(server, service, env, event, Some(hosted), above, below);
}
fn up_inner(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    event: Up,
    hosted: Option<Hosted<'_>>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match server.state {
        State::New | State::Closing | State::Closed => return,
        State::Reading | State::Receiving | State::Body | State::Waiting | State::Streaming => {}
    }
    match &event {
        Up::Bytes(_) => {
            if server.state == State::Reading {
                server.state = State::Receiving;
            }
        }
        Up::Room | Up::End | Up::Failed(_) => {}
    }
    http::up(
        &mut server.http,
        &Env { now: env.now, wall: env.wall, limits: env.limits.http },
        event,
        &mut server.events,
        below,
    );
    resume_inner(server, service, env, hosted, above, below);
}
/// The host routes only this connection's reply and lends the shared domain for follow-up reads.
pub fn down(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    reply: domain::Request,
    domain: &domain::Domain,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    let (to, result) = match reply {
        domain::Request::Reply { to, result } => (to, result),
        domain::Request::Hook { .. } => return,
    };
    if to.into_token() != server.owner || server.state != State::Waiting {
        return;
    }
    let Some(pending) = server.pending.take() else { return };
    let (outcome, git_pending) = match pending.kind {
        crate::service::PendingKind::Git(pending) => {
            let response = crate::git::render(pending.name, result, &env.limits);
            (Outcome::Reply(response), Some(pending))
        }
        kind @ (crate::service::PendingKind::Mutation(_)
        | crate::service::PendingKind::CommentEdit(_)
        | crate::service::PendingKind::Read(_)
        | crate::service::PendingKind::Enqueue(_)
        | crate::service::PendingKind::Conditional { .. }
        | crate::service::PendingKind::Write(_)
        | crate::service::PendingKind::Blob
        | crate::service::PendingKind::EditReach { .. }) => {
            (service.finish(Pending { kind, rate: pending.rate }, result, ReplyTo::new(server.owner), domain), None)
        }
    };
    // A request's reads and write continuations finish before its answer is lost.
    // Drawing here keeps a merge's preliminary reads from losing its write.
    let outcome = match outcome {
        call @ (Outcome::Call(_, _) | Outcome::Admit(_)) => call,
        Outcome::Reply(reply) => {
            if service.rng.chance(service.faults.drop) {
                if let Some(pending) = &git_pending {
                    service.record_git(pending, reply.status);
                }
                close(server, service, env, above, below);
                return;
            }
            Outcome::Reply(if service.rng.chance(service.faults.server_error) {
                let mut failed = answers::failed();
                failed.rate = reply.rate;
                failed
            } else {
                reply
            })
        }
    };
    if let Some(pending) = git_pending {
        match &outcome {
            Outcome::Reply(response) => service.record_git(&pending, response.status),
            Outcome::Call(_, _) | Outcome::Admit(_) => unreachable!("git finishes with one reply"),
        }
    }
    answer(server, service, &env.limits, outcome, above);
    resume(server, service, env, above, below);
}
/// Returns the one domain-window admission for this connection.
pub fn admitted(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    result: Result<domain::Rate, domain::RateDenied>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state != State::Waiting {
        return;
    }
    let Some(admission) = server.admission.take() else { return };
    let outcome = service.admitted(admission, result, ReplyTo::new(server.owner));
    answer(server, service, &env.limits, outcome, above);
    resume(server, service, env, above, below);
}
pub fn resume(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    resume_inner(server, service, env, None, above, below);
}
/// Resumes a connection while borrowing the domain for private git store calls.
pub fn resume_with_domain(
    server: &mut Server,
    service: &mut Service,
    hosted: Hosted<'_>,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    resume_inner(server, service, env, Some(hosted), above, below);
}
fn resume_inner(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    mut hosted: Option<Hosted<'_>>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    for _ in 0..8u32 {
        if above.room() < 2 || below.room() < 2 {
            break;
        }
        if let Some(event) = server.events.pop() {
            event_up(server, service, env, event, hosted.as_mut(), above, below);
            // The host records this entrance before another buffered head can arrive.
            if server.head.is_some() {
                break;
            }
        } else if let Some(request) = server.requests.pop() {
            http_down(server, env, request, below);
        } else {
            break;
        }
    }
}
pub fn close(
    server: &mut Server,
    _service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if server.state == State::Closing || server.state == State::Closed {
        return;
    }
    server.pending = None;
    server.admission = None;
    server.call = None;
    server.reply = None;
    server.body.clear();
    for _ in 0..server.events.capacity() {
        drop(server.events.pop());
    }
    for _ in 0..server.requests.capacity() {
        drop(server.requests.pop());
    }
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
    close(server, service, env, above, below);
    server.state = State::Closed;
    above.push(Event::Closed);
}
fn next(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if service.rng.chance(service.faults.reset) {
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
fn demand(server: &mut Server, limits: &Limits) {
    server.requests.push(http::Request::Body(Down::Demand { read: Read::Arrived { max: limits.http.read }, room: 0 }));
}
fn event_up(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    event: http::Event,
    hosted: Option<&mut Hosted<'_>>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match event {
        http::Event::Call(call) => {
            assert!(server.head.is_none(), "owner takes the captured head before reuse");
            let mut snapshot = call.clone();
            for header in &mut snapshot.headers {
                if header.is(b"authorization") {
                    header.value = Box::new([]);
                }
            }
            server.head = Some(snapshot);
            if !server.blob
                && let Some(Ok(name)) = git::name(&call.target)
                && name.repository()
                && let Err(status) = service.git_access(call.header(b"authorization"), env.wall)
            {
                let pending = crate::git::Pending { name, minted: service.git_identity(call.header(b"authorization")) };
                service.record_git(&pending, status);
                server.requests.push(http::Request::Discard);
                respond(server, &env.limits, crate::logs::empty(status), None);
                return;
            }
            server.call = Some(call);
            server.body.clear();
            server.state = State::Body;
            demand(server, &env.limits);
        }
        http::Event::Body(Up::Bytes(chunk)) => {
            if server.body.append(&chunk).is_err() {
                if let Some(call) = &server.call
                    && let Some(Ok(name)) = git::name(&call.target)
                {
                    let pending =
                        crate::git::Pending { name, minted: service.git_identity(call.header(b"authorization")) };
                    service.record_git(&pending, 413);
                }
                server.requests.push(http::Request::Discard);
                respond(server, &env.limits, answers::refusal(413, crate::texts::PAYLOAD_TOO_LARGE, &env.limits), None);
            } else {
                demand(server, &env.limits);
            }
        }
        http::Event::Body(Up::End) => dispatch(server, service, env, hosted, above),
        http::Event::Body(Up::Room) | http::Event::Closed => {}
        http::Event::Reply(Up::Room) => {
            let reply = server.reply.as_ref().expect("reply stream");
            let end =
                server.offset.saturating_add(usize::try_from(env.limits.http.send).expect("u32 fits")).min(reply.len());
            let chunk = bytes::copy_of(reply.get(server.offset..end).expect("bounded chunk"));
            server.offset = end;
            server.requests.push(http::Request::Reply(Down::Send(chunk)));
            if end == reply.len() {
                server.reply = None;
                server.requests.push(http::Request::Reply(Down::Finish));
            } else {
                reply_room(server, &env.limits);
            }
        }
        http::Event::Body(Up::Failed(_))
        | http::Event::Reply(Up::Bytes(_) | Up::End | Up::Failed(_))
        | http::Event::Refused(_)
        | http::Event::Failed(_)
        | http::Event::Ended => close(server, service, env, above, below),
        http::Event::Done(reuse) => {
            server.call = None;
            server.body.clear();
            match reuse {
                http::Reuse::Keep => next(server, service, env, above, below),
                http::Reuse::Close => close(server, service, env, above, below),
            }
        }
    }
}
fn dispatch(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    hosted: Option<&mut Hosted<'_>>,
    above: &mut Queue<Event>,
) {
    let call = server.call.take().expect("accepted head");
    server.issue_write = call.method == HttpMethod::Post && call.target.ends_with(b"/issues");
    if server.blob {
        server.first_byte = call.header(b"range") == Some(b"bytes=0-0");
        let method = if call.method == HttpMethod::Get { Method::Get } else { Method::Post };
        let outcome = service.blob(
            method,
            &call.target,
            call.header(b"authorization").is_some(),
            call.header(b"range").is_some(),
            env.wall,
            ReplyTo::new(server.owner),
        );
        answer(server, service, &env.limits, outcome, above);
        return;
    }
    if let Some(name) = git::name(&call.target) {
        dispatch_git(server, service, env, hosted, &call, name, above);
        return;
    }
    let method = match call.method {
        HttpMethod::Get => Method::Get,
        HttpMethod::Post => Method::Post,
        HttpMethod::Put => Method::Put,
        HttpMethod::Patch => Method::Patch,
        HttpMethod::Delete => Method::Delete,
        HttpMethod::Head => {
            server.state = State::Streaming;
            server.requests.push(http::Request::Respond(http::Response {
                status: 404,
                headers: Box::new([]),
                body: http::Body::None,
                close: false,
            }));
            return;
        }
        HttpMethod::Options => {
            respond(server, &env.limits, answers::missing(&env.limits), None);
            return;
        }
    };
    let body = if server.body.is_empty() {
        Box::new([])
    } else {
        server.body.meet(Read::Fill(server.body.len())).expect("whole body")
    };
    if pages::accepts(method, &call.target) {
        let page = service.page(&pages::Page { method, target: &call.target, body: &body }, env.wall);
        let content_type = match page.media {
            pages::Media::Html => crate::service::ContentType::Html,
            pages::Media::None => crate::service::ContentType::Json,
        };
        respond(
            server,
            &env.limits,
            Response {
                rate: None,
                etag: None,
                status: page.status,
                link: None,
                location: None,
                body: page.body,
                content_type,
            },
            page.location,
        );
    } else {
        let outcome = service.request(
            &Request {
                validator: call.header(b"if-none-match"),
                method,
                target: &call.target,
                body: &body,
                authorization: call.header(b"authorization"),
                version: call.header(b"x-github-api-version"),
                agent: call.header(b"user-agent").is_some(),
            },
            env.wall,
            ReplyTo::new(server.owner),
        );
        answer(server, service, &env.limits, outcome, above);
    }
}
fn dispatch_git(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    hosted: Option<&mut Hosted<'_>>,
    call: &http::Call,
    name: Result<git::Name, git::Unrouted>,
    above: &mut Queue<Event>,
) {
    let body = if server.body.is_empty() {
        Box::new([])
    } else {
        server.body.meet(Read::Fill(server.body.len())).expect("whole body")
    };
    let method = if call.method == HttpMethod::Post { Method::Post } else { Method::Get };
    let outcome = service.git_request(
        &Request {
            validator: None,
            method,
            target: &call.target,
            body: &body,
            authorization: call.header(b"authorization"),
            version: None,
            agent: false,
        },
        name,
        env.wall,
        ReplyTo::new(server.owner),
        hosted,
    );
    answer(server, service, &env.limits, outcome, above);
}
fn answer(server: &mut Server, service: &Service, limits: &Limits, outcome: Outcome, above: &mut Queue<Event>) {
    match outcome {
        Outcome::Admit(admission) => {
            let key = admission.key();
            server.admission = Some(admission);
            server.state = State::Waiting;
            above.push(Event::Rate { key, limit: service.rate_quota(), window: service.rate_window() });
        }
        Outcome::Reply(reply) => respond(server, limits, reply, None),
        Outcome::Call(pending, event) => {
            server.pending = Some(pending);
            server.state = State::Waiting;
            above.push(Event::Domain(event));
        }
    }
}
// http.md, 5.4: these statuses have no reply stream.
fn status_has_no_body(status: u16) -> bool {
    status == 204 || status == 304
}
fn respond(server: &mut Server, limits: &Limits, mut reply: Response, mut location: Option<Box<[u8]>>) {
    if location.is_none() {
        location = reply.location.take();
    }
    let location_bytes = match &location {
        Some(value) => value.len(),
        None => 0,
    };
    if reply.body.len() > usize::try_from(limits.response).expect("u32 fits")
        || location_bytes > usize::try_from(limits.response).expect("u32 fits")
    {
        reply = answers::failed();
        location = None;
    }
    let mut headers = List::with_capacity(11);
    if server.blob && server.first_byte && reply.status == 200 && !reply.body.is_empty() {
        let size = skein_lib::Decimal::of(u64::try_from(reply.body.len()).expect("bounded blob"));
        let value = crate::service::joined(b"bytes 0-0/", size.as_bytes()).expect("range header");
        headers.push(Header { name: bytes::copy_of(b"Content-Range"), value }).expect("range slot");
        reply.status = 206;
        reply.body = bytes::copy_of(reply.body.get(..1).expect("nonempty blob"));
    }
    if !server.blob && reply.status == 201 && server.issue_write {
        headers
            .push(Header {
                name: bytes::copy_of(b"X-Accepted-GitHub-Permissions"),
                value: bytes::copy_of(b"issues=write"),
            })
            .expect("permissions slot");
    }
    if let Some(rate) = reply.rate {
        crate::rates::headers(rate, reply.status == 403, &mut headers);
    }
    if let Some(etag) = reply.etag {
        headers.push(Header { name: bytes::copy_of(b"ETag"), value: etag }).expect("etag slot");
    }
    if let Some(link) = reply.link {
        headers.push(Header { name: bytes::copy_of(b"Link"), value: link }).expect("link slot");
    }
    if let Some(location) = location {
        headers.push(Header { name: bytes::copy_of(b"Location"), value: location }).expect("location slot");
    }
    let bodyless = status_has_no_body(reply.status);
    let length = if bodyless { 0 } else { u64::try_from(reply.body.len()).expect("bounded reply") };
    if length > 0 {
        headers
            .push(Header { name: bytes::copy_of(b"Content-Type"), value: bytes::copy_of(reply.content_type.bytes()) })
            .expect("media slot");
    }
    server.state = State::Streaming;
    server.offset = 0;
    server.reply = if length > 0 { Some(reply.body) } else { None };
    server.requests.push(http::Request::Respond(http::Response {
        status: reply.status,
        headers: headers.into_boxed(),
        body: if bodyless { http::Body::None } else { http::Body::Length(length) },
        close: false,
    }));
    if length > 0 {
        reply_room(server, limits);
    } else if !bodyless {
        server.requests.push(http::Request::Reply(Down::Finish));
    }
}
fn reply_room(server: &mut Server, limits: &Limits) {
    let remaining = server.reply.as_ref().expect("reply body").len().saturating_sub(server.offset);
    server.requests.push(http::Request::Reply(Down::Demand {
        read: Read::Nothing,
        room: u32::try_from(remaining).unwrap_or(u32::MAX).min(limits.http.send),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::fixtures;

    // The body fixture has four bytes; the refusal fixture adds one.
    const BODY_BOUND: u32 = 4;
    fn intake(body: &[u8]) -> (u16, u32) {
        let limits = Limits { body: BODY_BOUND, ..fixtures::limits() };
        let mut server = Server::new(Token::new(1), &limits).expect("bounded HTTP server");
        let mut service = Service::new(fixtures::config(), &limits, 1).expect("bounded service");
        let env = Env { now: skein_lib::Time::ZERO, wall: skein_lib::Wall::EPOCH, limits };
        let mut above = Queue::with_capacity(MAX_UP);
        let mut below = Queue::with_capacity(MAX_DOWN);
        let mut call = http::Call {
            method: HttpMethod::Post,
            version: skein_http::Version::Http11,
            body: http::Body::Length(u64::from(BODY_BOUND)),
            target: bytes::copy_of(b"/unknown"),
            headers: Box::new([]),
        };
        call.headers = Box::new([
            Header { name: bytes::copy_of(b"User-Agent"), value: bytes::copy_of(b"example-app") },
            Header {
                name: bytes::copy_of(b"X-GitHub-Api-Version"),
                value: bytes::copy_of(skein_forge::github::pin::API_VERSION.as_bytes()),
            },
        ]);
        event_up(&mut server, &mut service, &env, http::Event::Call(call), None, &mut above, &mut below);
        drop(server.requests.pop());
        event_up(
            &mut server,
            &mut service,
            &env,
            http::Event::Body(Up::Bytes(bytes::copy_of(body))),
            None,
            &mut above,
            &mut below,
        );
        if server.state == State::Body {
            drop(server.requests.pop());
            event_up(&mut server, &mut service, &env, http::Event::Body(Up::End), None, &mut above, &mut below);
        } else {
            drop(server.requests.pop());
        }
        let Some(http::Request::Respond(response)) = server.requests.pop() else { panic!("HTTP refusal") };
        (response.status, server.body.len())
    }
    #[test]
    fn github_http_intake_accepts_a_body_at_its_bound() {
        assert_eq!(intake(b"body"), (404, 0));
    }
    #[test]
    fn github_http_intake_refuses_one_byte_past_its_bound() {
        let (status, retained) = intake(b"body!");
        assert_eq!(status, 413);
        assert!(retained <= BODY_BOUND);
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;
    use crate::{GitCall, Minted, service::PendingKind, tests::fixtures};

    #[test]
    fn a_git_record_uses_the_status_after_a_server_fault() {
        let mut h = fixtures::Harness::new();
        h.service.faults.server_error = 1000;
        let owner = Token::new(77);
        let mut server = Server::new(owner, &h.limits).expect("fixture HTTP server");
        server.state = State::Waiting;
        let minted = Some(Minted { installation: 1, mint: 1 });
        server.pending =
            Some(Pending { rate: None, kind: PendingKind::Git(crate::git::Pending { name: git::Name::Push, minted }) });
        // Hold HTTP progression so the framed status can be inspected directly.
        let mut above = Queue::with_capacity(0);
        let mut below = Queue::with_capacity(MAX_DOWN);
        down(
            &mut server,
            &mut h.service,
            &Env { now: h.env.now, wall: h.now, limits: h.limits },
            domain::Request::Reply {
                to: ReplyTo::new(owner),
                result: Ok(domain::api::Answer::Pushed(domain::api::Pushed::Pushed)),
            },
            &h.domain,
            &mut above,
            &mut below,
        );
        let Some(http::Request::Respond(response)) = server.requests.pop() else { panic!("framed reply") };
        assert_eq!(response.status, 500);
        assert_eq!(h.service.git_calls().len(), 1);
        assert_eq!(h.service.git_calls().iter().next(), Some(&GitCall { minted, writes: true, status: 500 }));
    }

    #[test]
    fn a_private_git_credential_refusal_precedes_the_body_demand() {
        let mut h = fixtures::Harness::new();
        let mut server = Server::new(Token::new(77), &h.limits).expect("fixture HTTP server");
        let mut above = Queue::with_capacity(MAX_UP);
        let mut below = Queue::with_capacity(MAX_DOWN);
        event_up(
            &mut server,
            &mut h.service,
            &Env { now: h.env.now, wall: h.now, limits: h.limits },
            http::Event::Call(http::Call {
                method: HttpMethod::Post,
                version: skein_http::Version::Http11,
                body: http::Body::Length(u64::from(h.limits.body)),
                target: bytes::copy_of(b"/.fake/git/clone"),
                headers: Box::new([]),
            }),
            None,
            &mut above,
            &mut below,
        );
        let Some(http::Request::Discard) = server.requests.pop() else { panic!("discard refused body") };
        let Some(http::Request::Respond(response)) = server.requests.pop() else { panic!("credential refusal") };
        assert_eq!(response.status, 401);
        assert_eq!(h.service.git_calls().len(), 1);
        let Some(http::Request::Reply(Down::Finish)) = server.requests.pop() else { panic!("empty response finishes") };
        assert!(server.requests.is_empty(), "no body demand");
    }
}
