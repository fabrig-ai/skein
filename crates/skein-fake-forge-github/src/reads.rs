//! Repository reads each translate to one shared-domain call.
use crate::{
    answers::{self, Body, Page},
    auth::Access,
    service::{self, Limits, Outcome, Response, Row, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{
    self as fake,
    api::{self as a, Answer, Error, Op},
};
use skein_forge::{
    api,
    forgejo::id::{ObjectFormat, ObjectId},
    github::document as d,
};
use skein_lib::{List, ReplyTo, Wall, bytes};

mod checks;
mod issues;
pub(crate) mod people;
pub(crate) mod protection;
mod pulls;
mod reviews;

// The earliest observed nonempty since; the bound was not probed further.
const ISSUES_SINCE_MIN: Wall = Wall::from_nanos(31_536_000_000_000_000);

// Two quotes around the hexadecimal SHA-256 of the repository document.
pub(crate) const REPOSITORY_ETAG_BYTES: usize = 66;

#[derive(Debug)]
pub(crate) enum Read {
    Checks { owner: Box<[u8]>, name: Box<[u8]>, commit: api::Commit, page: u32, per_page: u32, runs: bool },
    JobLog { owner: Box<[u8]>, name: Box<[u8]>, id: u64 },
    Job { owner: Box<[u8]>, name: Box<[u8]>, id: u64 },
    Protection { owner: Box<[u8]>, name: Box<[u8]>, query: protection::Read },
    Repository(u64),
    Enqueue { owner: Box<[u8]>, name: Box<[u8]>, number: u64, head: api::Commit },
    PullGraphql { owner: Box<[u8]>, name: Box<[u8]>, number: u64 },
    Reviews { owner: Box<[u8]>, name: Box<[u8]>, number: u64, page: u32, per_page: u32 },
    ReviewComments { owner: Box<[u8]>, name: Box<[u8]>, number: u64, review: u64 },
    Collaborators { owner: Box<[u8]>, name: Box<[u8]>, page: u32, per_page: u32 },
    Permission { owner: Box<[u8]>, name: Box<[u8]>, login: Box<[u8]> },
    Pull { owner: Box<[u8]>, name: Box<[u8]>, number: u64 },
    PullsFor { owner: Box<[u8]>, name: Box<[u8]>, head: Box<[u8]>, base: Box<[u8]>, open: bool },
    PullFiles { owner: Box<[u8]>, name: Box<[u8]>, number: u64, page: u32, per_page: u32 },
    Compare { owner: Box<[u8]>, name: Box<[u8]>, before: api::Commit, after: api::Commit, per_page: u32 },
    Raw { owner: Box<[u8]>, name: Box<[u8]>, commit: api::Commit, path: Box<[u8]> },
    Issues { owner: Box<[u8]>, name: Box<[u8]>, since: Wall, page: u32, per_page: u32 },
    Issue { owner: Box<[u8]>, name: Box<[u8]>, number: u64 },
    Comments { owner: Box<[u8]>, name: Box<[u8]>, number: u64, page: u32, per_page: u32 },
    Locate { owner: Box<[u8]>, name: Box<[u8]> },
    Branch { owner: Box<[u8]>, name: Box<[u8]>, branch: Box<[u8]> },
    Branches { owner: Box<[u8]>, name: Box<[u8]>, page: u32, per_page: u32 },
}
#[derive(Debug)]
pub(crate) enum Pending {
    Checks { repository: Box<[u8]>, commit: u64, page: u32, size: u32, target: Box<[u8]>, runs: bool },
    JobLog { repository: u32, now: Wall },
    Job { repository: Box<[u8]> },
    Protection { repository: Box<[u8]>, query: protection::Read },
    Repository(Row),
    Enqueue { repository: Box<[u8]> },
    PullGraphql { repository: u32, number: u64 },
    Reviews { page: u32, size: u32, target: Box<[u8]> },
    ReviewComments(u64),
    Collaborators { page: u32, size: u32, target: Box<[u8]> },
    Permission,
    Pull,
    PullsFor { open: bool },
    PullFiles { target: Box<[u8]>, page: u32, size: u32 },
    Compare { size: u32 },
    Raw,
    Issues { target: Box<[u8]>, page: u32, size: u32 },
    Issue,
    Comments { target: Box<[u8]>, page: u32, size: u32 },
    Branch(Box<[u8]>),
    Branches { page: u32, size: u32, target: Box<[u8]> },
}
fn row<'a>(service: &'a Service, read: &Read) -> Option<&'a Row> {
    match read {
        Read::Repository(id) => service.repositories.get(&u32::try_from(*id).ok()?),
        Read::Checks { owner, name, .. }
        | Read::Job { owner, name, .. }
        | Read::JobLog { owner, name, .. }
        | Read::Protection { owner, name, .. }
        | Read::Enqueue { owner, name, .. }
        | Read::PullGraphql { owner, name, .. }
        | Read::Reviews { owner, name, .. }
        | Read::ReviewComments { owner, name, .. }
        | Read::Collaborators { owner, name, .. }
        | Read::Permission { owner, name, .. }
        | Read::Pull { owner, name, .. }
        | Read::PullsFor { owner, name, .. }
        | Read::PullFiles { owner, name, .. }
        | Read::Compare { owner, name, .. }
        | Read::Raw { owner, name, .. }
        | Read::Issues { owner, name, .. }
        | Read::Issue { owner, name, .. }
        | Read::Comments { owner, name, .. }
        | Read::Locate { owner, name }
        | Read::Branch { owner, name, .. }
        | Read::Branches { owner, name, .. } => {
            for (_, row) in &service.repositories {
                if row.owner == *owner && row.name == *name {
                    return Some(row);
                }
            }
            None
        }
    }
}
fn contents(read: &Read) -> bool {
    match read {
        Read::Repository(_)
        | Read::Locate { .. }
        | Read::Branch { .. }
        | Read::Branches { .. }
        | Read::Compare { .. }
        | Read::Raw { .. } => true,
        Read::Enqueue { .. }
        | Read::Checks { .. }
        | Read::Job { .. }
        | Read::JobLog { .. }
        | Read::Protection { .. }
        | Read::PullGraphql { .. }
        | Read::Reviews { .. }
        | Read::ReviewComments { .. }
        | Read::Collaborators { .. }
        | Read::Permission { .. }
        | Read::Pull { .. }
        | Read::PullsFor { .. }
        | Read::Issues { .. }
        | Read::Issue { .. }
        | Read::Comments { .. }
        | Read::PullFiles { .. } => false,
    }
}

#[expect(clippy::too_many_lines, reason = "each read variant translates to its domain call and retained identity")]
pub(crate) fn call(
    service: &Service,
    read: Read,
    grant: &Access,
    target: &[u8],
    body_fits: bool,
    now: Wall,
    reply_to: ReplyTo,
) -> Outcome {
    let Some(row) = row(service, &read) else {
        return Outcome::Reply(answers::missing(&service.limits));
    };
    if !grant.reaches(service, row.id) {
        return Outcome::Reply(answers::missing(&service.limits));
    }
    if !body_fits {
        return Outcome::Reply(answers::refusal(413, b"Payload Too Large", &service.limits));
    }
    if !contents(&read) && grant.contents() {
        return Outcome::Reply(answers::refusal(403, b"Resource not accessible by integration", &service.limits));
    }
    let Some(repository) = full(row) else {
        return Outcome::Reply(answers::failed());
    };
    let (kind, op) = match read {
        Read::Checks { commit, page, per_page, runs, .. } => {
            let Some(commit) = count(&commit) else {
                return Outcome::Reply(answers::missing(&service.limits));
            };
            (
                Pending::Checks {
                    repository: bytes::copy_of(&repository),
                    commit,
                    page,
                    size: per_page.min(service.limits.page),
                    target: bytes::copy_of(target),
                    runs,
                },
                Op::Read(a::Read::Checks { commit }),
            )
        }
        Read::JobLog { id, .. } => (Pending::JobLog { repository: row.id, now }, Op::Read(a::Read::Job { job: id })),
        Read::Job { id, .. } => {
            (Pending::Job { repository: bytes::copy_of(&repository) }, Op::Read(a::Read::Job { job: id }))
        }
        Read::Protection { query, .. } => (
            Pending::Protection { repository: repository.clone(), query },
            // GitHub's rule documents need repository read access, whereas
            // the shared admin-only Protection read serves Forgejo's face.
            Op::Read(a::Read::Settings),
        ),
        Read::Enqueue { number, head, .. } => {
            let Some(head) = count(&head) else {
                return Outcome::Reply(answers::response(
                    200,
                    &Body::GraphqlRefusal(b"UNPROCESSABLE", crate::texts::ENQUEUE_STALE),
                    &service.limits,
                ));
            };
            return crate::enqueue::start(repository, number, head, grant.user(), reply_to);
        }
        Read::PullGraphql { number, .. } => {
            (Pending::PullGraphql { repository: row.id, number }, Op::Read(a::Read::Pull { number }))
        }
        Read::Reviews { number, page, per_page, .. } => (
            Pending::Reviews { page, size: per_page.min(service.limits.page), target: bytes::copy_of(target) },
            Op::Read(a::Read::Pull { number }),
        ),
        Read::ReviewComments { number, review, .. } => {
            (Pending::ReviewComments(review), Op::Read(a::Read::Pull { number }))
        }
        Read::Collaborators { page, per_page, .. } => (
            Pending::Collaborators { page, size: per_page.min(service.limits.page), target: bytes::copy_of(target) },
            Op::Read(a::Read::Collaborators),
        ),
        Read::Permission { login, .. } => {
            let Some(user) = people::id(service, &login) else {
                return Outcome::Reply(answers::missing(&service.limits));
            };
            if user == service.bot {
                return Outcome::Reply(answers::response(
                    200,
                    &Body::Permission(api::Permission::None),
                    &service.limits,
                ));
            }
            (Pending::Permission, Op::Read(a::Read::Permission { user }))
        }
        Read::Pull { number, .. } => (Pending::Pull, Op::Read(a::Read::Pull { number })),
        Read::PullsFor { head, base, open, .. } => {
            (Pending::PullsFor { open }, Op::Read(a::Read::PullFor { head, base }))
        }
        Read::PullFiles { number, page, per_page, .. } => (
            Pending::PullFiles { target: bytes::copy_of(target), page, size: per_page.min(service.limits.page) },
            Op::Read(a::Read::PullFiles { number, page, limit: per_page.min(service.limits.page) }),
        ),
        Read::Compare { before, after, per_page, .. } => {
            let (Some(base), Some(head)) = (count(&before), count(&after)) else {
                return Outcome::Reply(answers::missing(&service.limits));
            };
            (
                Pending::Compare { size: per_page.min(service.limits.page) },
                Op::Read(a::Read::Compare { base, head, page: 1, limit: per_page.min(service.limits.page) }),
            )
        }
        Read::Raw { commit, path, .. } => {
            let Some(commit) = count(&commit) else {
                return Outcome::Reply(answers::missing(&service.limits));
            };
            (Pending::Raw, Op::Read(a::Read::File { commit, path }))
        }
        Read::Issues { since, .. } if since < ISSUES_SINCE_MIN && bytes::find(target, b"&since=").is_some() => {
            return Outcome::Reply(answers::response(200, &Body::Issues(&[]), &service.limits));
        }
        Read::Issues { since, page, per_page, .. } => (
            Pending::Issues { target: bytes::copy_of(target), page, size: per_page.min(service.limits.page) },
            Op::Read(a::Read::Items {
                state: None,
                kind: None,
                labels: Box::from([]),
                author: None,
                order: a::Order::LeastUpdated,
                since: skein_lib::Time::from_nanos(since.as_nanos()),
                page,
                limit: per_page.min(service.limits.page),
            }),
        ),
        Read::Issue { number, .. } => (Pending::Issue, Op::Read(a::Read::Item { number, after: 0 })),
        Read::Comments { number, page, per_page, .. } => (
            Pending::Comments { target: bytes::copy_of(target), page, size: per_page.min(service.limits.page) },
            Op::Read(a::Read::Comments { number }),
        ),
        Read::Repository(_) | Read::Locate { .. } => (Pending::Repository(row.clone()), Op::Read(a::Read::Settings)),
        Read::Branch { branch, .. } => (Pending::Branch(branch.clone()), Op::Read(a::Read::Branch { branch })),
        Read::Branches { page, per_page, .. } => (
            Pending::Branches { page, size: per_page.min(service.limits.page), target: bytes::copy_of(target) },
            Op::Git(a::Git::Clone),
        ),
    };
    Outcome::Call(
        service::Pending { rate: None, kind: service::PendingKind::Read(kind) },
        fake::Event::Call { reply_to, user: grant.user(), repository, op },
    )
}
pub(crate) fn full(row: &Row) -> Option<Box<[u8]>> {
    let owner = service::joined(&row.owner, b"/")?;
    service::joined(&owner, &row.name)
}
// fake-forge.md, 4: both faces share the domain's SHA-1 spelling.
fn object(commit: u64) -> ObjectId {
    skein_fake_forge_protocol::routes::object(commit)
}
pub(crate) fn commit(count: u64) -> api::Commit {
    object(count).bytes
}

// Reject a non-domain SHA rather than silently folding it onto a stored count.
#[expect(clippy::manual_map, reason = "step code spells Option transformations with exhaustive matches")]
fn optional_commit(value: Option<u64>) -> Option<api::Commit> {
    match value {
        Some(value) => Some(commit(value)),
        None => None,
    }
}
pub(crate) fn count(value: &api::Commit) -> Option<u64> {
    skein_fake_forge_protocol::routes::commit(&ObjectId { format: ObjectFormat::Sha1, bytes: *value })
}

pub(crate) fn finish(
    pending: Pending,
    result: Result<Answer, Error>,
    service: &Service,
    domain: &fake::Domain,
) -> Response {
    let limits = &service.limits;
    let result = match &pending {
        Pending::Enqueue { repository } => return enqueue_finish(result, repository, limits, domain),
        Pending::PullsFor { .. } => match result {
            Err(Error::Missing(a::What::Item | a::What::Pull)) => {
                return answers::response(200, &Body::Pulls(&[]), limits);
            }
            result => result,
        },
        Pending::Checks { .. }
        | Pending::Job { .. }
        | Pending::JobLog { .. }
        | Pending::Protection { .. }
        | Pending::PullGraphql { .. }
        | Pending::Reviews { .. }
        | Pending::ReviewComments(_)
        | Pending::Collaborators { .. }
        | Pending::Permission
        | Pending::Repository(_)
        | Pending::Issues { .. }
        | Pending::Issue
        | Pending::Comments { .. }
        | Pending::Branch(_)
        | Pending::Branches { .. }
        | Pending::Pull
        | Pending::PullFiles { .. }
        | Pending::Compare { .. }
        | Pending::Raw => result,
    };
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => return refused(error, limits),
    };
    match pending {
        Pending::Checks { repository, commit, page, size, target, runs } => {
            checks::render(answer, &repository, commit, page, size, &target, runs, limits, domain)
        }
        Pending::JobLog { repository, now } => crate::logs::redirect(service, repository, answer, now),
        Pending::Job { repository } => checks::job(answer, &repository, limits, domain),
        Pending::Protection { repository, query } => {
            let Answer::Settings(_) = answer else {
                return answers::failed();
            };
            protection::finish(service, domain, &repository, query)
        }
        Pending::Enqueue { repository } => enqueue_finish(Ok(answer), &repository, limits, domain),
        Pending::PullGraphql { repository, number } => pulls::graphql(answer, repository, number, limits),
        Pending::Reviews { page, size, target } => reviews::reviews(answer, page, size, &target, limits),
        Pending::ReviewComments(review) => reviews::comments(answer, review, limits),
        Pending::Collaborators { page, size, target } => people::collaborators(answer, page, size, &target, limits),
        Pending::Permission => {
            let Answer::Permission(value) = answer else {
                return answers::failed();
            };
            answers::response(200, &Body::Permission(people::permission(value)), limits)
        }
        Pending::Pull => pulls::pull(answer, limits),
        Pending::PullsFor { open } => pulls::pulls_for(answer, open, limits),
        Pending::PullFiles { target, page, size } => pulls::files(answer, &target, page, size, limits),
        Pending::Compare { size } => pulls::compare(answer, size, limits),
        Pending::Raw => {
            let Answer::File(body) = answer else {
                return answers::failed();
            };
            if body.len() > usize::try_from(limits.response).expect("response bound") {
                return answers::failed();
            }
            Response {
                rate: None,
                etag: None,
                content_type: service::ContentType::Octets,
                status: 200,
                link: None,
                location: None,
                body,
            }
        }
        Pending::Issues { target, page, size } => issues::items(answer, &target, page, size, limits),
        Pending::Issue => issues::item(answer, limits),
        Pending::Comments { target, page, size } => issues::comments(answer, &target, page, size, limits),
        Pending::Repository(row) => repository(row, answer, limits),
        Pending::Branch(name) => {
            let Answer::Commit(count) = answer else {
                return answers::failed();
            };
            let Some(object) = domain.object(count) else { return answers::failed() };
            answers::response(200, &Body::Branch(&d::Branch { name, commit: commit(count) }, &object.message), limits)
        }
        Pending::Branches { page, size, target } => branches(answer, page, size, &target, limits),
    }
}

fn branches(answer: Answer, page: u32, size: u32, target: &[u8], limits: &Limits) -> Response {
    let Answer::Cloned { branches, .. } = answer else {
        return answers::failed();
    };
    let Ok(total) = u64::try_from(branches.len()) else {
        return answers::failed();
    };
    let Some(page) = Page::new(page, size, total) else {
        return answers::failed();
    };
    let mut names = List::with_capacity(size);
    for (index, branch) in branches.iter().enumerate() {
        let Ok(index) = u64::try_from(index) else {
            return answers::failed();
        };
        if page.contains(index) {
            let Ok(length) = u32::try_from(branch.branch.len()) else {
                return answers::failed();
            };
            if length > limits.github.string_bytes || names.push(branch.branch.as_ref()).is_err() {
                return answers::failed();
            }
        }
    }
    page.link(answers::response(200, &Body::Branches(names.as_slice()), limits), target, limits)
}
fn refused(error: Error, limits: &Limits) -> Response {
    match error {
        Error::Missing(_) | Error::Forbidden => answers::missing(limits),
        Error::Unavailable => answers::refusal(503, b"Server Error", limits),
        Error::Timeout => answers::refusal(504, b"Server Error", limits),
        // Rate heads/refusals belong to the rates task; no rate simulation here.
        Error::RateLimited { .. }
        | Error::TooLarge
        | Error::Full
        | Error::AlreadyQueued(_)
        | Error::Exists
        | Error::Circular
        | Error::NothingToMerge
        | Error::Empty
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Unreachable
        | Error::Refused => answers::failed(),
    }
}

pub(crate) fn enqueue_finish(
    result: Result<Answer, Error>,
    repository: &[u8],
    limits: &Limits,
    domain: &fake::Domain,
) -> Response {
    let entry = match result {
        Ok(answer) => {
            let Answer::Pull(pull) = answer else {
                return answers::failed();
            };
            pull.queued
        }
        Err(Error::AlreadyQueued(entry)) => Some(entry),
        Err(Error::Protected) => {
            let Some(protection) = domain.inspect_repository_protection(repository) else {
                return answers::failed();
            };
            let Ok(capacity) = usize::try_from(limits.response) else {
                return answers::failed();
            };
            let mut out = skein_lib::Writer::new(capacity);
            if out.put(crate::texts::ENQUEUE_APPROVAL_PREFIX).is_err()
                || crate::texts::approval(&mut out, protection.approvals).is_none()
            {
                return answers::failed();
            }
            let Some(message) = answers::finish(out) else {
                return answers::failed();
            };
            return answers::response(200, &Body::EnqueueRefusal(&message), limits);
        }
        Err(Error::Stale) => {
            return answers::response(
                200,
                &Body::GraphqlRefusal(b"UNPROCESSABLE", crate::texts::ENQUEUE_STALE),
                limits,
            );
        }
        Err(Error::Missing(_)) => {
            return answers::response(
                200,
                &Body::GraphqlRefusal(b"NOT_FOUND", b"Could not resolve to a PullRequest node"),
                limits,
            );
        }
        Err(Error::Unavailable | Error::Timeout | Error::RateLimited { .. }) => return answers::failed(),
        Err(
            Error::Forbidden
            | Error::TooLarge
            | Error::Full
            | Error::Exists
            | Error::Circular
            | Error::NothingToMerge
            | Error::Empty
            | Error::Closed
            | Error::Conflict
            | Error::Unreachable
            | Error::Refused,
        ) => {
            return answers::response(
                200,
                &Body::GraphqlRefusal(b"UNPROCESSABLE", b"Cannot enqueue pull request"),
                limits,
            );
        }
    };
    match entry {
        Some(entry) => answers::response(200, &Body::Enqueued(entry.position), limits),
        None => answers::failed(),
    }
}

fn node(id: &[u8]) -> Option<(u32, u64)> {
    let suffix = id.strip_prefix(b"PR_")?;
    let at = bytes::find(suffix, b"_")?;
    let repository = skein_forge::forgejo::id::unsigned(suffix.get(..at)?)?;
    let number = skein_forge::forgejo::id::unsigned(suffix.get(at.checked_add(1)?..)?)?;
    Some((u32::try_from(repository).ok()?, number))
}

pub(crate) fn enqueue(service: &Service, id: &[u8], head: api::Commit, grant: &Access, reply_to: ReplyTo) -> Outcome {
    let missing = answers::response(
        200,
        &Body::GraphqlRefusal(b"NOT_FOUND", b"Could not resolve to a PullRequest node"),
        &service.limits,
    );
    let Some((repository, number)) = node(id) else {
        return Outcome::Reply(missing);
    };
    let Some(row) = service.repositories.get(&repository) else {
        return Outcome::Reply(missing);
    };
    if !grant.reaches(service, repository) {
        return Outcome::Reply(missing);
    }
    let Some(canonical) = pulls::node_id(repository, number, &service.limits) else {
        return Outcome::Reply(missing);
    };
    if canonical.as_ref() != id {
        return Outcome::Reply(missing);
    }
    call(
        service,
        Read::Enqueue { owner: row.owner.clone(), name: row.name.clone(), number, head },
        grant,
        b"/graphql",
        true,
        Wall::EPOCH,
        reply_to,
    )
}

fn repository(row: Row, answer: Answer, limits: &Limits) -> Response {
    let Answer::Settings(settings) = answer else {
        return answers::failed();
    };
    let Ok(length) = u32::try_from(settings.default.len()) else {
        return answers::failed();
    };
    if length > limits.github.string_bytes {
        return answers::failed();
    }
    let Some(full) = full(&row) else {
        return answers::failed();
    };
    // Routing/owner data is GitHub's table; settings and default come
    // only from the domain. html_url uses GitHub's public web origin.
    let Some(html_url) = service::joined(b"https://github.com/", &full) else {
        return answers::failed();
    };
    let document = d::Repository {
        id: u64::from(row.id),
        owner: row.owner,
        owner_id: row.owner_id,
        organization: row.organization,
        name: row.name,
        default_branch: settings.default,
        html_url,
        merge: settings.merge,
        squash: settings.squash,
        rebase: settings.rebase,
    };
    let mut response = answers::response(200, &Body::Repository(&document), limits);
    let mut tag = skein_lib::Writer::new(REPOSITORY_ETAG_BYTES);
    tag.put(b"\"").expect("tag");
    for byte in skein_lib::sha256::digest(&response.body) {
        for nibble in [byte / 16, byte % 16] {
            tag.put(&[if nibble < 10 {
                b'0'.saturating_add(nibble)
            } else {
                b'a'.saturating_add(nibble.saturating_sub(10))
            }])
            .expect("tag");
        }
    }
    tag.put(b"\"").expect("tag");
    response.etag = Some(tag.finish());
    response
}

pub(crate) fn user(service: &Service, id: u64, body_fits: bool) -> Response {
    people::user(service, id, body_fits)
}
