//! The in-process GitHub layer (fake-forge.md, section 10).
use crate::{
    answers::{self, Body, Page},
    app::{self, App},
    auth::{Access, Bearer},
    installations::{self, Grant, Installations, Kind, Mint, Scope},
    manifest::Manifest,
    pages::{self, Answer, Page as BrowserPage},
    reads, routes, writes,
};
use alloc::boxed::Box;
use skein_fake_forge_domain as fake;
use skein_forge::github::{self, document as d, request, server};
use skein_lib::{List, Map, Overflow, ReplyTo, Rng, Secret, Wall, Writer};

/// GitHub's repository metadata absent from the shared domain.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    /// The domain's nonzero repository id.
    pub id: u32,
    /// Owner login.
    pub owner: Box<[u8]>,
    /// Owner's numeric id.
    pub owner_id: u64,
    /// Whether the owner is an organization.
    pub organization: bool,
    /// Repository name without the owner.
    pub name: Box<[u8]>,
    /// Whether the repository is private.
    pub private: bool,
}
/// A scenario account that can own or install the App.
#[derive(Debug)]
pub struct Account {
    pub login: Box<[u8]>,
    pub id: u64,
    pub organization: bool,
}
/// How a scenario presents the converted App's permissions and subscriptions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppFacts {
    /// Keep every permission and event the manifest posts.
    Complete,
    /// Keep the grants, but omit both fields from GET /app.
    Omitted,
    /// Keep only the first permission at read access, and the first event.
    Narrow,
}
/// How the scenario shows domain protection (fake-forge.md, 10.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shown {
    Ruleset,
    Classic,
    Both,
}
/// The scenario's GitHub-only tables, including the signed-in browser account.
#[derive(Debug)]
pub struct Config {
    pub deliveries: crate::delivery::Config,
    /// Requests per token identity per forge-clock window.
    pub rate_quota: u32,
    pub rate_window: skein_lib::Duration,
    /// Life of every minted installation token, nonzero on the forge clock.
    pub token_life: skein_lib::Duration,
    /// Origin used by pagination links; a socket host sets its bound origin before serving.
    pub base: Box<[u8]>,
    /// Blob origin; a peer replaces it with its bound second listener.
    pub blob: Box<[u8]>,
    /// Optional unfamiliar origin for the API job-log redirect.
    pub third_origin: Option<Box<[u8]>>,
    /// Optional destination of a second redirect from the blob listener.
    pub blob_redirect: Option<Box<[u8]>>,
    /// App identity and test signing key.
    pub app: App,
    /// Grants at conversion and fields shown by GET /app.
    pub app_facts: AppFacts,
    /// Domain user acting for every installation.
    pub bot: u64,
    pub shown: Shown,
    /// Stable ruleset id supplied by the scenario, shared across its repository routes.
    pub ruleset_id: u64,
    /// Report `PullsOnly` for a bypassing App bot; otherwise `Always`.
    pub pull_requests_only: bool,
    /// A layer fact: the domain has no signing engine.
    pub required_signatures: bool,
    /// Initial webhook URL.
    pub hook: Box<[u8]>,
    /// Repository rows, in any order; ids and owner/name pairs must be unique.
    pub repositories: List<Row>,
    /// Scenario accounts, unique by both login and id, bounded by `github.inner`.
    pub accounts: List<Account>,
    /// Login of the signed-in browser user, from accounts.
    pub person: Box<[u8]>,
    /// Person secrets, domain user numbers and optional expiry, bounded by github.inner.
    pub tokens: List<(Secret, u64, Option<Wall>)>,
    /// Per-mille byte faults, using the Forgejo face's scenario vocabulary.
    pub faults: crate::Faults,
}
/// All retained tables and transient input/output are bounded here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub deliveries: crate::delivery::Limits,
    /// HTTP framing and per-connection input/output bounds.
    pub http: skein_http::server::Limits,
    /// The dialect's document and target bounds; inner caps repository rows.
    pub github: github::limits::Limits,
    /// Installation, token and per-installation reach capacities.
    pub installations: installations::Limits,
    /// Manifest records and the bytes admitted per manifest.
    pub pages: pages::Limits,
    /// Effective page cap, from one through GitHub's `PAGE_CAP`.
    pub page: u32,
    /// Maximum request body, no larger than `github.body_bytes`.
    pub body: u32,
    /// Maximum response body and, separately, Link or Location value (at least 128 bytes).
    pub response: u32,
    /// Recent blob requests retained in a bounded ring.
    pub blob_records: u32,
    /// Recent private git requests retained, oldest first; nonzero.
    pub git_records: u32,
}
/// An unusable limit or an invalid scenario table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Problem {
    Limits,
    Config,
}

/// One request above HTTP parsing. Debug never exposes the credential or body.
pub struct Request<'a> {
    pub validator: Option<&'a [u8]>,
    /// HTTP method.
    pub method: request::Method,
    /// Canonical dialect target, including paging where required.
    pub target: &'a [u8],
    /// Whole Authorization value: Bearer for REST, Basic for private git.
    pub authorization: Option<&'a [u8]>,
    /// X-GitHub-Api-Version value.
    pub version: Option<&'a [u8]>,
    /// Whether a User-Agent was supplied.
    pub agent: bool,
    /// Whole bounded request body.
    pub body: &'a [u8],
}
impl core::fmt::Debug for Request<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Request")
            .field("validator", &self.validator)
            .field("method", &self.method)
            .field("target_bytes", &self.target.len())
            .field("authorized", &self.authorization.is_some())
            .field("version", &self.version)
            .field("agent", &self.agent)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}
/// The finite media types served by the API and browser routes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContentType {
    Json,
    Octets,
    Html,
    Plaintext,
}
impl ContentType {
    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Json => b"application/json",
            Self::Octets => b"application/octet-stream",
            Self::Html => b"text/html",
            Self::Plaintext => b"text/plain",
        }
    }
}
/// An encoded document, with no HTTP framing. Debug redacts minted tokens.
pub struct Response {
    pub rate: Option<crate::rates::Heads>,
    pub etag: Option<Box<[u8]>>,
    /// GitHub status.
    pub status: u16,
    /// Media type of this route's body.
    pub content_type: ContentType,
    /// Optional absolute next and last links.
    pub link: Option<Box<[u8]>>,
    /// Redirect target, framed through the same Location path as browser pages.
    pub location: Option<Box<[u8]>>,
    /// JSON document, file/log bytes, or empty on a redirect or 204.
    pub body: Box<[u8]>,
}
impl core::fmt::Debug for Response {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Response")
            .field("rate", &self.rate)
            .field("etag", &self.etag)
            .field("status", &self.status)
            .field("content_type", &self.content_type)
            .field("link", &self.link)
            .field("redirect", &self.location.is_some())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}
/// An immediate reply or the next shared-domain call in this request.
#[derive(Debug)]
pub enum Outcome {
    /// No domain call was needed.
    Reply(Response),
    /// Authenticated token request awaiting one domain-window admission.
    Admit(crate::rates::Admission),
    /// Caller owns the continuation and routes the event's reply to finish.
    Call(Pending, fake::Event),
}
/// One caller-owned continuation; it contains no credential or reply right.
/// The supplied `ReplyTo` moves only into `Event::Call`. The caller bounds its
/// in-flight calls and owns their pending values, as it owns the domain replies.
#[derive(Debug)]
pub struct Pending {
    pub(crate) kind: PendingKind,
    pub(crate) rate: Option<crate::rates::Heads>,
}
#[derive(Debug)]
pub(crate) enum PendingKind {
    Mutation(crate::mutations::Mutation),
    CommentEdit(crate::comment_edit::Edit),
    Read(reads::Pending),
    Enqueue(crate::enqueue::Pending),
    Conditional { read: reads::Pending, admission: Box<crate::rates::Admission>, validator: Box<[u8]> },
    Write(writes::Pending),
    Blob,
    Git(crate::git::Pending),
    EditReach { installation: u64, repository: u32, add: bool },
}
/// GitHub's own state, with no clock, socket or HTTP parser.
#[derive(Debug)]
pub struct Service {
    pub(crate) rate_heads: Map<fake::RateKey, crate::rates::Heads>,
    pub(crate) rate_quota: u32,
    pub(crate) rate_window: skein_lib::Duration,
    pub(crate) deliveries: crate::delivery::Store,
    pub(crate) key_revoked: bool,
    pub(crate) verified: app::Verified,
    pub(crate) posts: skein_lib::Queue<crate::delivery::Post>,
    pub(crate) base: Box<[u8]>,
    pub(crate) logs: crate::logs::Store,
    pub(crate) git_calls: skein_lib::Queue<crate::GitCall>,
    pub(crate) app: App,
    pub(crate) app_facts: AppFacts,
    pub(crate) bot: u64,
    pub(crate) shown: Shown,
    pub(crate) ruleset_id: u64,
    pub(crate) pull_requests_only: bool,
    pub(crate) required_signatures: bool,
    pub(crate) hook: Box<[u8]>,
    pub(crate) accounts: List<Account>,
    pub(crate) person: Box<[u8]>,
    pub(crate) tokens: List<(Secret, u64, Option<Wall>)>,
    pub(crate) faults: crate::Faults,
    pub(crate) rng: Rng,
    pub(crate) pages: pages::Store,
    pub(crate) repositories: Map<u32, Row>,
    pub(crate) installations: Installations,
    pub(crate) limits: Limits,
}
impl Service {
    /// Validates the tables and limits before creating bounded stores.
    pub fn new(config: Config, limits: &Limits, seed: u64) -> Result<Self, Problem> {
        if worst_case(limits, &config).is_none() {
            return Err(Problem::Limits);
        }
        let string = usize::try_from(limits.github.string_bytes).or(Err(Problem::Limits))?;
        if !valid_faults(config.faults)
            || config.token_life == skein_lib::Duration::ZERO
            || config.rate_quota == 0
            || config.rate_window == skein_lib::Duration::ZERO
            || config.ruleset_id == 0
            || !valid_origin(&config.base, limits.github.target_bytes)
            || !crate::logs::origin_fits(&config.blob, limits)
            || !crate::logs::optional_origin_fits(config.third_origin.as_deref(), limits)
            || !crate::logs::optional_origin_fits(config.blob_redirect.as_deref(), limits)
            || config.bot == 0
            || config.bot == config.app.id
            || config.app.id == 0
            || config.app.slug.is_empty()
            || config.app.slug.len().checked_add(b"[bot]".len()).ok_or(Problem::Config)? > string
            || config.hook.len() > string
            || config.repositories.len() > limits.github.inner
            || config.app.permissions.len() > usize::try_from(limits.github.inner).or(Err(Problem::Limits))?
            || config.app.events.len() > usize::try_from(limits.github.inner).or(Err(Problem::Limits))?
        {
            return Err(Problem::Config);
        }
        for permission in &config.app.permissions {
            if permission.name.len() > string {
                return Err(Problem::Config);
            }
        }
        for event in &config.app.events {
            if event.len() > string {
                return Err(Problem::Config);
            }
        }
        repository_rows(&config.repositories, string)?;
        check_accounts(&config, limits, string)?;
        crate::auth::check(&config, limits)?;
        let mut repositories = Map::with_capacity(limits.github.inner);
        for row in config.repositories.into_boxed() {
            if repositories.insert(row.id, row).is_err() {
                return Err(Problem::Config);
            }
        }
        let post_capacity = config.deliveries.log_entries;
        Ok(Self {
            logs: crate::logs::Store::new(
                config.blob,
                config.third_origin,
                config.blob_redirect,
                limits.blob_records,
                seed,
            ),
            git_calls: skein_lib::Queue::with_capacity(limits.git_records),
            rate_heads: Map::with_capacity(limits.github.inner),
            rate_quota: config.rate_quota,
            rate_window: config.rate_window,
            deliveries: crate::delivery::Store::new(config.deliveries, limits.deliveries, seed),
            key_revoked: false,
            verified: app::Verified::empty(),
            posts: skein_lib::Queue::with_capacity(post_capacity),
            base: config.base,
            app: config.app,
            app_facts: config.app_facts,
            bot: config.bot,
            shown: config.shown,
            ruleset_id: config.ruleset_id,
            pull_requests_only: config.pull_requests_only,
            required_signatures: config.required_signatures,
            hook: config.hook,
            accounts: config.accounts,
            person: config.person,
            tokens: config.tokens,
            faults: config.faults,
            rng: Rng::new(seed),
            pages: pages::Store::new(limits.pages, seed),
            repositories,
            installations: Installations::with_life(&limits.installations, seed, config.token_life),
            limits: *limits,
        })
    }
    /// Scenario access to the delivery engine. No endpoint records a failed 502.
    pub fn release(
        &mut self,
        payload: github::delivery::Delivery,
        installation: Option<u64>,
        now: Wall,
    ) -> Result<(u64, Option<crate::delivery::Post>), Problem> {
        self.deliveries.record(payload, installation, now, &self.app.webhook, &self.limits.github, None)
    }
    pub fn redeliver(&mut self, id: u64, now: Wall) -> Result<(u64, Option<crate::delivery::Post>), Problem> {
        let (payload, installation, guid) = self.deliveries.redelivery(id).ok_or(Problem::Config)?;
        self.deliveries.record(payload, installation, now, &self.app.webhook, &self.limits.github, Some(guid))
    }
    pub fn delivered(&mut self, id: u64, status: u16) {
        self.deliveries.complete(id, status);
    }
    pub fn revoke_key(&mut self) {
        self.key_revoked = true;
        self.verified.forget();
    }
    #[must_use]
    pub fn endpoint(&self) -> Option<&crate::delivery::Endpoint> {
        self.deliveries.endpoint()
    }
    pub fn poll(&mut self, now: Wall) {
        self.installations.clock(now);
        for _ in 0..self.installations.event_capacity() {
            if self.posts.room() == 0 {
                break;
            }
            let Some(event) = self.installations.take_event() else { break };
            let (_, post) = self
                .deliveries
                .record(event.payload, None, event.at, &self.app.webhook, &self.limits.github, None)
                .expect("installation payload fits admitted delivery bound");
            if let Some(post) = post {
                self.posts.push(post);
            }
        }
        self.deliveries.poll(now);
    }
    pub fn next_post(&mut self) -> Option<crate::delivery::Post> {
        self.posts.pop()
    }
    #[must_use]
    pub fn next_deadline(&self) -> Option<Wall> {
        self.deliveries.next_deadline()
    }
    /// Scenario access to the bounded store, without touching the domain.
    pub fn installations_mut(&mut self) -> &mut Installations {
        &mut self.installations
    }
    /// Installation state, in id order.
    #[must_use]
    pub const fn installations(&self) -> &Installations {
        &self.installations
    }

    /// Serves browser pages without API headers or a credential, and never calls the domain.
    pub fn page(&mut self, page: &BrowserPage<'_>, now: Wall) -> Answer {
        self.poll(now);
        pages::serve(self, page, now)
    }
    /// The posted manifest stays readable after conversion until its record is reclaimed.
    #[must_use]
    pub fn manifest(&self, state: &[u8]) -> Option<&Manifest> {
        match self.pages.states.get(state) {
            Some(record) => Some(&record.manifest),
            None => None,
        }
    }

    /// Checks User-Agent, version, route, credential, installation state/reach,
    /// then the body. Every refusal precedes a domain call. The caller completes
    /// admission and all continuations before serving another request, so the
    /// resolved bearer slots remain valid through dispatch.
    pub fn request(&mut self, request: &Request<'_>, now: Wall, reply_to: ReplyTo) -> Outcome {
        self.poll(now);
        if let Some(name) = skein_fake_forge_protocol::git::name(request.target) {
            return self.git_request(request, name, now, reply_to, None);
        }
        let outcome = self.request_inner(request, now, reply_to);
        self.local_links(outcome)
    }
    fn request_inner(&mut self, request: &Request<'_>, now: Wall, reply_to: ReplyTo) -> Outcome {
        let limits = &self.limits;
        if !request.agent {
            return Outcome::Reply(answers::refusal(403, b"User agent required", limits));
        }
        if request.version != Some(github::pin::API_VERSION.as_bytes()) {
            return Outcome::Reply(answers::refusal(400, b"Invalid API version", limits));
        }
        let Some(route) = routes::decode(request.method, request.target, &limits.github) else {
            return Outcome::Reply(answers::missing(limits));
        };
        match &route {
            routes::Route::BotUser(slug) => {
                return Outcome::Reply(if slug == &self.app.slug {
                    answers::response(200, &Body::BotUser(self.bot, slug), limits)
                } else {
                    answers::missing(limits)
                });
            }
            routes::Route::Conversion(code) => {
                return Outcome::Reply(pages::convert(self, code.expose(), request.body, now));
            }
            routes::Route::App(_) | routes::Route::Token(_) => {}
        }
        let bearer = match request.authorization {
            Some(header) => header.strip_prefix(b"Bearer "),
            None => None,
        };
        let Some(bearer) = bearer else {
            return Outcome::Reply(answers::unauthorized(limits));
        };
        match route {
            routes::Route::Conversion(_) | routes::Route::BotUser(_) => Outcome::Reply(answers::failed()),
            routes::Route::App(route) => {
                if let Some(bearer) = self.resolve(bearer, now) {
                    return Outcome::Admit(crate::rates::Admission::new(
                        crate::rates::Route::App,
                        bearer,
                        request,
                        now,
                        self.limits.body,
                    ));
                }
                if self.key_revoked || self.verified.check(bearer, &self.app, now).is_err() {
                    return Outcome::Reply(answers::unauthorized(limits));
                }
                Outcome::Reply(self.app_route(route, request, now))
            }
            routes::Route::Token(route) => {
                let Some(bearer) = self.resolve(bearer, now) else {
                    return Outcome::Reply(answers::unauthorized(limits));
                };
                if let routes::Token::Read(reads::Read::Locate { .. } | reads::Read::Repository(_)) = &route
                    && let Some(validator) = request.validator
                    && validator.len() == reads::REPOSITORY_ETAG_BYTES
                {
                    let admission = crate::rates::Admission::new(
                        crate::rates::Route::Token(route),
                        bearer,
                        request,
                        now,
                        self.limits.body,
                    );
                    return self.conditional(admission, validator, reply_to);
                }
                Outcome::Admit(crate::rates::Admission::new(
                    crate::rates::Route::Token(route),
                    bearer,
                    request,
                    now,
                    self.limits.body,
                ))
            }
        }
    }
    pub(crate) fn token_route(
        &mut self,
        route: routes::Token,
        now: Wall,
        bearer: Bearer,
        request: &Request<'_>,
        reply_to: ReplyTo,
    ) -> Outcome {
        let access = match self.access(bearer) {
            Ok(access) => access,
            Err(response) => return Outcome::Reply(response),
        };
        let limits = &self.limits;
        match route {
            routes::Token::CommentEdit(route) => crate::comment_edit::call(self, route, access, request.body, reply_to),
            routes::Token::Mutation(route) => crate::mutations::call(self, route, access, request.body, reply_to),
            routes::Token::Reachable(page) => crate::auth::reachable(self, page, access, request),
            routes::Token::Identity => Outcome::Reply(match access {
                Access::Person(user) => reads::user(self, user, self.body_fits(request.body)),
                Access::Installation { .. } => answers::refusal(403, crate::texts::INTEGRATION, limits),
            }),
            routes::Token::Revoke => Outcome::Reply(match bearer {
                Bearer::Person { .. } => answers::unauthorized(limits),
                Bearer::Installation { slot, .. } => {
                    self.installations.revoke_slot(slot);
                    crate::auth::empty()
                }
            }),
            routes::Token::EditReach { installation, repository, add } => {
                crate::auth::edit_reach(self, access, installation, repository, add, reply_to)
            }
            routes::Token::User(id) => Outcome::Reply(if access.contents() {
                answers::refusal(403, crate::texts::INTEGRATION, limits)
            } else {
                reads::user(self, id, self.body_fits(request.body))
            }),
            routes::Token::Graphql => self.graphql(request, &access, reply_to),
            routes::Token::Write(route) => writes::call(self, route, &access, request.body, reply_to),
            routes::Token::Read(read) => {
                reads::call(self, read, &access, request.target, self.body_fits(request.body), now, reply_to)
            }
        }
    }
    fn graphql(&self, request: &Request<'_>, grant: &Access, reply_to: ReplyTo) -> Outcome {
        let limits = &self.limits;
        if !self.body_fits(request.body) {
            return Outcome::Reply(answers::response(200, &Body::Graphql, limits));
        }
        if grant.contents() {
            return Outcome::Reply(answers::response(
                200,
                &Body::GraphqlRefusal(b"FORBIDDEN", crate::texts::INTEGRATION),
                limits,
            ));
        }
        let enqueue = request::Request::Enqueue { id: Box::from([]), head: [0; 32] };
        if let Some(request::Request::Enqueue { id, head }) = server::write_body(enqueue, request.body, &limits.github)
        {
            return reads::enqueue(self, &id, head, grant, reply_to);
        }
        match server::graphql_body(request.body, &limits.github) {
            Some(server::Graphql::Viewer) => Outcome::Reply(self.viewer(grant.user())),
            Some(server::Graphql::Pull { owner, name, number }) => reads::call(
                self,
                reads::Read::PullGraphql { owner, name, number },
                grant,
                request.target,
                self.body_fits(request.body),
                Wall::EPOCH,
                reply_to,
            ),
            None => Outcome::Reply(answers::response(200, &Body::Graphql, limits)),
        }
    }

    /// Continues the request after a domain reply. The caller must finish all
    /// its admission and calls before handling another request, so handles, checks
    /// and writes agree.
    /// The domain reply returns the right supplied for the preceding call.
    pub fn finish(
        &mut self,
        pending: Pending,
        result: Result<fake::api::Answer, fake::api::Error>,
        reply_to: ReplyTo,
        domain: &fake::Domain,
    ) -> Outcome {
        let rate = pending.rate;
        let outcome = match pending.kind {
            PendingKind::CommentEdit(pending) => crate::comment_edit::finish(self, pending, result, reply_to),
            PendingKind::Mutation(pending) => Outcome::Reply(crate::mutations::finish(self, pending, result, domain)),
            PendingKind::Enqueue(pending) => crate::enqueue::finish(self, pending, result, reply_to, domain),
            PendingKind::Read(pending) => Outcome::Reply(reads::finish(pending, result, self, domain)),
            PendingKind::Conditional { read, admission, validator } => {
                let mut response = reads::finish(read, result, self, domain);
                if response.status == 200
                    && response.etag.as_deref() == Some(validator.as_ref())
                    && let Some(heads) = self.rate_heads.get(&admission.key())
                    && heads.rate.reset > admission.now()
                {
                    response.status = 304;
                    response.body = Box::new([]);
                    response.rate = Some(*heads);
                    return Outcome::Reply(response);
                }
                Outcome::Admit(*admission)
            }
            PendingKind::Write(pending) => writes::finish(self, pending, result, reply_to, domain),
            PendingKind::Git(pending) => Outcome::Reply(crate::git::finish(self, pending, result)),
            PendingKind::Blob => Outcome::Reply(crate::logs::finish(result, &self.limits)),
            PendingKind::EditReach { installation, repository, add } => {
                Outcome::Reply(crate::auth::finish_reach(self, installation, repository, add, result))
            }
        };
        crate::rates::attach(self.local_links(outcome), rate)
    }
    /// A host supplies its bound local origin before it accepts requests.
    pub fn set_origin(&mut self, base: Box<[u8]>) -> Result<(), Problem> {
        if !valid_origin(&base, self.limits.github.target_bytes) {
            return Err(Problem::Config);
        }
        self.base = base;
        Ok(())
    }
    pub(crate) fn local_links(&self, outcome: Outcome) -> Outcome {
        let mut response = match outcome {
            Outcome::Reply(response) => response,
            call @ (Outcome::Call(_, _) | Outcome::Admit(_)) => return call,
        };
        if let Some(link) = response.link.take() {
            // The dialect renders its pinned public origin. The fake's Config
            // supplies the origin for every link, including continuation replies.
            let mut out = Writer::new(usize::try_from(self.limits.response).expect("bounded response"));
            let mut rest = link.as_ref();
            // A numbered page has at most next and last links.
            for _ in 0..2u32 {
                let Some(at) = skein_lib::bytes::find(rest, b"<https://api.github.com") else { break };
                let Some(prefix) = rest.get(..at) else { return Outcome::Reply(answers::failed()) };
                if local_link_chunk(&mut out, prefix, &self.base).is_err() {
                    return Outcome::Reply(answers::failed());
                }
                let Some(tail) = rest.get(at.saturating_add(b"<https://api.github.com".len())..) else {
                    return Outcome::Reply(answers::failed());
                };
                rest = tail;
            }
            if out.put(rest).is_err() {
                return Outcome::Reply(answers::failed());
            }
            response.link = answers::finish(out);
        }
        Outcome::Reply(response)
    }
    fn app_route(&mut self, route: routes::App, request: &Request<'_>, now: Wall) -> Response {
        if !self.body_fits(request.body) {
            return match route {
                routes::App::Mint(id) => self.mint(id, request.body, now),
                routes::App::SetHook => self.set_hook(request.body),
                routes::App::Installation(id) | routes::App::Uninstall(id) => {
                    if self.installations.get(id).is_none() {
                        answers::missing(&self.limits)
                    } else {
                        answers::refusal(413, b"Payload Too Large", &self.limits)
                    }
                }
                routes::App::Identity
                | routes::App::Hook
                | routes::App::Installations(_)
                | routes::App::LogPage { .. }
                | routes::App::LogDelivery(_) => answers::refusal(413, b"Payload Too Large", &self.limits),
            };
        }
        match route {
            routes::App::LogPage { ask, per_page } => {
                self.deliveries.page(&ask, per_page, request.validator, &self.base, &self.limits)
            }
            routes::App::LogDelivery(id) => self.deliveries.get(id, &self.limits),
            routes::App::Identity => self.app_identity(),
            routes::App::Hook => answers::response(200, &Body::Hook(&d::Hook { url: self.hook.clone() }), &self.limits),
            routes::App::SetHook => self.set_hook(request.body),
            routes::App::Installations(page) => self.installation_page(page, request.target),
            routes::App::Installation(id) => match self.installations.get(id) {
                Some(row) => answers::response(200, &Body::Installation(&installation(row)), &self.limits),
                None => answers::missing(&self.limits),
            },
            routes::App::Mint(id) => self.mint(id, request.body, now),
            routes::App::Uninstall(id) => {
                if self.installations.remove(id) {
                    Response {
                        rate: None,
                        etag: None,
                        content_type: ContentType::Json,
                        status: 204,
                        link: None,
                        location: None,
                        body: Box::from([]),
                    }
                } else {
                    answers::missing(&self.limits)
                }
            }
        }
    }
    fn app_identity(&self) -> Response {
        let app = d::App {
            id: self.app.id,
            slug: self.app.slug.clone(),
            permissions: self.app.permissions.clone(),
            events: self.app.events.clone(),
        };
        let body = match self.app_facts {
            AppFacts::Complete | AppFacts::Narrow => Body::App(&app),
            AppFacts::Omitted => Body::AppIdentity(&app),
        };
        answers::response(200, &body, &self.limits)
    }
    fn mint(&mut self, id: u64, body: &[u8], now: Wall) -> Response {
        let Some(row) = self.installations.get(id) else {
            return answers::missing(&self.limits);
        };
        if row.suspended {
            return answers::refusal(403, answers::SUSPENDED, &self.limits);
        }
        let Some(scope) = server::mint_scope(body) else {
            return answers::refusal(422, b"Validation Failed", &self.limits);
        };
        if !self.body_fits(body) {
            return answers::refusal(422, b"Validation Failed", &self.limits);
        }
        let scope = match scope {
            request::Scope::Full => Scope::Full,
            request::Scope::Contents => Scope::Contents,
        };
        match self.installations.mint(id, scope, now) {
            Ok((token, expires)) => answers::response(
                201,
                &Body::Minted(&d::Minted { token, expires }, &self.app.permissions, scope),
                &self.limits,
            ),
            Err(Mint::Missing) => answers::missing(&self.limits),
            Err(Mint::Suspended) => answers::refusal(403, answers::SUSPENDED, &self.limits),
            Err(Mint::Full) => answers::refusal(503, b"Server Error", &self.limits),
        }
    }
    pub(crate) fn body_fits(&self, body: &[u8]) -> bool {
        match u32::try_from(body.len()) {
            Ok(length) => length <= self.limits.body,
            Err(_) => false,
        }
    }
    fn set_hook(&mut self, body: &[u8]) -> Response {
        if !self.body_fits(body) {
            return answers::refusal(422, b"Validation Failed", &self.limits);
        }
        let Some(url) = server::hook_url(body, &self.limits.github) else {
            return answers::refusal(422, b"Validation Failed", &self.limits);
        };
        // The hook reader also represents an absent URL as empty; an update
        // needs a usable value and must not erase the current address.
        if url.is_empty() {
            return answers::refusal(422, b"Validation Failed", &self.limits);
        }
        let response = answers::response(200, &Body::Hook(&d::Hook { url: url.clone() }), &self.limits);
        if response.status == 200 {
            self.hook = url;
        }
        response
    }
    fn viewer(&self, user: u64) -> Response {
        let login = if user == self.bot { joined(&self.app.slug, b"[bot]") } else { reads::people::login(self, user) };
        let Some(login) = login else {
            return answers::failed();
        };
        answers::response(200, &Body::Viewer(&d::Viewer { id: user, login }), &self.limits)
    }
    fn installation_page(&self, page: u32, target: &[u8]) -> Response {
        let Ok(total) = u64::try_from(self.installations.iter().count()) else {
            return answers::failed();
        };
        let Some(page) = Page::new(page, self.limits.page, total) else {
            return answers::failed();
        };
        let mut rows = List::with_capacity(self.limits.page);
        for (index, row) in self.installations.iter().enumerate() {
            let Ok(index) = u64::try_from(index) else {
                return answers::failed();
            };
            if page.contains(index) && rows.push(installation(row)).is_err() {
                return answers::failed();
            }
        }
        page.link(answers::response(200, &Body::Installations(rows.as_slice()), &self.limits), target, &self.limits)
    }
    // The fake pages by its own limits.page, independent of the request's size.
    pub(crate) fn reachable(&self, number: u32, grant: &Grant, target: &[u8]) -> Response {
        let mut total = 0_u64;
        for (id, _) in &self.repositories {
            if self.reaches(grant, *id) {
                let Some(next) = total.checked_add(1) else {
                    return answers::failed();
                };
                total = next;
            }
        }
        let Some(page) = Page::new(number, self.limits.page, total) else {
            return answers::failed();
        };
        let mut index = 0_u64;
        let mut rows = List::with_capacity(self.limits.page);
        for (id, row) in &self.repositories {
            if !self.reaches(grant, *id) {
                continue;
            }
            if page.contains(index)
                && rows
                    .push(d::Reachable {
                        id: u64::from(*id),
                        owner: row.owner_id,
                        login: row.owner.clone(),
                        name: row.name.clone(),
                        private: row.private,
                    })
                    .is_err()
            {
                return answers::failed();
            }
            let Some(next) = index.checked_add(1) else {
                return answers::failed();
            };
            index = next;
        }
        page.link(answers::response(200, &Body::Reachable(rows.as_slice(), total), &self.limits), target, &self.limits)
    }
}
fn installation(row: &installations::Installation) -> d::Installation {
    d::Installation {
        id: row.id,
        login: row.login.clone(),
        account: row.account,
        kind: match row.kind {
            Kind::User => d::Account::User,
            Kind::Organization => d::Account::Organization,
        },
        created: row.created,
        suspended: row.suspended,
    }
}
pub(crate) fn joined(first: &[u8], last: &[u8]) -> Option<Box<[u8]>> {
    let mut out = Writer::new(first.len().checked_add(last.len())?);
    out.put(first).ok()?;
    out.put(last).ok()?;
    Some(out.finish())
}

/// The layer's service scratch and one HTTP connection, including caller-owned
/// continuations and framing buffers. A host adds one connection envelope per
/// additional connection and budgets the shared domain's replies separately,
/// including domain payloads moved into a continuation.
#[must_use]
pub fn worst_case(limits: &Limits, config: &Config) -> Option<u64> {
    if config.rate_quota == 0
        || config.rate_window == skein_lib::Duration::ZERO
        || github::limits::check(&limits.github).is_err()
        || limits.page == 0
        || limits.page > github::pin::PAGE_CAP
        || limits.page > limits.github.inner
        || limits.body == 0
        || limits.body > limits.github.body_bytes
        || limits.pages.states == 0
        || limits.pages.manifest_bytes == 0
        || limits.response < 128
        || limits.blob_records == 0
        || limits.git_records == 0
        || limits.github.inner.checked_mul(4).is_none()
    {
        return None;
    }
    let mut bytes = crate::server::worst_case(limits)?
        .checked_add(Map::<fake::RateKey, crate::rates::Heads>::worst_case(limits.github.inner)?)?
        .checked_add(List::<u32>::worst_case(limits.installations.listed)?)?
        .checked_add(skein_lib::Queue::<crate::delivery::Post>::worst_case(config.deliveries.log_entries)?)?
        .checked_add(
            u64::from(config.deliveries.log_entries).checked_mul(
                u64::from(limits.deliveries.payload_bytes)
                    .checked_add(u64::from(limits.github.string_bytes).checked_mul(2)?)?
                    .checked_add(512)?,
            )?,
        )?
        .checked_add(List::<github::log::Row>::worst_case(limits.github.inner.min(100))?)?
        .checked_add(
            u64::from(limits.github.inner.min(100))
                .checked_mul(u64::from(limits.deliveries.payload_bytes).checked_add(128)?)?,
        )?
        .checked_add(crate::delivery::worst_case(&config.deliveries, limits.deliveries, limits.github.string_bytes)?)?
        .checked_add(u64::from(limits.github.target_bytes))?
        .checked_add(pages::worst_case(limits)?)?
        .checked_add(crate::logs::worst_case(limits)?)?
        .checked_add(skein_lib::Queue::<crate::GitCall>::worst_case(limits.git_records)?)?
        .checked_add(List::<Account>::worst_case(config.accounts.capacity())?)?;
    for account in &config.accounts {
        bytes = bytes.checked_add(u64::try_from(account.login.len()).ok()?)?;
    }
    bytes =
        bytes
            .checked_add(u64::try_from(config.person.len()).ok()?)?
            .checked_add(List::<(Secret, u64, Option<Wall>)>::worst_case(config.tokens.capacity())?)?;
    for (secret, _, _) in &config.tokens {
        bytes = bytes.checked_add(u64::try_from(secret.expose().len()).ok()?)?;
    }
    bytes = bytes
        .checked_add(Installations::worst_case(&limits.installations)?)?
        .checked_add(Map::<u32, Row>::worst_case(limits.github.inner)?)?
        .checked_add(List::<Row>::worst_case(config.repositories.capacity())?)?;
    for row in &config.repositories {
        bytes = bytes
            .checked_add(u64::try_from(row.owner.len()).ok()?)?
            .checked_add(u64::try_from(row.name.len()).ok()?)?;
    }
    bytes = bytes
        .checked_add(u64::try_from(config.app.slug.len()).ok()?)?
        .checked_add(u64::try_from(config.app.webhook.len()).ok()?)?
        .checked_add(u64::from(limits.github.string_bytes))?;
    // The retained App and its GET /app snapshot coexist. A conversion's
    // manifest remains in the page store, whose budget counts its own lists.
    let app_lists = List::<d::Permission>::worst_case(limits.github.inner)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.github.inner)?)?
        .checked_add(
            u64::from(limits.github.string_bytes).checked_mul(2)?.checked_mul(u64::from(limits.github.inner))?,
        )?;
    bytes = bytes.checked_add(app_lists.checked_mul(2)?)?;
    // Compare moves file paths from the caller-owned domain reply into at
    // most 300 descriptors, alongside one page of commit ids. Those bounded
    // vectors coexist with the response encoder; file contents are dropped.
    bytes = bytes
        .checked_add(List::<d::File>::worst_case(300)?)?
        .checked_add(List::<skein_forge::api::Commit>::worst_case(limits.page)?)?;
    // A CI page holds one run or one classic context descriptor per row;
    // the domain reply owns their texts, moved into these descriptors. Each
    // run also owns the fourteen-byte github-actions app slug.
    bytes = bytes
        .checked_add(List::<d::Run>::worst_case(limits.page)?.max(List::<d::Context>::worst_case(limits.page)?))?
        .checked_add(u64::from(limits.page).checked_mul(14)?)?;
    // Five retained rule rows and the page's cloned contexts coexist.
    bytes = bytes
        .checked_add(List::<d::Rule>::worst_case(5)?)?
        .checked_add(List::<d::Rule>::worst_case(5)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.github.inner)?)?
        .checked_add(u64::from(limits.github.inner).checked_mul(u64::from(limits.github.string_bytes))?)?;
    // The query reader retains at most 16 tokens and their decoded text
    // (bounded by its input), while its tokenizer and input intake coexist.
    bytes = bytes
        .checked_add(List::<skein_json::Token>::worst_case(16)?)?
        .checked_add(u64::from(limits.github.body_bytes))?;
    // Reviewer login decoding and the translated user ids coexist. Pending
    // repository paths are caller-owned; write_body's retained fields, copied
    // request, intake and token text fit the eight input-body copies below.
    bytes = bytes.checked_add(List::<u64>::worst_case(limits.github.inner)?)?;
    let row_bytes = u64::from(limits.github.string_bytes).checked_mul(4)?.checked_add(256)?;
    bytes
        .checked_add(row_bytes.checked_mul(u64::from(limits.page))?)?
        .checked_add(skein_json::tokenizer::worst_case(&limits.github.json)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.github.inner)?.checked_mul(2)?)?
        // The fixed JWT reader (512 tokens), its queues and signing scratch.
        .checked_add(65_536)?
        // One verified entry retains the bounded JWT bytes; its claims are inline.
        .checked_add(u64::try_from(github::jwt::JWT_BYTES).ok()?)?
        .checked_add(u64::from(limits.body).checked_mul(8)?)?
        // A merge refusal retains one response-sized rule message while the
        // JSON encoder measures and writes it; these fit eight response copies.
        .checked_add(u64::from(limits.response).checked_mul(8)?)?
        .checked_add(u64::from(limits.github.target_bytes).checked_mul(8)?)?
        .checked_add(u64::from(limits.github.string_bytes).checked_mul(8)?)
}

pub(crate) fn valid_origin(base: &[u8], cap: u32) -> bool {
    let Ok(length) = u32::try_from(base.len()) else { return false };
    if length > cap {
        return false;
    }
    let Some(host) = base.strip_prefix(b"http://") else { return false };
    if host.is_empty() {
        return false;
    }
    for byte in host {
        if *byte <= b' ' || *byte == b'/' || *byte == b'?' || *byte == b'#' || *byte == 127 {
            return false;
        }
    }
    true
}

fn local_link_chunk(out: &mut Writer, prefix: &[u8], base: &[u8]) -> Result<(), Overflow> {
    out.put(prefix)?;
    out.put(b"<")?;
    out.put(base)
}

fn repository_rows(rows: &List<Row>, string: usize) -> Result<(), Problem> {
    for (at, row) in rows.iter().enumerate() {
        if row.id == 0
            || row.owner_id == 0
            || row.owner.is_empty()
            || row.name.is_empty()
            || row.owner.len() > string
            || row.name.len() > string
            || row.owner.contains(&b'/')
            || row.name.contains(&b'/')
            || b"https://github.com/"
                .len()
                .checked_add(row.owner.len())
                .ok_or(Problem::Config)?
                .checked_add(1)
                .ok_or(Problem::Config)?
                .checked_add(row.name.len())
                .ok_or(Problem::Config)?
                > string
        {
            return Err(Problem::Config);
        }
        for prior in rows.iter().take(at) {
            if prior.id == row.id || (prior.owner == row.owner && prior.name == row.name) {
                return Err(Problem::Config);
            }
        }
    }
    Ok(())
}

fn valid_faults(faults: crate::Faults) -> bool {
    faults.reset <= 1000 && faults.drop <= 1000 && faults.server_error <= 1000
}

fn check_accounts(config: &Config, limits: &Limits, string: usize) -> Result<(), Problem> {
    if config.accounts.len() > limits.github.inner || config.person.len() > string {
        return Err(Problem::Config);
    }
    let mut person_found = false;
    for (at, account) in config.accounts.iter().enumerate() {
        if account.id == 0 || account.login.len() > string || !pages::unreserved(&account.login) {
            return Err(Problem::Config);
        }
        if account.login == config.person {
            person_found = true;
        }
        for prior in config.accounts.iter().take(at) {
            if prior.id == account.id || prior.login == account.login {
                return Err(Problem::Config);
            }
        }
    }
    if !person_found {
        return Err(Problem::Config);
    }
    Ok(())
}
