//! Forge component limits (forge-connection.md, sections 2, 3.5, 7 and 8).
use skein_forge::forgejo::limits as documents;
use skein_lib::Duration;

/// The component's limits (forge-connection.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// The forges served.
    pub forges: u32,
    /// The longest API token.
    pub token_bytes: u32,
    /// The longest App key, PKCS#1 DER (forge-connection.md, 5.2).
    pub key_bytes: u32,
    /// Forge connections in all: the client's `calls`.
    pub connections: u32,
    /// The largest response head read (HTTP's `head`).
    pub head_bytes: u32,
    /// The most fields of a response head (HTTP's own).
    pub headers: u32,
    /// The largest response body read (JSON's `length`): time and bytes, not memory.
    pub body_bytes_in: u32,
    /// The JSON nesting followed.
    pub depth: u32,
    /// The most bytes of a string's text demanded at once (JSON's own).
    pub chunk: u32,
    /// The longest name, title, context, description, link or path kept.
    pub string_bytes: u32,
    /// The longest body kept whole: at least twice the client's `op_bytes` plus the word's length and 15.
    pub body_bytes: u32,
    /// The most elements kept of a list inside one document (06's `inner`).
    pub inner: u32,
    /// The longest request target (06's `target_bytes`).
    pub target_bytes: u32,
    /// The repositories' table: at most the forge's `repositories`.
    pub repositories: u32,
    /// The users' table.
    pub users: u32,
    /// The installations kept per GitHub forge (4.4).
    pub installations: u32,
    /// Configured and remembered job log origins per GitHub forge, at least one.
    pub log_origins: u32,
    /// A call's, at most the client's `lifetime`; also each start read's.
    pub deadline: Duration,
    /// How long an idle connection is kept, below the forge's keep-alive timeout.
    pub idle: Duration,
    /// A forge's start asked again after this, doubling to `start_backoff_max`.
    pub start_backoff: Duration,
    pub start_backoff_max: Duration,
    /// An installation token is minted again this long before it expires (5.1).
    pub renew_before: Duration,
    /// Delivery-log poll interval and catch-up horizon (6.5).
    pub log_poll: Duration,
    pub log_catch_up: Duration,
    /// Maximum pages per poll and bytes per whole delivery.
    pub log_pages: u32,
    pub delivery_bytes: u32,
    /// Webhook connections, head and body bytes, request deadline and secret bytes.
    pub hooks: u32,
    pub hook_head_bytes: u32,
    pub hook_body_bytes: u32,
    pub hook_deadline: Duration,
    pub hook_secret_bytes: u32,
    /// TLS's own.
    pub tls: skein_tls::client::Limits,
}

/// What the component is given of its neighbours' limits
/// (forge-connection.md, 12, "Shared limits").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Shared {
    /// The forge client's: `calls`, `rows`, `answer_bytes`, `op_bytes`, `lifetime`.
    pub calls: u32,
    pub rows: u32,
    pub answer_bytes: u32,
    pub op_bytes: u32,
    pub lifetime: Duration,
    /// The owner's largest retained name.
    pub name_bytes: u32,
    /// The forge's `repositories`.
    pub repositories: u32,
    /// io's caps, which the stacked machines' demands must fit.
    pub io: skein_io::Limits,
}

/// Configured forge counts by dialect (forge-connection.md, 7.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Forges {
    pub forgejo: u32,
    pub github: u32,
}
impl Forges {
    /// Counts each configured kind, or returns `None` if a count overflows.
    #[must_use]
    pub fn of(settings: &Settings) -> Option<Forges> {
        let mut counts = Self { forgejo: 0, github: 0 };
        for forge in &settings.forges {
            match forge.kind {
                Kind::Forgejo => counts.forgejo = counts.forgejo.checked_add(1)?,
                Kind::GitHub => counts.github = counts.github.checked_add(1)?,
            }
        }
        Some(counts)
    }
}

/// The listings whose page size the component chooses (forge-connection.md, 3.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Listing {
    Items,
    Reviews,
    Statuses,
    Remarks,
    Branches,
    Collaborators,
    PullFiles,
    Compare,
}

/// A machine stacked on another, whose demand must fit the other's cap.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stacked {
    HttpOverTls,
    HttpOverIo,
    TlsOverIo,
    JsonOverHttp,
    ServerOverIo,
    JsonOverServer,
}

/// Why the component cannot start (protocol/configuration.md, 7:
/// "each failure naming both limits").
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Unusable {
    /// More configured job log origins than the bound.
    LogOrigins { forge: u16, origins: u32, limit: u32 },
    /// A refused origin, including its dialect, name, address or missing TLS trust.
    Origin { forge: u16, origin: skein_forge::github::redirect::Origin },
    /// A limit is 0, or the worst case passes a `u64`.
    Zero,
    /// Owner word outside its alphabet or length: terminal before begin.
    Word,
    /// Empty or unsafe plaintext name: terminal before begin; no input bytes.
    PlaintextName { forge: u16 },
    /// The secret bound must admit at least 32 bytes.
    SecretBytes { bytes: u32 },
    /// A forge's webhook secret with no listener, empty, or past its byte bound.
    Secret { forge: u16 },
    /// A webhook listener must be a loopback IP socket.
    Listener,
    /// `connections` is not the client's `calls`.
    Connections { connections: u32, calls: u32 },
    /// `deadline` passes the client's `lifetime`.
    Deadline { deadline: Duration, lifetime: Duration },
    /// `body_bytes` below the largest body the owner writes, its marker included.
    BodyBytes { body_bytes: u32, needed: u64 },
    /// `string_bytes` below the forge's `name_bytes`.
    StringBytes { string_bytes: u32, name_bytes: u32 },
    /// `repositories` above the forge's.
    Repositories { repositories: u32, forge: u32 },
    /// One of the listing's largest rows does not fit the client's `answer_bytes`.
    PageSize { listing: Listing },
    /// The documents' limits refused (06's `limits::check`).
    Documents(documents::Problem),
    /// A stacked machine's demand past the cap of the stream below.
    Stack { stacked: Stacked, demand: u32, cap: u32 },
    /// More forges than `forges`.
    Forges { forges: u32 },
    /// Two forges with one number.
    Duplicate { forge: u16 },
    /// A forge's credential of the other dialect's kind.
    Credential { forge: u16 },
    /// An API token empty, past `token_bytes`, or containing unsafe header controls.
    Token { forge: u16 },
    /// A forge's App key past `key_bytes`, or one that will not sign.
    Key { forge: u16 },
    /// `deadline` past 270 s with a GitHub forge: half a JWT's life (5.2).
    AppDeadline { deadline: Duration },
    /// `renew_before` below twice `deadline`, or past 30 minutes.
    RenewBefore { renew_before: Duration, deadline: Duration },
    /// `key_bytes` below `skein_tls::RS256_KEY_BYTES`.
    KeyBytes { key_bytes: u32 },
    /// GitHub's derived document limits refused.
    GitHubDocuments(skein_forge::github::limits::Problem),
    /// A user of 0.
    User { forge: u16 },
    /// Plaintext to an address not on loopback, or an in-process name.
    Address { forge: u16 },
    /// An empty or unsafe agent, or a request head past HTTP's `request`.
    Request { forge: u16 },
}

use crate::{
    Access, Value,
    settings::{Kind, Settings, Transport},
};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_forge::api;
use skein_json::{tokenizer, writer};
const APP_DEADLINE: Duration = Duration::from_secs(270);
const MAX_RENEW_BEFORE: Duration = Duration::from_secs(1800);

/// Checks neighbours' agreements, stacked demands, then every configured forge.
pub fn check(l: &Limits, s: &Shared, settings: &Settings) -> Result<(), Unusable> {
    nonzero(l)?;
    if l.hook_secret_bytes < 32 {
        return Err(Unusable::SecretBytes { bytes: l.hook_secret_bytes });
    }
    let server = hook_http(l);
    skein_http::server::worst_case(&server).ok_or(Unusable::Zero)?;
    stack(Stacked::ServerOverIo, skein_http::server::largest_read(&server), s.io.intake)?;
    stack(Stacked::ServerOverIo, skein_http::server::largest_room(&server), s.io.output)?;
    stack(Stacked::JsonOverServer, tokenizer::largest_demand(&hook_documents(l).json), server.read)?;
    if l.connections != s.calls {
        return Err(Unusable::Connections { connections: l.connections, calls: s.calls });
    }
    if l.deadline > s.lifetime {
        return Err(Unusable::Deadline { deadline: l.deadline, lifetime: s.lifetime });
    }
    check_github(l, s, settings)?;
    if !skein_forge::marker::valid_word(&settings.word) {
        return Err(Unusable::Word);
    }
    let needed = u64::from(skein_forge::marker::length(&settings.word, s.op_bytes).ok_or(Unusable::Zero)?);
    if u64::from(l.body_bytes) < needed {
        return Err(Unusable::BodyBytes { body_bytes: l.body_bytes, needed });
    }
    if l.string_bytes < s.name_bytes {
        return Err(Unusable::StringBytes { string_bytes: l.string_bytes, name_bytes: s.name_bytes });
    }
    if l.repositories > s.repositories {
        return Err(Unusable::Repositories { repositories: l.repositories, forge: s.repositories });
    }
    if let Err(problem) = documents::check(&documents(l)) {
        return Err(Unusable::Documents(problem));
    }
    for listing in [
        Listing::Items,
        Listing::Reviews,
        Listing::Statuses,
        Listing::Remarks,
        Listing::Branches,
        Listing::Collaborators,
        Listing::PullFiles,
        Listing::Compare,
    ] {
        if page_size(listing, u32::MAX, l, s) == 0 {
            return Err(Unusable::PageSize { listing });
        }
    }
    for tls in [true, false] {
        let h = http(l, tls, &s.io);
        let (stacked, read, send) = if tls {
            (Stacked::HttpOverTls, l.tls.read, l.tls.send)
        } else {
            (Stacked::HttpOverIo, s.io.intake, s.io.output)
        };
        stack(stacked, skein_http::client::largest_read(&h), read)?;
        stack(stacked, skein_http::client::largest_room(&h), send)?;
    }
    stack(Stacked::TlsOverIo, skein_tls::client::LARGEST_READ, s.io.intake)?;
    stack(Stacked::TlsOverIo, skein_tls::client::largest_room(&l.tls), s.io.output)?;
    stack(Stacked::JsonOverHttp, tokenizer::largest_demand(&documents(l).json), http(l, false, &s.io).read)?;
    check_settings(l, s, settings)?;
    worst_case(
        l,
        s,
        Forges::of(settings).ok_or(Unusable::Zero)?,
        crate::settings::owned_bytes(settings).ok_or(Unusable::Zero)?,
    )
    .ok_or(Unusable::Zero)?;
    Ok(())
}
fn check_github(l: &Limits, s: &Shared, settings: &Settings) -> Result<(), Unusable> {
    let mut github = false;
    for forge in &settings.forges {
        if forge.kind == Kind::GitHub {
            github = true;
        }
    }
    if github {
        if let Err(problem) = skein_forge::github::limits::check(&github_documents(l)) {
            return Err(Unusable::GitHubDocuments(problem));
        }
        if let Err(problem) = skein_forge::github::limits::check(&github_mint_documents(l)) {
            return Err(Unusable::GitHubDocuments(problem));
        }
        installation_bounds(l, s).ok_or(Unusable::Zero)?;
        if l.deadline > APP_DEADLINE {
            return Err(Unusable::AppDeadline { deadline: l.deadline });
        }
        if l.renew_before < l.deadline.saturating_mul(2) || l.renew_before > MAX_RENEW_BEFORE {
            return Err(Unusable::RenewBefore { renew_before: l.renew_before, deadline: l.deadline });
        }
        if usize::try_from(l.key_bytes).expect("u32 fits") < skein_tls::RS256_KEY_BYTES {
            return Err(Unusable::KeyBytes { key_bytes: l.key_bytes });
        }
    }
    Ok(())
}
fn nonzero(l: &Limits) -> Result<(), Unusable> {
    for value in [
        l.forges,
        l.token_bytes,
        l.key_bytes,
        l.connections,
        l.head_bytes,
        l.headers,
        l.body_bytes_in,
        l.depth,
        l.chunk,
        l.string_bytes,
        l.body_bytes,
        l.inner,
        l.target_bytes,
        l.repositories,
        l.users,
        l.installations,
        l.log_pages,
        l.delivery_bytes,
        l.hooks,
        l.hook_head_bytes,
        l.hook_body_bytes,
        l.hook_secret_bytes,
        l.tls.read,
        l.tls.send,
        l.tls.records,
    ] {
        if value == 0 {
            return Err(Unusable::Zero);
        }
    }
    for value in [
        l.deadline,
        l.idle,
        l.start_backoff,
        l.start_backoff_max,
        l.renew_before,
        l.log_poll,
        l.log_catch_up,
        l.hook_deadline,
    ] {
        if value.as_nanos() == 0 {
            return Err(Unusable::Zero);
        }
    }
    Ok(())
}
fn check_settings(l: &Limits, s: &Shared, settings: &Settings) -> Result<(), Unusable> {
    if l.log_origins == 0 {
        return Err(Unusable::Zero);
    }
    if settings.forges.len() > l.forges {
        return Err(Unusable::Forges { forges: l.forges });
    }
    for (at, forge) in settings.forges.as_slice().iter().enumerate() {
        for before in settings.forges.as_slice().get(..at).expect("prior forge positions") {
            if before.number == forge.number {
                return Err(Unusable::Duplicate { forge: forge.number });
            }
        }
        if let Some(a) = &forge.access
            && let Err(value) = access(l, forge.kind, settings.webhooks.is_some(), a)
        {
            return Err(match value {
                Value::Credential => Unusable::Credential { forge: forge.number },
                Value::Key => Unusable::Key { forge: forge.number },
                Value::Token => Unusable::Token { forge: forge.number },
                Value::User => Unusable::User { forge: forge.number },
                Value::Secret => Unusable::Secret { forge: forge.number },
            });
        }
        let tls = match &forge.transport {
            Transport::Plaintext { name } => {
                if let Some(name) = name
                    && !plaintext_name(name)
                {
                    return Err(Unusable::PlaintextName { forge: forge.number });
                }
                false
            }
            Transport::Tls { .. } => true,
        };
        match forge.address {
            skein_io::Addr::Ip(ip) => {
                if !tls && !ip.ip().is_loopback() {
                    return Err(Unusable::Address { forge: forge.number });
                }
            }
            skein_io::Addr::InProcess(_) => return Err(Unusable::Address { forge: forge.number }),
        }
        check_origins(l, s, settings, forge, tls)?;
        // Longest method, target, seven fields and final empty line. Decimal lengths
        // use u64's longest spelling; headers here reserve their complete framing.
        let head = u64::from(l.target_bytes)
            .checked_add(
                u64::try_from(crate::settings::host(forge).ok_or(Unusable::Zero)?.len()).ok().ok_or(Unusable::Zero)?,
            )
            .ok_or(Unusable::Zero)?;
        let credential = match forge.kind {
            Kind::Forgejo => u64::from(l.token_bytes),
            Kind::GitHub => u64::from(l.token_bytes)
                .max(u64::try_from(skein_forge::github::jwt::JWT_BYTES).expect("JWT bound fits")),
        };
        let head = head
            .checked_add(credential)
            .ok_or(Unusable::Zero)?
            .checked_add(u64::try_from(settings.user_agent.len()).ok().ok_or(Unusable::Zero)?)
            .ok_or(Unusable::Zero)?
            .checked_add(192)
            .ok_or(Unusable::Zero)?;
        if !header_value(&settings.user_agent) || head > u64::from(http(l, tls, &s.io).request) {
            return Err(Unusable::Request { forge: forge.number });
        }
    }
    if let Some(hooks) = &settings.webhooks {
        match hooks.listener {
            skein_io::Addr::Ip(ip) if ip.ip().is_loopback() => {}
            skein_io::Addr::Ip(_) | skein_io::Addr::InProcess(_) => return Err(Unusable::Listener),
        }
    }
    Ok(())
}
fn check_origins(
    l: &Limits,
    s: &Shared,
    settings: &Settings,
    forge: &crate::settings::Forge,
    tls: bool,
) -> Result<(), Unusable> {
    if forge.log_origins.len() > l.log_origins {
        return Err(Unusable::LogOrigins {
            forge: forge.number,
            origins: forge.log_origins.len(),
            limit: l.log_origins,
        });
    }
    for configured in &forge.log_origins {
        let origin = &configured.origin;
        let address_ok = match configured.address {
            skein_io::Addr::Ip(ip) => origin.tls || ip.ip().is_loopback(),
            skein_io::Addr::InProcess(_) => false,
        };
        if forge.kind != Kind::GitHub
            || crate::settings::log_name(origin).is_none()
            || !address_ok
            || (origin.tls && !tls)
        {
            return Err(Unusable::Origin { forge: forge.number, origin: origin.clone() });
        }
        let head = u64::from(l.target_bytes)
            .checked_add(u64::try_from(crate::settings::log_host(configured).len()).ok().ok_or(Unusable::Zero)?)
            .ok_or(Unusable::Zero)?
            .checked_add(u64::try_from(settings.user_agent.len()).ok().ok_or(Unusable::Zero)?)
            .ok_or(Unusable::Zero)?
            .checked_add(192)
            .ok_or(Unusable::Zero)?;
        if head > u64::from(http(l, origin.tls, &s.io).request) {
            return Err(Unusable::Origin { forge: forge.number, origin: origin.clone() });
        }
    }
    Ok(())
}
/// A connect's check of one access (forge-connection.md, 3.5), before any io.
pub(crate) fn access(l: &Limits, kind: Kind, listener: bool, access: &Access) -> Result<(), Value> {
    use skein_forge::credential::Credential;
    match (&access.credential, kind) {
        (Credential::Token(token), Kind::Forgejo) => {
            if !header_value(token.expose()) || token.len() > usize::try_from(l.token_bytes).expect("u32 fits") {
                return Err(Value::Token);
            }
        }
        (Credential::App { key, .. }, Kind::GitHub) => {
            if key.len() > usize::try_from(l.key_bytes).expect("u32 fits")
                || skein_tls::sign_rs256(key.expose(), b"skein").is_none()
            {
                return Err(Value::Key);
            }
        }
        (Credential::Token(_), Kind::GitHub) | (Credential::App { .. }, Kind::Forgejo) => {
            return Err(Value::Credential);
        }
    }
    if access.user == 0 {
        return Err(Value::User);
    }
    if let Some(secret) = &access.webhook_secret
        && (kind == Kind::GitHub
            || !listener
            || secret.is_empty()
            || secret.len() > usize::try_from(l.hook_secret_bytes).expect("u32 fits"))
    {
        return Err(Value::Secret);
    }
    Ok(())
}
pub(crate) fn stack(stacked: Stacked, demand: u32, cap: u32) -> Result<(), Unusable> {
    if demand > cap { Err(Unusable::Stack { stacked, demand, cap }) } else { Ok(()) }
}
/// A listing's largest fixed row and owned payloads, measured as the client measures them.
#[must_use]
pub fn page_size(listing: Listing, cap: u32, l: &Limits, s: &Shared) -> u32 {
    let Some(row) = largest_row(listing, l) else {
        return 0;
    };
    let room = u64::from(s.answer_bytes).checked_div(row).unwrap_or(0);
    s.rows.min(cap).min(u32::try_from(room).unwrap_or(u32::MAX))
}
fn largest_row(listing: Listing, l: &Limits) -> Option<u64> {
    use skein_forge::measure;
    let string = u64::from(l.string_bytes);
    let body = u64::from(l.body_bytes);
    let key = body / 2; // Every valid marker key is within the retained body's framing.
    let (fixed, payload) = match listing {
        Listing::Items => (
            measure::SUMMARY_BYTES,
            body.checked_add(key)?.checked_add(string)?.checked_add(
                u64::from(l.inner).checked_mul(string.checked_add(u64::try_from(measure::STRING_BYTES).ok()?)?)?,
            )?,
        ),
        Listing::Reviews => (measure::REVIEW_BYTES, body.checked_add(key)?),
        Listing::Statuses => (measure::STATUS_BYTES, string.checked_mul(3)?),
        Listing::Remarks => (measure::REMARK_BYTES, body.checked_add(string)?),
        Listing::Branches => (measure::STRING_BYTES, string),
        Listing::Collaborators => (measure::COLLABORATOR_BYTES, 0),
        Listing::Compare => (measure::COMMIT_BYTES, 0),
        Listing::PullFiles => (measure::FILE_BYTES, string.checked_add(body.checked_mul(2)?)?),
    };
    u64::try_from(fixed).ok()?.checked_add(payload)
}
/// Derived Forgejo document limits, with body strings capped independently of incoming bytes.
#[must_use]
pub fn documents(l: &Limits) -> documents::Limits {
    documents::Limits {
        json: tokenizer::Limits {
            depth: l.depth,
            string: l.body_bytes.max(l.string_bytes),
            number: 32,
            chunk: l.chunk,
            length: l.body_bytes_in,
        },
        writer: writer::Limits { depth: l.depth, length: l.body_bytes_in },
        string_bytes: l.string_bytes,
        body_bytes: l.body_bytes,
        inner: l.inner,
        target_bytes: l.target_bytes,
    }
}
/// GitHub documents use the same component bounds in their dialect's vocabulary.
pub(crate) fn github_documents(l: &Limits) -> skein_forge::github::limits::Limits {
    let docs = documents(l);
    skein_forge::github::limits::Limits {
        json: docs.json,
        writer: docs.writer,
        string_bytes: docs.string_bytes,
        body_bytes: docs.body_bytes,
        inner: docs.inner,
        target_bytes: docs.target_bytes,
    }
}
// RFC 3339's YYYY-MM-DDTHH:MM:SSZ must fit even when token_bytes is smaller.
const MINTED_TIME_BYTES: u32 = 20;
pub(crate) fn github_mint_documents(l: &Limits) -> skein_forge::github::limits::Limits {
    let mut docs = github_documents(l);
    docs.string_bytes = l.token_bytes.max(MINTED_TIME_BYTES);
    docs.json.string = docs.json.string.max(docs.string_bytes);
    docs
}
/// The body stream's demands are chunked; heads use the stream's complete send cap.
#[must_use]
pub fn http(l: &Limits, tls: bool, io: &skein_io::Limits) -> skein_http::client::Limits {
    skein_http::client::Limits {
        request: if tls { l.tls.send } else { io.output },
        head: l.head_bytes,
        headers: l.headers,
        read: l.chunk.max(5),
        send: if tls { l.tls.send } else { io.output },
    }
}
/// The webhook server reads bounded pieces and answers with no response body.
#[must_use]
pub fn hook_http(l: &Limits) -> skein_http::server::Limits {
    skein_http::server::Limits {
        head: l.hook_head_bytes,
        headers: l.headers,
        body: u64::from(l.hook_body_bytes),
        read: l.chunk.max(5),
        response: 256,
        send: 1,
    }
}
/// Webhook documents have their own incoming body bound.
#[must_use]
pub fn hook_documents(l: &Limits) -> documents::Limits {
    let mut docs = documents(l);
    docs.json.length = l.hook_body_bytes;
    docs
}
pub(crate) fn decoder_heap(l: &Limits, forges: Forges) -> Option<u64> {
    let forgejo = if forges.forgejo > 0 { size_of::<skein_forge::forgejo::client::Decoder>() } else { 0 };
    let github = if forges.github > 0 { size_of::<skein_forge::github::decode::Decoder>() } else { 0 };
    let fixed_decoder = forgejo.max(github);
    // Conservatively covers the boxed decoder state and both builder/delivered
    // rows: 32 scalar strings, two bodies, three lists of texts/users, and the
    // walker's depth stack. Both dialects fit these payload envelopes;
    // each boxed decoder also holds its fixed builder state.
    let inner = u64::from(l.string_bytes)
        .checked_mul(3)?
        .checked_add(u64::try_from(size_of::<skein_forge::forgejo::document::User>()).ok()?.checked_mul(2)?)?
        .checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_mul(2)?)?;
    // An App's builder and delivered document each retain a permission and
    // event list: two names, a Permission and a text box per entry, twice.
    let app_inner = if forges.github > 0 {
        u64::from(l.string_bytes)
            .checked_mul(4)?
            .checked_add(u64::try_from(size_of::<skein_forge::github::document::Permission>()).ok()?.checked_mul(2)?)?
            .checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_mul(2)?)?
    } else {
        0
    };
    let inner = if forges.forgejo > 0 || forges.github > 0 { inner.max(app_inner) } else { 0 };
    let string =
        if forges.github > 0 { l.string_bytes.max(github_mint_documents(l).string_bytes) } else { l.string_bytes };
    let bytes = u64::from(string)
        .checked_mul(32)?
        .checked_add(u64::from(l.inner).checked_mul(inner)?)?
        .checked_add(u64::from(l.body_bytes).checked_mul(2)?)?
        .checked_add(u64::from(l.depth).checked_mul(128)?)?
        .checked_add(4096)?
        .checked_add(u64::try_from(fixed_decoder).ok()?)?
        // The shared GitHub payload decoder moves the path into its logged document.
        .checked_add(if forges.github > 0 {
            u64::from(skein_forge::github::delivery::OWNER_BYTES)
                .checked_add(u64::from(skein_forge::github::delivery::NAME_BYTES))?
        } else {
            0
        })?;
    Some(bytes)
}
/// A full log page includes bounded event/action text and both header values.
pub(crate) fn log_page_bytes(l: &Limits) -> Option<u32> {
    skein_forge::github::pin::PAGE_CAP
        .min(l.inner)
        .checked_mul(
            u32::try_from(size_of::<skein_forge::github::log::Row>())
                .ok()?
                .checked_add(l.string_bytes.checked_mul(2)?)?,
        )?
        .checked_add(u32::try_from(api::VALIDATOR_BYTES).ok()?.checked_mul(2)?)
}
/// Puller-owned rows and the whole-delivery decoder can coexist with a held hint.
pub(crate) fn puller_heap(l: &Limits) -> Option<u64> {
    use skein_forge::github;
    u64::from(log_page_bytes(l)?)
        .checked_mul(2)? // Plan/page handoff and header copies.
        .checked_add(u64::try_from(api::VALIDATOR_BYTES).ok()?.checked_mul(4)?)?
        .checked_add(u64::try_from(size_of::<github::plans::start::LogPage>()).ok()?)?
        .checked_add(u64::try_from(size_of::<github::plans::start::LogDelivery>()).ok()?)?
        .checked_add(u64::try_from(size_of::<github::delivery::Decoder>()).ok()?)?
        .checked_add(decoder_heap(l, Forges { forgejo: 0, github: 1 })?)?
        // The selected row's event is owned by the request and its decoder.
        .checked_add(u64::from(l.string_bytes).checked_mul(2)?)?
        .checked_add(u64::try_from(size_of::<api::Hint>()).ok()?)?
        .checked_add(u64::from(l.string_bytes).checked_add(u64::from(l.body_bytes))?)
}
/// A complete page is independent of both the retained table and ordinary calls.
pub(crate) fn installation_bounds(l: &Limits, s: &Shared) -> Option<skein_forge::github::plans::plan::Bounds> {
    let cap = skein_forge::github::pin::PAGE_CAP;
    Some(skein_forge::github::plans::plan::Bounds {
        inner: l.inner,
        page_size: cap,
        rows: cap,
        answer_bytes: cap
            .checked_mul(s.name_bytes.checked_add(u32::try_from(skein_forge::measure::STRING_BYTES).ok()?)?)?,
        body_bytes: l.body_bytes,
    })
}
// Both metadata tables, live credentials, accepted bookkeeping and page/event handoffs.
// Per-scope clocks, pending work and both revisions are in Row's container cost.
// Retry candidates, held Contents and old/new Full coexist with mint/authorization handoffs.
fn installation_heap(l: &Limits, s: &Shared) -> Option<u64> {
    use crate::installations::{Accepted, Metadata, Row};
    use skein_lib::{List, Map, Set};
    let cap = l.installations;
    let page = skein_forge::github::pin::PAGE_CAP;
    Map::<u64, Row>::worst_case(cap)?
        .checked_add(Map::<u64, Metadata>::worst_case(cap)?.checked_mul(2)?)?
        .checked_add(Map::<u64, Accepted>::worst_case(cap)?.checked_mul(2)?)?
        .checked_add(Map::<u64, Option<crate::Account>>::worst_case(cap)?)?
        .checked_add(Set::<u64>::worst_case(cap)?.checked_mul(3)?)?
        .checked_add(List::<u64>::worst_case(cap)?.checked_mul(2)?)?
        .checked_add(List::<skein_forge::github::document::Installation>::worst_case(page)?.checked_mul(2)?)?
        .checked_add(u64::from(page).checked_mul(u64::from(s.name_bytes))?.checked_mul(2)?)?
        .checked_add(
            u64::from(cap)
                .checked_mul(u64::from(s.name_bytes).checked_mul(6)?.checked_add(u64::from(l.token_bytes))?)?,
        )?
        .checked_add(u64::from(l.token_bytes).checked_mul(7)?)?
        .checked_add(u64::try_from(size_of::<crate::Event>()).ok()?)?
        .checked_add(u64::from(s.name_bytes).checked_mul(3)?)
}
fn log_origin_heap(l: &Limits) -> Option<u64> {
    skein_lib::List::<crate::settings::LogOrigin>::worst_case(l.log_origins)?
        .checked_add(skein_lib::Queue::<skein_forge::github::redirect::Origin>::worst_case(l.log_origins)?)?
        .checked_add(u64::from(l.log_origins).checked_mul(253_u64.checked_add(u64::from(l.target_bytes))?)?)
}
fn github_forge_heap(l: &Limits, s: &Shared, forges: u32) -> Option<u64> {
    if forges == 0 {
        return Some(0);
    }
    installation_heap(l, s)?
        .checked_add(log_origin_heap(l)?)?
        .checked_add(puller_heap(l)?)?
        .checked_add(u64::try_from(size_of::<crate::installations::GitHub>()).ok()?)?
        .checked_add(u64::from(l.key_bytes))?
        .checked_add(u64::try_from(skein_forge::github::jwt::JWT_BYTES).ok()?)?
        .checked_mul(u64::from(forges))
}
/// Conservative complete heap, pricing TLS on every connection because this interface
/// knows each kind's count, not their transports. Raw growth includes its doubling overlap.
#[must_use]
pub fn worst_case(l: &Limits, s: &Shared, forges: Forges, owned_bytes: u64) -> Option<u64> {
    use crate::{
        boundary::Answered,
        component::{Held, Waiting},
        settings::Forge,
        start::Start,
    };
    use skein_forge::forgejo::plans::tables::{RepositoryEntry, UserEntry};
    use skein_lib::{Id, Intake, List, Map, Queue, Slab, Token};
    // Forgejo start traffic uses the call pool; only GitHub reserves an extra connection.
    let connections = l.connections.checked_add(forges.github)?;
    let count = forges.forgejo.checked_add(forges.github)?;
    let docs = documents(l);
    let raw = u64::from(s.answer_bytes).checked_mul(2)?;
    let github = github_forge_heap(l, s, forges.github)?;
    let decoder = decoder_heap(l, forges)?;
    let json = if forges.github > 0 { github_mint_documents(l).json } else { docs.json };
    let exchange = if forges.github > 0 {
        u64::try_from(size_of::<skein_forge::github::plans::plan::Exchange>()).ok()?
    } else {
        0
    };
    let socket = Intake::worst_case(s.io.intake)?
        .checked_add(u64::from(s.io.receive))?
        .checked_add(u64::from(s.io.output))?
        .checked_add(Queue::<Box<[u8]>>::worst_case(s.io.sends)?)?;
    let connection = skein_http::client::worst_case(&http(l, true, &s.io))?
        .max(skein_http::client::worst_case(&http(l, false, &s.io))?)
        .checked_add(skein_tls::client::worst_case(&l.tls)?)?
        .checked_add(tokenizer::worst_case(&tokenizer::Limits {
            string: json.string.max(docs.json.string),
            ..docs.json
        })?)?
        .checked_add(decoder)?
        .checked_add(raw)?
        // A GitHub connection retains one boxed request, retention and comparison.
        .checked_add(exchange)?
        // The local JSON intake and the delivery copied from it can coexist.
        // HTTP already counts the incoming refill's box.
        .checked_add(Intake::worst_case(l.chunk.max(5))?)?
        .checked_add(u64::from(l.chunk.max(5)))?
        .checked_add(crate::connection::routing_bytes()?)?
        .checked_add(u64::from(l.tls.send.max(s.io.output)))?
        .checked_add(u64::from(l.body_bytes).checked_mul(6)?.checked_add(1024)?)?
        .checked_add(socket)?;
    let hooks = hook_heap(l, decoder, socket)?;
    // Common call storage: record and indexes, three operation copies, and the
    // held answer below. A slot holds one dialect's plans and payloads at a time:
    // Forgejo's boxed read/write plans, staging lists and decoded rows, or GitHub's
    // boxed plans, discovery table/candidates, files state and retained answer handoffs.
    let forgejo_call = if forges.forgejo > 0 { forgejo_call_heap(l, s)? } else { 0 };
    let github_call = if forges.github > 0 { github_call_heap(l, s)? } else { 0 };
    let calls = Slab::<Waiting>::worst_case(s.calls)?
        .checked_add(Map::<Token, Token>::worst_case(connections)?)?
        .checked_add(List::<(Token, Id<Waiting>)>::worst_case(s.calls)?)?
        .checked_add(Map::<Token, Id<Waiting>>::worst_case(s.calls)?)?
        .checked_add(
            u64::from(s.calls)
                .checked_mul(u64::from(s.op_bytes).checked_mul(3)?.checked_add(forgejo_call.max(github_call))?)?,
        )?;
    let held = Map::<Token, Held>::worst_case(s.calls)?
        .checked_add(u64::from(s.calls).checked_mul(u64::from(s.answer_bytes))?)?;
    let repositories = if forges.forgejo > 0 {
        Map::<api::Repository, RepositoryEntry>::worst_case(l.repositories)?
            .checked_add(u64::from(l.repositories).checked_mul(u64::from(l.string_bytes).checked_mul(3)?)?)?
    } else {
        0
    };
    let users = if forges.forgejo > 0 {
        Map::<(u16, u64), UserEntry>::worst_case(l.users)?
            .checked_add(u64::from(l.users).checked_mul(u64::from(l.string_bytes))?)?
    } else {
        0
    };
    let github_tables = if forges.github > 0 { github_tables_heap(l, l.repositories, l.users)? } else { 0 };
    let configured =
        List::<Forge>::worst_case(l.forges)?.checked_add(List::<Start>::worst_case(l.forges)?)?.checked_add(
            u64::from(count).checked_mul(
                u64::from(l.token_bytes)
                    .checked_add(u64::from(l.hook_secret_bytes))?
                    .checked_add(u64::from(l.tls.send.max(s.io.output)))?
                    .checked_add(skein_tls::client::worst_case(&l.tls)?)?,
            )?,
        )?;
    connection
        .checked_mul(u64::from(connections))?
        .checked_add(List::<Option<Box<crate::installations::GitHub>>>::worst_case(l.forges)?)?
        .checked_add(github)?
        .checked_add(Map::<Token, Id<crate::connection::Connection>>::worst_case(connections)?)?
        .checked_add(Slab::<crate::connection::Connection>::worst_case(connections)?)?
        .checked_add(List::<Option<Token>>::worst_case(l.forges)?)?
        .checked_add(List::<Option<crate::component::OwnPlan>>::worst_case(l.forges)?)?
        .checked_add(hooks)?
        .checked_add(calls)?
        .checked_add(held)?
        .checked_add(github_tables)?
        .checked_add(repositories)?
        .checked_add(users)?
        .checked_add(configured)?
        .checked_add(u64::try_from(size_of::<Box<[u8]>>()).ok()?.checked_mul(2)?)?
        .checked_add(owned_bytes)?
        .checked_add(16)?
        .checked_add(u64::try_from(size_of::<Answered>()).ok()?)
}

// Forgejo plan boxes, operation staging, kept answer and decoded file/commit/user rows.
fn forgejo_call_heap(l: &Limits, s: &Shared) -> Option<u64> {
    use skein_lib::List;
    u64::try_from(size_of::<skein_forge::forgejo::plans::reads::Plan>())
        .ok()?
        .checked_add(u64::try_from(size_of::<skein_forge::forgejo::plans::writes::Plan>()).ok()?)?
        .checked_add(List::<u64>::worst_case(s.op_bytes.checked_div(8)?)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(s.op_bytes.checked_div(8)?.checked_add(l.inner)?)?)?
        .checked_add(
            u64::from(s.op_bytes.checked_div(8)?.checked_add(l.inner)?).checked_mul(u64::from(l.string_bytes))?,
        )?
        .checked_add(u64::from(s.answer_bytes))?
        .checked_add(List::<skein_forge::forgejo::document::ChangedFile>::worst_case(s.rows)?)?
        .checked_add(u64::from(s.rows).checked_mul(u64::from(l.string_bytes).checked_mul(2)?)?)?
        .checked_add(List::<api::File>::worst_case(s.rows)?)?
        .checked_add(List::<api::Commit>::worst_case(s.rows)?)?
        .checked_add(u64::from(l.string_bytes).checked_mul(4)?)?
        .checked_add(u64::from(l.string_bytes).checked_mul(u64::from(l.inner).checked_add(5)?)?)?
        .checked_add(
            u64::from(l.inner).checked_mul(u64::try_from(size_of::<skein_forge::forgejo::document::User>()).ok()?)?,
        )
}

// Both boxed plans, discovery tables/candidates and three answer envelopes:
// retained payloads (including current file bodies, Item and Pull), terminal
// conversion overlap, and handoffs. Common call storage pays operation copies.
// Read containers reserve min(rows, answer_bytes / measure width), irrespective
// of page_size. Only one read runs: take the largest reserved storage, including
// Branches, Items, Item comments, Statuses, Checks + pending jobs, Collaborators,
// Remarks and Reviews. Their texts fit the retained answer envelope:
// Branches' names; Items/Item's label boxes/text, key, title and body;
// comments/reviews' key/body; Pull's reviewers/head/base/dequeued;
// Statuses/Checks' context/description/url; Remarks' path/body; Settings'
// default_branch. Collaborator logins move into the shared users table.
// Files additionally retain descriptors beside File/Commit storage. Paths move
// from descriptors through current into files; previous paths move into previous.
// Price these names once across those owners, capped together by descriptor_size.
// FileStatus and current/previous envelopes live inside the boxed read plan;
// commits are fixed arrays, and raw bodies fit the retained answer envelope.
// Reviewers holds two id lists, two login lists and their boxed envelope; login
// payloads and request copies fit two answer envelopes. Merge's enqueue id is
// string-bounded. Row copies, request owner/name, Permission login/request copy,
// marked write bodies and refusal cursor/etag/location coexist with payloads.
// Route and Discovery's scalar cursor/revisions/errors are inline in Waiting;
// its candidates and one-row tables are priced beside the plan boxes.
pub(crate) fn github_call_heap(l: &Limits, s: &Shared) -> Option<u64> {
    use skein_forge::{github, measure};
    use skein_lib::{List, Queue};
    let files = github_read_capacity(s, measure::FILE_BYTES)?;
    let commits = github_read_capacity(s, measure::COMMIT_BYTES)?;
    let names = u64::from(files).checked_mul(u64::from(l.string_bytes))?.checked_mul(2)?;
    let mut read = Queue::<github::document::File>::worst_case(files)?
        .checked_add(List::<api::File>::worst_case(files)?)?
        .checked_add(List::<api::Commit>::worst_case(commits)?)?
        .checked_add(names.min(u64::from(s.answer_bytes)))?;
    // STATUS_BYTES covers Checks' status and pending-job slot together.
    let statuses = github_read_capacity(s, measure::STATUS_BYTES)?;
    for storage in [
        List::<Box<[u8]>>::worst_case(github_read_capacity(s, measure::STRING_BYTES)?)?,
        List::<api::Summary>::worst_case(github_read_capacity(s, measure::SUMMARY_BYTES)?)?,
        List::<api::Comment>::worst_case(github_read_capacity(s, measure::COMMENT_BYTES)?)?,
        List::<api::Status>::worst_case(statuses)?.checked_add(List::<(u32, u64)>::worst_case(statuses)?)?,
        List::<api::Collaborator>::worst_case(github_read_capacity(s, measure::COLLABORATOR_BYTES)?)?,
        List::<api::Remark>::worst_case(github_read_capacity(s, measure::REMARK_BYTES)?)?,
        List::<api::Review>::worst_case(github_read_capacity(s, measure::REVIEW_BYTES)?)?,
    ] {
        read = read.max(storage);
    }
    let desired = s.rows.min(s.op_bytes.checked_div(8)?).min(s.answer_bytes.checked_div(8)?);
    let current = s.rows.min(l.inner).min(s.answer_bytes.checked_div(8)?);
    let logins = s.rows.min(s.answer_bytes.checked_div(8)?);
    // Four List envelopes and two words cover bytes, added and their padding.
    let reviewers = u64::try_from(size_of::<(List<u64>, List<Box<[u8]>>, u64)>())
        .ok()?
        .checked_mul(2)?
        .checked_add(List::<u64>::worst_case(desired)?)?
        .checked_add(List::<u64>::worst_case(current)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(logins)?)?;
    u64::try_from(size_of::<github::plans::reads::Plan>())
        .ok()?
        .checked_mul(2)?
        .checked_add(u64::try_from(size_of::<github::plans::writes::Plan>()).ok()?)?
        .checked_add(github_tables_heap(l, 1, 1)?)?
        .checked_add(List::<crate::installations::Selected>::worst_case(l.installations)?)?
        .checked_add(read)?
        // Protection keeps distinct ruleset ids beside its context union.
        .checked_add(List::<u64>::worst_case(l.inner)?)?
        .checked_add(reviewers)?
        // Two repository rows (probe/original), request owner/name and three
        // strings for Permission's login/request, a raw path or Merge's id.
        // Each decoded field is string_bytes-bounded.
        .checked_add(u64::from(l.string_bytes).checked_mul(11)?)?
        .checked_add(u64::from(l.body_bytes))?
        .checked_add(256_u64.checked_add(u64::from(l.target_bytes))?)?
        .checked_add(u64::from(s.answer_bytes).checked_mul(3)?)?
        .checked_add(github_write_heap(l, s)?)
}
// The retained addition/removal login arrays share the combined difference
// bound already priced in reviewers. The pending mutation owns one more array;
// its cloned text shares the answer envelopes, so only its slots are added.
// Decoded write prerequisites coexist with that state, outside read containers:
// Pull's reviewer ids/head/base, GraphQL's node/dequeue reason, and Enqueue's
// error type/message. The request's enqueue id stays in the eleven-string term.
fn github_write_heap(l: &Limits, s: &Shared) -> Option<u64> {
    use skein_lib::List;
    let logins = s.rows.min(s.answer_bytes.checked_div(8)?);
    List::<Box<[u8]>>::worst_case(logins)?
        .checked_add(List::<u64>::worst_case(l.inner)?)?
        .checked_add(u64::from(l.string_bytes).checked_mul(6)?)
}
// reads::size::capacity: the backing allocation precedes page_size admission.
fn github_read_capacity(s: &Shared, width: usize) -> Option<u32> {
    Some(s.rows.min(s.answer_bytes.checked_div(u32::try_from(width).ok()?)?))
}
// Tuple envelopes price each dialect-private entry's row and LRU counter.
// Discovery's one-row staging coexists with both full public tables.
fn github_tables_heap(l: &Limits, repositories: u32, users: u32) -> Option<u64> {
    use skein_lib::Map;
    Map::<api::Repository, (skein_forge::github::plans::tables::Repository, u64)>::worst_case(repositories)?
        .checked_add(u64::from(repositories).checked_mul(u64::from(l.string_bytes).checked_mul(3)?)?)?
        .checked_add(Map::<(u16, u64), (Box<[u8]>, u64)>::worst_case(users)?)?
        .checked_add(u64::from(users).checked_mul(u64::from(l.string_bytes))?)
}

fn hook_heap(l: &Limits, decoder: u64, socket: u64) -> Option<u64> {
    use skein_lib::Queue;
    skein_http::server::worst_case(&hook_http(l))?
        .checked_add(tokenizer::worst_case(&hook_documents(l).json)?)?
        .checked_add(decoder)?
        .checked_add(socket)?
        .checked_add(u64::try_from(size_of::<skein_lib::sha256::Hmac>()).ok()?)?
        .checked_add(u64::try_from(size_of::<api::Hint>()).ok()?)?
        .checked_add(u64::from(l.string_bytes).checked_add(u64::from(l.body_bytes))?)?
        .checked_add(Queue::<skein_http::server::Event>::worst_case(4)?)?
        .checked_add(Queue::<skein_lib::stream::Down>::worst_case(4)?)?
        .checked_add(Queue::<tokenizer::Event>::worst_case(1)?)?
        .checked_add(Queue::<skein_lib::stream::Down>::worst_case(1)?)?
        .checked_add(u64::from(l.hook_head_bytes))?
        .checked_add(u64::from(l.chunk).checked_mul(2)?)?
        .checked_mul(u64::from(l.hooks))?
        .checked_add(crate::hooks::storage(l.hooks)?)
}

fn plaintext_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for byte in name {
        if *byte <= b' ' || *byte >= 127 || b"/:[]".contains(byte) {
            return false;
        }
    }
    true
}

pub(crate) fn header_value(value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }
    for byte in value {
        if (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests;
