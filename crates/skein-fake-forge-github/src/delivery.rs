//! GitHub deliveries and their bounded log (fake-forge.md, 10.3).
use crate::{
    answers,
    service::{ContentType, Problem, Response},
};
use alloc::boxed::Box;
use core::net::SocketAddr;
use skein_forge::github::{self, delivery::Delivery, log, request, server};
use skein_lib::{Decimal, Duration, List, Rng, Secret, Wall, Writer, bytes};

/// A scenario's loopback destination; the App's advertised hook URL need not resolve.
#[derive(Debug)]
pub struct Endpoint {
    pub host: Box<[u8]>,
    pub target: Box<[u8]>,
}
#[derive(Debug)]
pub struct Config {
    pub endpoint: Option<Endpoint>,
    pub log_min: Duration,
    pub log_max: Duration,
    pub log_keep: Duration,
    pub log_entries: u32,
    /// Minimum text-body size for issue, comment and review payloads, for oversize stories.
    pub payload_size: u32,
}
impl Config {
    #[must_use]
    pub fn standard() -> Self {
        Self {
            endpoint: None,
            log_min: Duration::from_millis(1500),
            log_max: Duration::from_millis(2200),
            log_keep: Duration::from_secs(259_200),
            log_entries: 64,
            payload_size: 0,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub entries: u32,
    pub payload_bytes: u32,
}
/// One complete bounded HTTP request and the log entry its answer updates.
#[derive(Debug)]
pub struct Post {
    pub id: u64,
    pub head: Box<[u8]>,
    pub body: Box<[u8]>,
}
#[derive(Debug)]
struct Entry {
    row: log::Row,
    payload: Delivery,
    signature: [u8; 71],
    visible: Wall,
    listed: bool,
}
#[derive(Debug)]
pub(crate) struct Store {
    entries: List<Option<Entry>>,
    config: Config,
    limits: Limits,
    rng: Rng,
    next: u64,
    revision: u64,
}
impl Store {
    pub(crate) fn new(config: Config, limits: Limits, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let next = 1_000_000_000_000_000_000_u64.checked_add(rng.below(1_000_000_000)).expect("19-digit id");
        Self { entries: List::with_capacity(config.log_entries), config, limits, rng, next, revision: 0 }
    }
    pub(crate) fn endpoint(&self) -> Option<&Endpoint> {
        self.config.endpoint.as_ref()
    }
    pub(crate) fn poll(&mut self, now: Wall) {
        for index in (0..self.entries.len()).rev() {
            let entry = self.entries.get_mut(index).expect("entry").as_mut().expect("packed log");
            if now.as_nanos().saturating_sub(entry.row.delivered_at.as_nanos()) >= self.config.log_keep.as_nanos() {
                self.remove(index);
            } else if !entry.listed && now >= entry.visible {
                entry.listed = true;
                self.revision = self.revision.checked_add(1).expect("finite log changes");
            }
        }
    }
    fn remove(&mut self, at: u32) {
        let last = self.entries.len().checked_sub(1).expect("entry exists");
        if self.entries.get(at).expect("entry").as_ref().expect("packed log").listed {
            self.revision = self.revision.checked_add(1).expect("finite log changes");
        }
        for index in at..last {
            let moved = self.entries.get_mut(index.checked_add(1).expect("before last")).expect("next").take();
            *self.entries.get_mut(index).expect("entry") = moved;
        }
        self.entries.truncate(last);
    }
    #[expect(clippy::manual_map, reason = "step code spells Option transformations with exhaustive matches")]
    pub(crate) fn record(
        &mut self,
        mut payload: Delivery,
        installation: Option<u64>,
        now: Wall,
        secret: &Secret,
        bounds: &github::limits::Limits,
        redelivery: Option<[u8; 16]>,
    ) -> Result<(u64, Option<Post>), Problem> {
        self.poll(now);
        match &mut payload {
            Delivery::Issue { body, .. } | Delivery::Comment { body, .. } | Delivery::Review { body, .. } => match body
            {
                skein_forge::forgejo::document::Body::Kept(text) => {
                    if text.len() < usize::try_from(self.config.payload_size).expect("u32 fits") {
                        let mut padded = Writer::new(usize::try_from(self.config.payload_size).expect("u32 fits"));
                        padded.put(text).expect("payload size");
                        for _ in 0..padded.room() {
                            padded.put(b"x").expect("payload size");
                        }
                        *text = padded.finish();
                    }
                }
                skein_forge::forgejo::document::Body::Long(_) => return Err(Problem::Config),
            },
            Delivery::Pull { .. }
            | Delivery::Remark { .. }
            | Delivery::Push { .. }
            | Delivery::Ref { .. }
            | Delivery::Ci { .. }
            | Delivery::Installation { .. }
            | Delivery::Other => {}
        }
        let encode_bounds = github::limits::Limits {
            body_bytes: self.limits.payload_bytes,
            string_bytes: self.limits.payload_bytes,
            writer: skein_json::writer::Limits { length: self.limits.payload_bytes, ..bounds.writer },
            ..*bounds
        };
        let sent = server::encode(&payload, &encode_bounds).or(Err(Problem::Config))?;
        let signature = sign(secret, &sent.body);
        let mut guid = [0; 16];
        for byte in &mut guid {
            *byte = u8::try_from(self.rng.below(256)).expect("byte draw");
        }
        // RFC 4122 version 4 and variant bits, with all randomness from the scenario seed.
        *guid.get_mut(6).expect("UUID byte") = guid.get(6).expect("UUID byte") & 15 | 64;
        *guid.get_mut(8).expect("UUID byte") = guid.get(8).expect("UUID byte") & 63 | 128;
        let guid = redelivery.unwrap_or(guid);
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(Problem::Config)?;
        if self.next > 9_999_999_999_999_999_999 {
            return Err(Problem::Config);
        }
        let (repository, action) = metadata(&payload);
        let global =
            sent.event == b"ping" || sent.event == b"installation" || sent.event == b"installation_repositories";
        let row = log::Row {
            id,
            guid,
            event: bytes::copy_of(sent.event),
            action: match action {
                Some(action) => Some(bytes::copy_of(action)),
                None => None,
            },
            installation_id: if global { None } else { installation },
            repository_id: if global { None } else { repository },
            status_code: 502,
            delivered_at: now,
            redelivery: redelivery.is_some(),
        };
        let delay = self.rng.between(self.config.log_min.as_nanos(), self.config.log_max.as_nanos());
        let visible = Wall::from_nanos(now.as_nanos().saturating_add(delay));
        let post = match &self.config.endpoint {
            Some(endpoint) => Some(post(id, &guid, sent.event, sent.body, &signature, endpoint)?),
            None => None,
        };
        if self.entries.room() == 0 {
            self.remove(0);
        }
        self.entries.push(Some(Entry { row, payload, signature, visible, listed: false })).expect("log capacity");
        self.poll(now);
        Ok((id, post))
    }
    pub(crate) fn complete(&mut self, id: u64, status: u16) {
        for entry in self.entries.as_slice().iter().flatten() {
            if entry.row.id == id && entry.row.status_code == status {
                return;
            }
        }
        for index in 0..self.entries.len() {
            let entry = self.entries.get_mut(index).expect("entry").as_mut().expect("packed log");
            if entry.row.id == id {
                entry.row.status_code = status;
                if entry.listed {
                    self.revision = self.revision.checked_add(1).expect("finite log changes");
                }
            }
        }
    }
    pub(crate) fn redelivery(&self, id: u64) -> Option<(Delivery, Option<u64>, [u8; 16])> {
        for entry in self.entries.as_slice().iter().flatten() {
            if entry.row.id == id {
                return Some((entry.payload.clone(), entry.row.installation_id, entry.row.guid));
            }
        }
        None
    }
    pub(crate) fn page(
        &self,
        ask: &log::Ask,
        per_page: u32,
        validator: Option<&[u8]>,
        base: &[u8],
        limits: &crate::Limits,
    ) -> Response {
        if per_page == 0 || per_page > github::pin::PAGE_CAP || per_page > limits.github.inner {
            return answers::missing(limits);
        }
        let etag = self.etag();
        let before = match ask {
            log::Ask::First { .. } => {
                if validator == Some(etag.as_ref()) {
                    return Response {
                        location: None,
                        rate: None,
                        status: 304,
                        content_type: ContentType::Json,
                        link: None,
                        body: Box::new([]),
                        etag: Some(etag),
                    };
                }
                u64::MAX
            }
            log::Ask::Next { cursor } => match cursor_id(cursor.as_bytes()) {
                Some(id) => id,
                None => return answers::missing(limits),
            },
        };
        let mut rows = List::with_capacity(per_page);
        let mut more = false;
        for entry in self.entries.as_slice().iter().rev().flatten() {
            if entry.listed && entry.row.id < before {
                if rows.room() == 0 {
                    more = true;
                    break;
                }
                rows.push(entry.row.clone()).expect("page room");
            }
        }
        let mut out = Writer::new(usize::try_from(limits.response).expect("u32 fits"));
        if server::deliveries(rows.as_slice(), &mut out).is_err() {
            return answers::failed();
        }
        let Some(body) = answers::finish(out) else { return answers::failed() };
        let link = if more {
            let id = rows.last().expect("nonempty page").id;
            let target = request::encode(
                &request::Request::LogPage {
                    ask: log::Ask::Next { cursor: log::Cursor::new(&cursor(id)).expect("cursor") },
                    per_page,
                },
                &limits.github,
            );
            let Ok(target) = target else { return answers::failed() };
            let mut link = Writer::new(usize::try_from(limits.response).expect("u32 fits"));
            for part in [b"<".as_slice(), base, &target.target, b">; rel=\"next\""] {
                if link.put(part).is_err() {
                    return answers::failed();
                }
            }
            answers::finish(link)
        } else {
            None
        };
        Response {
            location: None,
            rate: None,
            status: 200,
            content_type: ContentType::Json,
            link,
            body,
            etag: match ask {
                log::Ask::First { .. } => Some(etag),
                log::Ask::Next { .. } => None,
            },
        }
    }
    pub(crate) fn get(&self, id: u64, limits: &crate::Limits) -> Response {
        for entry in self.entries.as_slice().iter().flatten() {
            if entry.row.id == id && entry.listed {
                let mut out = Writer::new(usize::try_from(limits.response).expect("u32 fits"));
                if server::logged(&entry.row, &entry.payload, &entry.signature, &mut out).is_err() {
                    return answers::failed();
                }
                return match answers::finish(out) {
                    Some(body) => Response {
                        location: None,
                        rate: None,
                        status: 200,
                        content_type: ContentType::Json,
                        link: None,
                        body,
                        etag: None,
                    },
                    None => answers::failed(),
                };
            }
        }
        answers::missing(limits)
    }
    fn etag(&self) -> Box<[u8]> {
        let digits = Decimal::of(self.revision);
        let mut out = Writer::new(digits.as_bytes().len().checked_add(2).expect("etag"));
        out.put(b"\"").expect("etag");
        out.put(digits.as_bytes()).expect("etag");
        out.put(b"\"").expect("etag");
        out.finish()
    }
    pub(crate) fn next_deadline(&self) -> Option<Wall> {
        let mut next = None;
        for entry in self.entries.as_slice().iter().flatten() {
            if entry.listed {
                continue;
            }
            let deadline = entry.visible;
            next = Some(match next {
                Some(value) => Wall::min(value, deadline),
                None => deadline,
            });
        }
        next
    }
}
fn metadata(payload: &Delivery) -> (Option<u64>, Option<&[u8]>) {
    use github::delivery::{Change, Ci};
    match payload {
        Delivery::Issue { repository, .. } => (Some(*repository), Some(b"opened")),
        Delivery::Comment { repository, .. } | Delivery::Remark { repository, .. } => {
            (Some(*repository), Some(b"created"))
        }
        Delivery::Review { repository, .. } => (Some(*repository), Some(b"submitted")),
        Delivery::Pull { repository, action, .. } => (Some(*repository), Some(&action.action)),
        Delivery::Push { repository, .. } | Delivery::Ref { repository, .. } => (Some(*repository), None),
        Delivery::Ci { repository, event, .. } => (
            Some(*repository),
            match event {
                Ci::ChecksRequested => Some(b"checks_requested"),
                Ci::Destroyed => Some(b"destroyed"),
                Ci::Status => None,
                Ci::CheckRun | Ci::CheckSuite | Ci::WorkflowJob | Ci::WorkflowRun => Some(b"completed"),
            },
        ),
        Delivery::Installation { change, .. } => (
            None,
            Some(match change {
                Change::Created => b"created",
                Change::Deleted => b"deleted",
                Change::Suspended => b"suspend",
                Change::Unsuspended => b"unsuspend",
                Change::Repositories => b"added",
                Change::Renamed { .. } => b"renamed",
            }),
        ),
        Delivery::Other => (None, None),
    }
}
fn cursor(id: u64) -> Box<[u8]> {
    let mut out = Writer::new(21);
    out.put(b"d-").expect("cursor");
    out.put(Decimal::of(id).as_bytes()).expect("19 digits");
    out.finish()
}
fn cursor_id(value: &[u8]) -> Option<u64> {
    skein_forge::forgejo::id::unsigned(value.strip_prefix(b"d-")?)
}
fn post(
    id: u64,
    guid: &[u8; 16],
    event: &[u8],
    body: Box<[u8]>,
    signature: &[u8],
    endpoint: &Endpoint,
) -> Result<Post, Problem> {
    let length = Decimal::of(u64::try_from(body.len()).or(Err(Problem::Limits))?);
    let parts = [
        b"POST ".as_slice(),
        &endpoint.target,
        b" HTTP/1.1\r\nHost: ",
        &endpoint.host,
        b"\r\nContent-Type: application/json\r\nContent-Length: ",
        length.as_bytes(),
        b"\r\nX-GitHub-Event: ",
        event,
        b"\r\nX-GitHub-Delivery: ",
        &github::guid::write(guid),
        b"\r\nX-Hub-Signature-256: ",
        signature,
        b"\r\nConnection: close\r\n\r\n",
    ];
    let mut size = 0usize;
    for part in parts {
        size = size.checked_add(part.len()).ok_or(Problem::Limits)?;
    }
    let mut out = Writer::new(size);
    for part in parts {
        out.put(part).expect("measured head");
    }
    Ok(Post { id, head: out.finish(), body })
}
fn sign(secret: &Secret, body: &[u8]) -> [u8; 71] {
    github::delivery::sign(secret.expose(), body)
}
pub(crate) fn valid(config: &Config, limits: Limits, string: u32) -> bool {
    if config.log_entries == 0
        || config.log_entries > limits.entries
        || limits.payload_bytes < 1024
        || u64::from(config.payload_size).saturating_mul(6).saturating_add(1024) > u64::from(limits.payload_bytes)
        || config.log_min > config.log_max
        || config.log_keep <= config.log_max
    {
        return false;
    }
    match &config.endpoint {
        None => true,
        Some(endpoint) => {
            let Some(address) = address(&endpoint.host) else { return false };
            for byte in &endpoint.target {
                if *byte <= 32 || *byte >= 127 {
                    return false;
                }
            }
            address.ip().is_loopback()
                && address.port() != 0
                && endpoint.host.len() <= usize::try_from(string).expect("u32 fits")
                && endpoint.target.len() <= usize::try_from(string).expect("u32 fits")
                && endpoint.target.starts_with(b"/")
        }
    }
}
pub(crate) fn worst_case(config: &Config, limits: Limits, string: u32) -> Option<u64> {
    if !valid(config, limits, string) {
        return None;
    }
    List::<Option<Entry>>::worst_case(config.log_entries)?
        .checked_add(
            u64::from(config.log_entries)
                .checked_mul(u64::from(limits.payload_bytes).checked_mul(2)?.checked_add(128)?)?,
        )?
        .checked_add(u64::from(limits.payload_bytes).checked_mul(3)?)?
        .checked_add(u64::from(string).checked_mul(2)?)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the GitHub protocol layer validates the scenario's numeric loopback authority before any effect"
)]
fn address(host: &[u8]) -> Option<SocketAddr> {
    core::str::from_utf8(host).ok()?.parse().ok()
}

impl crate::Service {
    /// Converts domain hooks without inspecting later domain state.
    pub fn hook(&mut self, hook: &skein_fake_forge_domain::Request, now: Wall) -> Result<Option<Post>, Problem> {
        use github::delivery::{Ci, Pull};
        use skein_fake_forge_domain::api::{Change, QueueHook, QueueRemoval};
        use skein_forge::forgejo::document::Body;
        let (repository, change, number, branch, commit, by, body, ended, queue) = match hook {
            skein_fake_forge_domain::Request::Reply { .. } => return Ok(None),
            skein_fake_forge_domain::Request::Hook {
                repository,
                change,
                number,
                branch,
                commit,
                by,
                body,
                ended,
                queue,
            } => (repository, change, number, branch, commit, by, body, ended, queue),
        };
        let mut known = None;
        for (id, row) in &self.repositories {
            let prefix = crate::service::joined(&row.owner, b"/").ok_or(Problem::Config)?;
            let full = crate::service::joined(&prefix, &row.name);
            if full.as_deref() == Some(repository.as_ref()) {
                known = Some(*id);
            }
        }
        let Some(id) = known else { return Err(Problem::Config) };
        let repository = u64::from(id);
        let payload = match change {
            Change::Issue => Delivery::Issue {
                repository,
                number: number.ok_or(Problem::Config)?,
                body: Body::Kept(body.clone().ok_or(Problem::Config)?),
            },
            Change::Comment => Delivery::Comment {
                repository,
                number: number.ok_or(Problem::Config)?,
                body: Body::Kept(body.clone().ok_or(Problem::Config)?),
            },
            Change::Review => Delivery::Review {
                repository,
                number: number.ok_or(Problem::Config)?,
                body: Body::Kept(body.clone().ok_or(Problem::Config)?),
            },
            Change::Pull => {
                let (action, reason): (&[u8], Option<Box<[u8]>>) = match queue {
                    None => (b"opened".as_slice(), None),
                    Some(QueueHook::Enqueued) => (b"enqueued", None),
                    Some(QueueHook::Dequeued(reason)) => (
                        b"dequeued",
                        Some(bytes::copy_of(match reason {
                            QueueRemoval::CiFailure => crate::texts::CI_FAILURE,
                            QueueRemoval::MergeConflict => crate::texts::MERGE_CONFLICT,
                        })),
                    ),
                };
                Delivery::Pull {
                    repository,
                    number: number.ok_or(Problem::Config)?,
                    action: Pull { action: bytes::copy_of(action), reason },
                }
            }
            Change::Push => Delivery::Push {
                repository,
                branch: branch.clone().ok_or(Problem::Config)?,
                after: match commit {
                    Some(count) => crate::reads::commit(*count),
                    None => [0; 32],
                },
                deleted: commit.is_none(),
                sender: by.ok_or(Problem::Config)?,
            },
            Change::Group { ended } => Delivery::Ci {
                repository,
                commit: crate::reads::commit(commit.ok_or(Problem::Config)?),
                event: if *ended { Ci::Destroyed } else { Ci::ChecksRequested },
            },
            Change::Job => Delivery::Ci {
                repository,
                commit: crate::reads::commit(ended.ok_or(Problem::Config)?.commit),
                event: Ci::WorkflowJob,
            },
        };
        let mut installation = None;
        for row in self.installations.iter() {
            if self.installations.reaches(
                &crate::installations::Grant { installation: row.id, scope: crate::installations::Scope::Full },
                id,
            ) {
                installation = Some(row.id);
                break;
            }
        }
        let (_, post) = self.release(payload, installation, now)?;
        Ok(post)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn github_fake_full_delivery_store_fits_its_heap_bound() {
        let mut config = Config::standard();
        config.log_entries = 3;
        config.payload_size = 6000;
        let limits = Limits { entries: 3, payload_bytes: 49152 };
        let bound = worst_case(&config, limits, 256).unwrap();
        let mut store = Store::new(config, limits, 7);
        let secret = Secret::new(bytes::copy_of(b"invented-hook-secret"));
        let bounds = crate::tests::fixtures::limits().github;
        for _ in 0_u32..3 {
            store
                .record(
                    Delivery::Issue {
                        repository: 2,
                        number: 3,
                        body: skein_forge::forgejo::document::Body::Kept(bytes::copy_of(b"hello")),
                    },
                    Some(1),
                    Wall::EPOCH,
                    &secret,
                    &bounds,
                    None,
                )
                .unwrap();
        }
        assert_eq!(store.entries.len(), store.entries.capacity());
        let mut held = u64::try_from(size_of_val(store.entries.as_slice())).unwrap();
        for entry in store.entries.as_slice().iter().flatten() {
            held = held.checked_add(u64::try_from(entry.row.event.len()).unwrap()).unwrap();
            if let Some(action) = &entry.row.action {
                held = held.checked_add(u64::try_from(action.len()).unwrap()).unwrap();
            }
            let Delivery::Issue { body: skein_forge::forgejo::document::Body::Kept(body), .. } = &entry.payload else {
                panic!("filled issue payload")
            };
            assert_eq!(body.len(), 6000);
            held = held.checked_add(u64::try_from(body.len()).unwrap()).unwrap();
        }
        assert!(held <= bound);
        let mut config = Config::standard();
        config.log_entries = u32::MAX;
        assert_eq!(worst_case(&config, Limits { entries: u32::MAX, payload_bytes: u32::MAX }, 256), None);
    }
}
