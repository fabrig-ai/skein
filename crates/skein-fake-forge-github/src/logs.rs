//! Signed job-log targets on the forge's clock (fake-forge.md, 10.1).
use crate::{
    answers, reads,
    service::{ContentType, Limits, Outcome, Pending, PendingKind, Problem, Response, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as domain, api as a};
use skein_forge::{forgejo::id::unsigned, github::request::Method};
use skein_lib::{Decimal, List, Queue, ReplyTo, Rng, Secret, Wall, Writer, bytes, sha256::Hmac};

const LIFE: u64 = 600_000_000_000;
// u32 repository, u64 job/expiry, u32 attempt, separators and 64 hex digits.
const SIGNED_MAX: u32 = 160;

/// A blob request records only header presence, never a credential's value.
#[derive(PartialEq, Eq)]
pub struct Record {
    pub path: Box<[u8]>,
    pub authorization: bool,
    pub range: bool,
}
impl core::fmt::Debug for Record {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Record")
            .field("path_bytes", &self.path.len())
            .field("authorization", &self.authorization)
            .field("range", &self.range)
            .finish()
    }
}
#[derive(Debug)]
pub(crate) struct Store {
    pub(crate) origin: Box<[u8]>,
    third_origin: Option<Box<[u8]>>,
    redirect: Option<Box<[u8]>>,
    key: Secret,
    records: Queue<Record>,
}
impl Store {
    pub(crate) fn new(
        origin: Box<[u8]>,
        third_origin: Option<Box<[u8]>>,
        redirect: Option<Box<[u8]>>,
        records: u32,
        seed: u64,
    ) -> Self {
        let mut rng = Rng::new(seed);
        let mut key = Writer::new(32);
        for _ in 0_u8..4 {
            key.put(&rng.next_u64().to_be_bytes()).expect("four key words");
        }
        Self { origin, third_origin, redirect, key: Secret::new(key.finish()), records: Queue::with_capacity(records) }
    }
}
impl Service {
    /// The socket host learns this origin when its second listener binds.
    pub fn set_blob_origin(&mut self, origin: Box<[u8]>) -> Result<(), Problem> {
        if !origin_fits(&origin, &self.limits) {
            return Err(Problem::Config);
        }
        self.logs.origin = origin;
        Ok(())
    }
    #[must_use]
    pub const fn blob_records(&self) -> &Queue<Record> {
        &self.logs.records
    }

    /// Serve the blob without API authentication or version headers. Range is ignored.
    pub fn blob(
        &mut self,
        method: Method,
        target: &[u8],
        authorization: bool,
        range: bool,
        now: Wall,
        reply_to: ReplyTo,
    ) -> Outcome {
        // Keep recent requests, evicting the oldest when the bounded record is full.
        if self.logs.records.room() == 0 {
            drop(self.logs.records.pop());
        }
        let cap = usize::try_from(self.limits.http.head).expect("u32 fits");
        let path = bytes::copy_of(target.get(..target.len().min(cap)).expect("bounded prefix"));
        self.logs.records.push(Record { path, authorization, range });
        if method != Method::Get || !target.starts_with(b"/logs/") {
            return Outcome::Reply(empty(404));
        }
        // Empty 403 on tampering/expiry is inferred: github.com's blob store is not modelled.
        let Some((repository, job, attempt)) = verify(&self.logs.key, target, now) else {
            return Outcome::Reply(empty(403));
        };
        let Some(row) = self.repositories.get(&repository) else {
            return Outcome::Reply(empty(404));
        };
        if let Some(origin) = &self.logs.redirect {
            return Outcome::Reply(redirect_to(origin, target));
        }
        let Some(repository) = reads::full(row) else {
            return Outcome::Reply(answers::failed());
        };
        Outcome::Call(
            Pending { rate: None, kind: PendingKind::Blob },
            domain::Event::Call {
                reply_to,
                user: self.bot,
                repository,
                op: a::Op::Read(a::Read::Log { job, attempt, step: None }),
            },
        )
    }
}
pub(crate) fn optional_origin_fits(origin: Option<&[u8]>, limits: &Limits) -> bool {
    match origin {
        Some(origin) => origin_fits(origin, limits),
        None => true,
    }
}
pub(crate) fn origin_fits(origin: &[u8], limits: &Limits) -> bool {
    let Some(length) = origin.len().checked_add(usize::try_from(SIGNED_MAX).expect("small constant")) else {
        return false;
    };
    crate::service::valid_origin(origin, limits.github.target_bytes)
        && length <= usize::try_from(limits.github.target_bytes).expect("u32 fits")
        && length <= usize::try_from(limits.response).expect("u32 fits")
}

pub(crate) fn redirect(service: &Service, repository: u32, answer: a::Answer, now: Wall) -> Response {
    let a::Answer::Job { job, .. } = answer else {
        return answers::failed();
    };
    let Some(expires) = now.as_nanos().checked_add(LIFE) else {
        return answers::failed();
    };
    let Some(path) = signed(&service.logs.key, repository, job.id, job.attempt, expires, &service.limits) else {
        return answers::failed();
    };
    let origin = match &service.logs.third_origin {
        Some(origin) => origin,
        None => &service.logs.origin,
    };
    redirect_to(origin, &path)
}
fn redirect_to(origin: &[u8], target: &[u8]) -> Response {
    let Some(location) = crate::service::joined(origin, target) else {
        return answers::failed();
    };
    let mut response = empty(302);
    response.location = Some(location);
    response
}
fn signed(key: &Secret, repository: u32, job: u64, attempt: u32, expires: u64, limits: &Limits) -> Option<Box<[u8]>> {
    let mut out = Writer::new(usize::try_from(limits.github.target_bytes).ok()?);
    for part in [
        b"/logs/".as_slice(),
        Decimal::of(u64::from(repository)).as_bytes(),
        b"/",
        Decimal::of(job).as_bytes(),
        b"/",
        Decimal::of(u64::from(attempt)).as_bytes(),
        b"?expires=",
        Decimal::of(expires).as_bytes(),
    ] {
        out.put(part).ok()?;
    }
    let prefix = answers::finish(out)?;
    let mut out = Writer::new(prefix.len().checked_add(11)?.checked_add(64)?);
    out.put(&prefix).ok()?;
    out.put(b"&signature=").ok()?;
    out.put(&signature(key, &prefix)).ok()?;
    Some(out.finish())
}
fn signature(key: &Secret, input: &[u8]) -> [u8; 64] {
    let mut mac = Hmac::new(key.expose());
    mac.update(input);
    let mut out = [0; 64];
    for (pair, byte) in out.as_chunks_mut::<2>().0.iter_mut().zip(mac.finish()) {
        for (digit, nibble) in pair.iter_mut().zip([byte / 16, byte % 16]) {
            *digit =
                if nibble < 10 { b'0'.saturating_add(nibble) } else { b'a'.saturating_add(nibble.saturating_sub(10)) };
        }
    }
    out
}
fn verify(key: &Secret, target: &[u8], now: Wall) -> Option<(u32, u64, u32)> {
    let at = bytes::find(target, b"&signature=")?;
    let prefix = target.get(..at)?;
    if target.get(at.checked_add(11)?..)? != signature(key, prefix) {
        return None;
    }
    let at = bytes::find(prefix, b"?expires=")?;
    let expires = unsigned(prefix.get(at.checked_add(9)?..)?)?;
    if now.as_nanos() >= expires {
        return None;
    }
    let path = prefix.get(..at)?.strip_prefix(b"/logs/")?;
    let first = bytes::find(path, b"/")?;
    let repository = u32::try_from(unsigned(path.get(..first)?)?).ok()?;
    let tail = path.get(first.checked_add(1)?..)?;
    let second = bytes::find(tail, b"/")?;
    let job = unsigned(tail.get(..second)?)?;
    let attempt = u32::try_from(unsigned(tail.get(second.checked_add(1)?..)?)?).ok()?;
    Some((repository, job, attempt))
}
pub(crate) fn empty(status: u16) -> Response {
    Response {
        rate: None,
        etag: None,
        status,
        content_type: ContentType::Plaintext,
        link: None,
        location: None,
        body: Box::new([]),
    }
}
pub(crate) fn finish(result: Result<a::Answer, a::Error>, limits: &Limits) -> Response {
    let Ok(answer) = result else {
        return empty(404);
    };
    let a::Answer::File(body) = answer else {
        return answers::failed();
    };
    if body.len() > usize::try_from(limits.response).expect("u32 fits") {
        return answers::failed();
    }
    Response { body, ..empty(200) }
}

pub(crate) fn worst_case(limits: &Limits) -> Option<u64> {
    Queue::<Record>::worst_case(limits.blob_records)?
        .checked_add(u64::from(limits.blob_records).checked_mul(u64::from(limits.http.head))?)?
        .checked_add(32)?
        // Three retained origins, signing scratch and the composed Location.
        .checked_add(u64::from(limits.github.target_bytes).checked_mul(6)?)?
        .checked_add(List::<u8>::worst_case(SIGNED_MAX)?)
}
