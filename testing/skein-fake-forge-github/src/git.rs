//! Section 3's private git route with installation credentials (fake-forge.md, 10.1).
use crate::{
    auth::Access,
    service::{Limits, Outcome, Pending as Continuation, PendingKind, Request, Response, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as domain, api};
use skein_fake_forge_protocol::{
    self as protocol,
    git::{self, Call, Name},
};
use skein_forge::github::request::Method;
use skein_json::tokenizer as json;
use skein_lib::{
    Env, Queue, ReplyTo, Time, Wall, bytes,
    stream::{Down, Held},
};

/// One request on the private git route (fake-forge.md 10.1), never its credential.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GitCall {
    /// The token's installation and mint number while its issued entry is retained,
    /// including expired or suspended tokens; none for unknown, revoked, deleted
    /// installation's or reclaimed tokens. No credential is retained.
    pub minted: Option<Minted>,
    /// Whether the call writes (create, push).
    pub writes: bool,
    /// The status the layer answered.
    pub status: u16,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Minted {
    pub installation: u64,
    pub mint: u64,
}
#[derive(Debug)]
pub(crate) struct Pending {
    pub(crate) name: Name,
    pub(crate) minted: Option<Minted>,
}
impl Service {
    #[must_use]
    pub const fn git_calls(&self) -> &Queue<GitCall> {
        &self.git_calls
    }
    pub(crate) fn record_git(&mut self, pending: &Pending, status: u16) {
        if pending.name.repository() {
            if self.git_calls.room() == 0 {
                self.git_calls.pop().expect("full git record has an oldest call");
            }
            self.git_calls.push(GitCall { minted: pending.minted, writes: pending.name.writes(), status });
        }
    }
    pub(crate) fn git_identity(&self, authorization: Option<&[u8]>) -> Option<Minted> {
        let decoded = basic(authorization?)?;
        let colon = bytes::find(&decoded, b":")?;
        self.installations.identity(decoded.get(colon.checked_add(1)?..)?)
    }
    pub(crate) fn git_access(&self, authorization: Option<&[u8]>, now: Wall) -> Result<Access, u16> {
        let authorization = authorization.ok_or(401_u16)?;
        if !authorization.starts_with(b"Basic ") {
            return Err(401);
        }
        let decoded = basic(authorization).ok_or(403_u16)?;
        let colon = bytes::find(&decoded, b":").ok_or(403_u16)?;
        if decoded.get(..colon) != Some(b"x-access-token") {
            return Err(403);
        }
        let at = colon.checked_add(1).ok_or(403_u16)?;
        let token = decoded.get(at..).ok_or(403_u16)?;
        let Some(bearer) = self.resolve(token, now) else {
            return Err(403);
        };
        match self.access(bearer) {
            Ok(access @ Access::Installation { .. }) => Ok(access),
            Ok(Access::Person(_)) | Err(_) => Err(403),
        }
    }
    pub(crate) fn git_request(
        &mut self,
        request: &Request<'_>,
        name: Result<Name, git::Unrouted>,
        now: Wall,
        reply_to: ReplyTo,
        hosted: Option<&mut protocol::Hosted<'_>>,
    ) -> Outcome {
        let Ok(name) = name else {
            return Outcome::Reply(crate::logs::empty(404));
        };
        let pending = Pending { name, minted: self.git_identity(request.authorization) };
        let outcome = self.git_inner(request, &pending, now, reply_to, hosted);
        match &outcome {
            Outcome::Reply(response) => self.record_git(&pending, response.status),
            Outcome::Call(_, _) | Outcome::Admit(_) => {}
        }
        outcome
    }
    fn git_inner(
        &mut self,
        request: &Request<'_>,
        pending: &Pending,
        now: Wall,
        reply_to: ReplyTo,
        hosted: Option<&mut protocol::Hosted<'_>>,
    ) -> Outcome {
        let name = pending.name;
        let access = if name.repository() {
            match self.git_access(request.authorization, now) {
                Ok(access) => Some(access),
                Err(status) => return refused(status),
            }
        } else {
            None
        };
        if request.method != Method::Post {
            return refused(405);
        }
        if !self.body_fits(request.body) {
            return refused(413);
        }
        let limits = limits(&self.limits);
        let Some(call) = decode(name, request.body, &limits) else {
            return refused(400);
        };
        let (repository, op) = match call {
            Call::Clone { repository } => (repository, api::Op::Git(api::Git::Clone)),
            Call::Fetch { repository, want } => (repository, api::Op::Git(api::Git::Fetch { want })),
            Call::Create { repository, branch, commit } => {
                (repository, api::Op::Git(api::Git::Create { branch, commit }))
            }
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
                    return refused(404);
                };
                return Outcome::Reply(match git::store_answer(hosted, call, &limits, self.limits.github.inner) {
                    Ok(body) => {
                        Response { body, content_type: crate::service::ContentType::Json, ..crate::logs::empty(200) }
                    }
                    Err(why) => error(name, why, &limits),
                });
            }
        };
        let Some(access) = access else {
            return refused(403);
        };
        let Some(slash) = bytes::find(&repository, b"/") else {
            return refused(404);
        };
        let owner = repository.get(..slash).expect("found separator");
        let repo_name = repository.get(slash.saturating_add(1)..).expect("found separator");
        let mut reached = false;
        for (id, row) in &self.repositories {
            if row.owner.as_ref() == owner && row.name.as_ref() == repo_name && access.reaches(self, *id) {
                reached = true;
                break;
            }
        }
        if !reached {
            return refused(404);
        }
        // Both minted scopes grant contents write; the domain checks the bot's rights.
        Outcome::Call(
            Continuation { rate: None, kind: PendingKind::Git(Pending { name, minted: pending.minted }) },
            domain::Event::Call { reply_to, user: access.user(), repository, op },
        )
    }
}
fn basic(authorization: &[u8]) -> Option<Box<[u8]>> {
    let value = authorization.strip_prefix(b"Basic ")?;
    skein_lib::base64::decode(skein_lib::base64::Alphabet::Standard, value, u32::try_from(value.len()).ok()?).ok()
}
fn refused(status: u16) -> Outcome {
    Outcome::Reply(crate::logs::empty(status))
}
pub(crate) fn finish(service: &mut Service, pending: Pending, result: Result<api::Answer, api::Error>) -> Response {
    let response = render(pending.name, result, &service.limits);
    service.record_git(&pending, response.status);
    response
}
pub(crate) fn render(name: Name, result: Result<api::Answer, api::Error>, bounds: &Limits) -> Response {
    let limits = limits(bounds);
    let answer = match result {
        Ok(api::Answer::Cloned { default, branches }) => git::Answer::Cloned { default, branches },
        Ok(api::Answer::Commit(commit)) => git::Answer::Commit(commit),
        Ok(api::Answer::Branch(created)) => git::Answer::Created(created),
        Ok(api::Answer::Pushed(pushed)) => git::Answer::Pushed(pushed),
        Err(why) => return error(name, why, &limits),
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
            | api::Answer::Tree(_)
            | api::Answer::File(_)
            | api::Answer::Pages { .. }
            | api::Answer::Page(_)
            | api::Answer::Created(_)
            | api::Answer::Commented(_)
            | api::Answer::Reviewed(_)
            | api::Answer::Merged(_)
            | api::Answer::Revision(_)
            | api::Answer::Done,
        ) => return crate::answers::failed(),
    };
    match git::server::encode(&answer, &limits) {
        Ok(body) => Response { body, content_type: crate::service::ContentType::Json, ..crate::logs::empty(200) },
        Err(_) => crate::logs::empty(413),
    }
}
fn error(name: Name, why: api::Error, limits: &protocol::Limits) -> Response {
    let status = protocol::errors::answer(why, protocol::Route::Git(name), Time::ZERO).status;
    match git::server::error(why, limits) {
        Ok(body) => Response { body, content_type: crate::service::ContentType::Json, ..crate::logs::empty(status) },
        Err(_) => crate::logs::empty(413),
    }
}
pub(crate) fn limits(limits: &Limits) -> protocol::Limits {
    protocol::Limits {
        calls: 1,
        connections: 1,
        http: limits.http,
        raw_bytes: limits.body,
        documents: skein_forge::forgejo::limits::Limits {
            json: json::Limits { length: limits.github.json.length.min(limits.body), ..limits.github.json },
            writer: skein_json::writer::Limits {
                length: limits.github.writer.length.min(limits.response),
                ..limits.github.writer
            },
            string_bytes: limits.github.string_bytes,
            body_bytes: limits.body,
            inner: limits.github.inner,
            target_bytes: limits.github.target_bytes,
        },
    }
}
fn decode(name: Name, body: &[u8], limits: &protocol::Limits) -> Option<Call> {
    let mut decoder = git::server::decoder(name, limits);
    let mut tokenizer = json::Tokenizer::new(&limits.documents.json);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.documents.json };
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut input = Held::new(bytes::copy_of(body));
    json::down(&mut tokenizer, &env, git::server::body_first(&decoder), &mut above, &mut below);
    // At most eight tokenizer/stream transitions per input byte, plus completion.
    for _ in 0..body.len().saturating_mul(8).saturating_add(64) {
        if let Some(event) = above.pop() {
            match git::server::body(&mut decoder, limits, &event) {
                git::server::BodyStep::Demand(request) => {
                    json::down(&mut tokenizer, &env, request, &mut above, &mut below);
                }
                git::server::BodyStep::Decoded(call) => return Some(call),
                git::server::BodyStep::Malformed => return None,
            }
        } else {
            let Some(Down::Demand { read, .. }) = below.pop() else {
                return None;
            };
            json::up(&mut tokenizer, &env, input.answer(read)?, &mut above, &mut below);
        }
    }
    None
}
pub(crate) fn worst_case(bounds: &Limits) -> Option<u64> {
    let limits = limits(bounds);
    git::decoder_worst_case(&limits)?
        .checked_add(json::worst_case(&limits.documents.json)?)?
        .checked_add(Queue::<json::Event>::worst_case(1)?)?
        .checked_add(Queue::<Down>::worst_case(1)?)?
        // A decoded Basic credential is bounded by the HTTP head.
        .checked_add(u64::from(bounds.http.head))?
        // Held's input copy and its largest stream delivery coexist.
        .checked_add(u64::from(bounds.body).checked_mul(2)?)?
        // objects holds whole/graph ids and a frontier of four parents per item.
        .checked_add(skein_lib::List::<u64>::worst_case(bounds.github.inner)?)?
        .checked_add(skein_lib::List::<git::Parents>::worst_case(bounds.github.inner)?)?
        .checked_add(skein_lib::Set::<core::cmp::Reverse<u64>>::worst_case(bounds.github.inner.checked_mul(4)?)?)
}
