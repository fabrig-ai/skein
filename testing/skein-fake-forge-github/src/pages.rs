//! Browser pages and their single-use conversion codes (fake-forge.md, 10.1).
use crate::{
    answers,
    app::App,
    installations::{Install, Kind, Reach},
    keys,
    manifest::{self, Manifest},
    service::{Limits as ServiceLimits, Response, Service},
};
use alloc::boxed::Box;
use skein_forge::github::{document, request::Method, server};
use skein_lib::{Decimal, Duration, List, Map, Rng, Secret, Wall, Writer, bytes};

/// One hour on the forge's clock; the expiry instant is excluded.
pub const CODE_LIFE: Duration = Duration::from_secs(3600);
/// Bounds for the state-indexed manifest store, with one code per state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub states: u32,
    pub manifest_bytes: u32,
}
/// A browser request, with no API headers. Its target may contain a code.
pub struct Page<'a> {
    pub method: Method,
    pub target: &'a [u8],
    pub body: &'a [u8],
}
impl core::fmt::Debug for Page<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Page")
            .field("method", &self.method)
            .field("target_bytes", &self.target.len())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}
/// The browser response's media; refusal bodies use the API's JSON vocabulary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Html,
    None,
}
/// A browser response above HTTP framing. Debug omits the link and body's secrets.
pub struct Answer {
    pub status: u16,
    pub location: Option<Box<[u8]>>,
    pub media: Media,
    pub body: Box<[u8]>,
}
impl core::fmt::Debug for Answer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Answer")
            .field("status", &self.status)
            .field("media", &self.media)
            .field("redirect", &self.location.is_some())
            .field("body_bytes", &self.body.len())
            .finish()
    }
}
#[derive(Debug)]
pub(crate) struct Record {
    pub(crate) manifest: Manifest,
    code: Secret,
    expires: Wall,
    used: bool,
}
#[derive(Debug)]
pub(crate) struct Store {
    pub(crate) states: Map<Box<[u8]>, Record>,
    rng: Rng,
    key: u32,
    pub(crate) setup: Option<Box<[u8]>>,
}
impl Store {
    pub(crate) fn new(limits: Limits, seed: u64) -> Self {
        Self { states: Map::with_capacity(limits.states), rng: Rng::new(seed), key: 0, setup: None }
    }
    fn reclaim(&mut self, now: Wall) -> Option<()> {
        let mut dropped = List::with_capacity(self.states.capacity());
        for (state, record) in &self.states {
            if record.used || now >= record.expires {
                dropped.push(state.clone()).ok()?;
            }
        }
        for state in &dropped {
            drop(self.states.remove(state.as_ref()));
        }
        Some(())
    }
}
fn from_response(response: Response) -> Answer {
    Answer { status: response.status, body: response.body, location: None, media: Media::None }
}
fn invalid(limits: &ServiceLimits) -> Answer {
    from_response(answers::refusal(422, b"Validation Failed", limits))
}
fn full(limits: &ServiceLimits) -> Answer {
    from_response(answers::refusal(503, b"Server Error", limits))
}
#[derive(Debug)]
enum Route<'a> {
    Manifest(&'a [u8]),
    Install(&'a [u8]),
}
fn route(method: Method, path: &[u8]) -> Option<Route<'_>> {
    if method == Method::Post {
        if path == b"/settings/apps/new" {
            return Some(Route::Manifest(b""));
        }
        if let Some(org) = path.strip_prefix(b"/organizations/") {
            let org = org.strip_suffix(b"/settings/apps/new")?;
            if !org.is_empty() && !org.contains(&b'/') {
                return Some(Route::Manifest(org));
            }
        }
    }
    if method == Method::Get {
        let slug = path.strip_prefix(b"/apps/")?.strip_suffix(b"/installations/new")?;
        if !slug.is_empty() && !slug.contains(&b'/') {
            return Some(Route::Install(slug));
        }
    }
    None
}
/// Whether the browser layer has a route for this method and target's path.
pub(crate) fn accepts(method: Method, target: &[u8]) -> bool {
    let Some((path, _query)) = split(target, b'?') else { return false };
    route(method, path).is_some()
}
pub(crate) fn serve(service: &mut Service, page: &Page<'_>, now: Wall) -> Answer {
    let limits = service.limits;
    if !fits(page.target, limits.github.target_bytes) || page.target.contains(&b'#') {
        return from_response(answers::missing(&limits));
    }
    let Some((path, query)) = split(page.target, b'?') else {
        return from_response(answers::failed());
    };
    let Some(route) = route(page.method, path) else {
        return from_response(answers::missing(&limits));
    };
    if !fits(page.body, limits.body) {
        return from_response(answers::refusal(413, b"Payload Too Large", &limits));
    }
    let Some(query) = fields(query, false) else {
        return invalid(&limits);
    };
    let Some(state) = query.get(b"state".as_slice()) else {
        return invalid(&limits);
    };
    if !unreserved(state) {
        return invalid(&limits);
    }
    match route {
        Route::Manifest(org) => post(service, org, state, page.body, now),
        Route::Install(slug) => install(service, slug, state, &query, now),
    }
}
fn post(service: &mut Service, org: &[u8], state: &[u8], body: &[u8], now: Wall) -> Answer {
    let limits = service.limits;
    let login = if org.is_empty() { service.person.as_ref() } else { org };
    let mut account = None;
    for row in &service.accounts {
        if row.login.as_ref() == login && (org.is_empty() || row.organization) {
            account = Some(row);
        }
    }
    let Some(account) = account else {
        return invalid(&limits);
    };
    let Some(form) = fields(body, true) else {
        return invalid(&limits);
    };
    let Some(input) = form.get(b"manifest".as_slice()) else {
        return invalid(&limits);
    };
    if !fits(input, limits.pages.manifest_bytes) {
        return invalid(&limits);
    }
    let Some(manifest) = manifest::read(input, &account.login, &limits.github) else {
        return invalid(&limits);
    };
    let Some(expires) = now.as_nanos().checked_add(CODE_LIFE.as_nanos()) else {
        return full(&limits);
    };
    if !service.pages.states.contains_key(state) && service.pages.states.len() == limits.pages.states {
        if service.pages.reclaim(now).is_none() {
            return from_response(answers::failed());
        }
        if service.pages.states.len() == limits.pages.states {
            return full(&limits);
        }
    }
    let Some(code) = hex(&mut service.pages.rng, 20) else {
        return from_response(answers::failed());
    };
    let Some(link) = join(&[&manifest.redirect_url, b"?code=", code.expose(), b"&state=", state], limits.response)
    else {
        return from_response(answers::failed());
    };
    let Some(body) = link_page(&link, limits.response) else {
        return from_response(answers::failed());
    };
    let record = Record { manifest, code, expires: Wall::from_nanos(expires), used: false };
    if service.pages.states.insert(bytes::copy_of(state), record).is_err() {
        return full(&limits);
    }
    Answer { status: 200, location: None, media: Media::Html, body }
}

pub(crate) fn convert(service: &mut Service, code: &[u8], body: &[u8], now: Wall) -> Response {
    let limits = service.limits;
    if !fits(body, limits.body) {
        return answers::refusal(413, b"Payload Too Large", &limits);
    }
    let mut state = None;
    for (key, record) in &service.pages.states {
        if record.code.expose() == code && !record.used && now < record.expires {
            state = Some(key.clone());
        }
    }
    let Some(state) = state else {
        return answers::missing(&limits);
    };
    let Some(record) = service.pages.states.get(state.as_ref()) else {
        return answers::failed();
    };
    let manifest = &record.manifest;
    let Some(slug) = slug(&manifest.name) else {
        return answers::refusal(422, b"Validation Failed", &limits);
    };
    // The scenario's domain bot keeps its id across conversions; App ids skip it.
    let Some(next) = service.app.id.checked_add(1) else {
        return answers::refusal(503, b"Server Error", &limits);
    };
    let id = if next == service.bot { next.checked_add(1) } else { Some(next) };
    let Some(id) = id else {
        return answers::refusal(503, b"Server Error", &limits);
    };
    let mut account = None;
    for row in &service.accounts {
        if row.login == manifest.account {
            account = Some(row);
        }
    }
    let Some(account) = account else {
        return answers::refusal(422, b"Validation Failed", &limits);
    };
    let Some(key) = keys::KEYS.get(match usize::try_from(service.pages.key) {
        Ok(key) => key,
        Err(_) => return answers::failed(),
    }) else {
        return answers::failed();
    };
    let Some(pem) = keys::pem(key) else {
        return answers::failed();
    };
    let Some(webhook) = hex(&mut service.pages.rng, 32) else {
        return answers::failed();
    };
    let conversion = document::Conversion {
        id,
        slug: slug.clone(),
        owner: account.login.clone(),
        owner_id: account.id,
        pem: Secret::new(pem),
        webhook_secret: webhook.clone(),
    };
    let Ok(capacity) = usize::try_from(limits.response) else {
        return answers::failed();
    };
    let mut out = Writer::new(capacity);
    if server::conversion(&conversion, &mut out).is_err() {
        return answers::failed();
    }
    let Some(body) = answers::finish(out) else {
        return answers::failed();
    };
    let hook = match &manifest.hook_attributes {
        Some(hook) => hook.url.clone(),
        None => None,
    };
    service.pages.setup = manifest.setup_url.clone();
    if let Some(hook) = hook {
        service.hook = hook;
    }
    let grants = app_grants(manifest, service.app_facts);
    service.verified.forget();
    service.installations.replace_app();
    service.app =
        App { id, slug, owner: account.id, key, webhook, permissions: grants.permissions, events: grants.events };
    service.pages.key = match service.pages.key.checked_add(1) {
        Some(next) => next.rem_euclid(3),
        None => return answers::failed(),
    };
    let Some(record) = service.pages.states.get_mut(state.as_ref()) else {
        return answers::failed();
    };
    record.used = true;
    Response {
        rate: None,
        etag: None,
        content_type: crate::service::ContentType::Json,
        status: 201,
        link: None,
        location: None,
        body,
    }
}
#[derive(Debug)]
struct Grants {
    permissions: Box<[document::Permission]>,
    events: Box<[Box<[u8]>]>,
}
fn app_grants(manifest: &Manifest, facts: crate::service::AppFacts) -> Grants {
    match facts {
        crate::service::AppFacts::Complete | crate::service::AppFacts::Omitted => Grants {
            permissions: manifest.default_permissions.clone().into_boxed(),
            events: manifest.default_events.clone().into_boxed(),
        },
        crate::service::AppFacts::Narrow => {
            let mut permissions = List::with_capacity(1);
            if let Some(permission) = manifest.default_permissions.get(0) {
                permissions
                    .push(document::Permission { name: permission.name.clone(), access: document::Access::Read })
                    .expect("one permission");
            }
            let mut events = List::with_capacity(1);
            if let Some(event) = manifest.default_events.get(0) {
                events.push(event.clone()).expect("one event");
            }
            Grants { permissions: permissions.into_boxed(), events: events.into_boxed() }
        }
    }
}
fn slug(name: &[u8]) -> Option<Box<[u8]>> {
    let bound = u32::try_from(name.len()).ok()?;
    let mut out = List::with_capacity(bound);
    let mut separator = false;
    for byte in name {
        let byte = byte.to_ascii_lowercase();
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            if separator && !out.is_empty() {
                out.push(b'-').ok()?;
            }
            out.push(byte).ok()?;
            separator = false;
        } else {
            separator = true;
        }
    }
    if out.is_empty() { None } else { Some(out.into_boxed()) }
}
fn install(service: &mut Service, slug: &[u8], state: &[u8], query: &Map<Box<[u8]>, Box<[u8]>>, now: Wall) -> Answer {
    let limits = service.limits;
    if slug != service.app.slug.as_ref() {
        return from_response(answers::missing(&limits));
    }
    let Some(setup) = &service.pages.setup else {
        return invalid(&limits);
    };
    let mut account = None;
    for row in &service.accounts {
        let chosen = match query.get(b"account".as_slice()) {
            Some(login) => row.login == *login,
            None => row.id == service.app.owner,
        };
        if chosen {
            account = Some(row);
        }
    }
    let Some(account) = account else {
        return invalid(&limits);
    };
    let Some(reach) = reach(service, query) else {
        return invalid(&limits);
    };
    let mut existing = None;
    for row in service.installations.iter() {
        if row.account == account.id {
            existing = Some(row.id);
        }
    }
    let action = if existing.is_some() { b"update".as_slice() } else { b"install".as_slice() };
    // Reserve room for even the longest id before changing the installation store.
    if redirect(setup, u64::MAX, action, state, limits.response).is_none() {
        return from_response(answers::failed());
    }
    let id = match existing {
        Some(id) => {
            if !service.installations.set_reach(id, reach) {
                return invalid(&limits);
            }
            id
        }
        None => {
            let kind = if account.organization { Kind::Organization } else { Kind::User };
            match service.installations.install(account.id, &account.login, kind, reach, now) {
                Ok(id) => id,
                Err(Install::Full) => return full(&limits),
                Err(Install::Login) => return invalid(&limits),
            }
        }
    };
    let Some(location) = redirect(setup, id, action, state, limits.response) else {
        return from_response(answers::failed());
    };
    Answer { status: 302, location: Some(location), media: Media::None, body: Box::from([]) }
}
fn reach(service: &Service, query: &Map<Box<[u8]>, Box<[u8]>>) -> Option<Reach> {
    let all = query.get(b"repositories".as_slice());
    let listed = query.get(b"repository_ids".as_slice());
    if let Some(all) = all
        && (all.as_ref() != b"all" || listed.is_some())
    {
        return None;
    }
    let Some(listed) = listed else {
        return Some(Reach::All);
    };
    if listed.is_empty() {
        return None;
    }
    let mut ids = List::with_capacity(service.limits.installations.listed);
    let mut rest = listed.as_ref();
    for _id in 0..listed.len() {
        let (text, tail) = split(rest, b',')?;
        let id = unsigned(text)?;
        if !service.repositories.contains_key(&id) || ids.as_slice().contains(&id) {
            return None;
        }
        ids.push(id).ok()?;
        if tail.is_empty() {
            if rest.ends_with(b",") {
                return None;
            }
            return Some(Reach::Listed(ids));
        }
        rest = tail;
    }
    None
}
fn redirect(setup: &[u8], id: u64, action: &[u8], state: &[u8], max: u32) -> Option<Box<[u8]>> {
    let digits = Decimal::of(id);
    join(&[setup, b"?installation_id=", digits.as_bytes(), b"&setup_action=", action, b"&state=", state], max)
}
fn unsigned(text: &[u8]) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let mut id = 0_u32;
    for byte in text {
        if !byte.is_ascii_digit() {
            return None;
        }
        id = id.checked_mul(10)?.checked_add(u32::from(byte.checked_sub(b'0')?))?;
    }
    Some(id)
}
pub(crate) fn unreserved(text: &[u8]) -> bool {
    if text.is_empty() {
        return false;
    }
    for byte in text {
        if !byte.is_ascii_alphanumeric() && !b"._~-".contains(byte) {
            return false;
        }
    }
    true
}
fn fits(input: &[u8], max: u32) -> bool {
    match u32::try_from(input.len()) {
        Ok(length) => length <= max,
        Err(_) => false,
    }
}
fn split(input: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
    for (at, byte) in input.iter().enumerate() {
        if *byte == delimiter {
            return Some((input.get(..at)?, input.get(at.checked_add(1)?..)?));
        }
    }
    Some((input, b""))
}
// Query state is raw URI-unreserved text, so it can be echoed byte for byte.
type Fields = Map<Box<[u8]>, Box<[u8]>>;
fn fields(input: &[u8], form: bool) -> Option<Fields> {
    let mut kept = Map::with_capacity(u32::try_from(input.len()).ok()?);
    let mut rest = input;
    for _field in 0..input.len() {
        if rest.is_empty() {
            break;
        }
        let (pair, tail) = split(rest, b'&')?;
        rest = tail;
        let (key, value) = split(pair, b'=')?;
        let key = unescape(key)?;
        let value = if form { unescape(value)? } else { bytes::copy_of(value) };
        if kept.insert(key, value).ok()?.is_some() {
            return None;
        }
    }
    Some(kept)
}
fn unescape(input: &[u8]) -> Option<Box<[u8]>> {
    let mut out = List::with_capacity(u32::try_from(input.len()).ok()?);
    let mut at = 0_usize;
    for _byte in 0..input.len() {
        let Some(byte) = input.get(at) else {
            break;
        };
        let byte = match byte {
            b'+' => b' ',
            b'%' => {
                let high = nibble(*input.get(at.checked_add(1)?)?)?;
                let low = nibble(*input.get(at.checked_add(2)?)?)?;
                at = at.checked_add(2)?;
                high.checked_mul(16)?.checked_add(low)?
            }
            byte => *byte,
        };
        out.push(byte).ok()?;
        at = at.checked_add(1)?;
    }
    Some(out.into_boxed())
}
fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => byte.checked_sub(b'0'),
        b'a'..=b'f' => byte.checked_sub(b'a')?.checked_add(10),
        b'A'..=b'F' => byte.checked_sub(b'A')?.checked_add(10),
        _ => None,
    }
}
fn hex(rng: &mut Rng, length: usize) -> Option<Secret> {
    let mut out = Writer::new(length);
    for _byte in 0..length {
        let index = usize::try_from(rng.below(16)).ok()?;
        out.put(&[*b"0123456789abcdef".get(index)?]).ok()?;
    }
    Some(Secret::new(out.finish()))
}
fn join(parts: &[&[u8]], max: u32) -> Option<Box<[u8]>> {
    let mut length = 0_usize;
    for part in parts {
        length = length.checked_add(part.len())?;
    }
    if length > usize::try_from(max).ok()? {
        return None;
    }
    let mut out = Writer::new(length);
    for part in parts {
        out.put(part).ok()?;
    }
    Some(out.finish())
}
fn escaped(byte: u8) -> Option<&'static [u8]> {
    match byte {
        b'&' => Some(b"&amp;"),
        b'<' => Some(b"&lt;"),
        b'>' => Some(b"&gt;"),
        b'"' => Some(b"&quot;"),
        b'\'' => Some(b"&#39;"),
        _ => None,
    }
}
fn link_page(link: &[u8], max: u32) -> Option<Box<[u8]>> {
    const START: &[u8] = b"<!doctype html><meta charset=\"utf-8\"><title>GitHub App</title><a id=\"create\" href=\"";
    const END: &[u8] = b"\">Create GitHub App</a>";
    let mut length = START.len().checked_add(END.len())?;
    for byte in link {
        let width = match escaped(*byte) {
            Some(entity) => entity.len(),
            None => 1,
        };
        length = length.checked_add(width)?;
    }
    if length > usize::try_from(max).ok()? {
        return None;
    }
    let mut out = Writer::new(length);
    out.put(START).ok()?;
    for byte in link {
        match escaped(*byte) {
            Some(entity) => out.put(entity).ok()?,
            None => out.put(&[*byte]).ok()?,
        }
    }
    out.put(END).ok()?;
    Some(out.finish())
}
pub(crate) fn worst_case(limits: &ServiceLimits) -> Option<u64> {
    let record = manifest::worst_case(&limits.github, limits.pages.manifest_bytes)?
        .checked_add(u64::from(limits.github.target_bytes).checked_mul(2)?)?
        .checked_add(20)?;
    Map::<Box<[u8]>, Record>::worst_case(limits.pages.states)?
        .checked_add(List::<Box<[u8]>>::worst_case(limits.pages.states)?)?
        .checked_add(record.checked_mul(u64::from(limits.pages.states))?)?
        // Both parameter maps, decoded form copies, and container/input scratch.
        .checked_add(Fields::worst_case(limits.body)?)?
        .checked_add(Fields::worst_case(limits.github.target_bytes)?)?
        .checked_add(skein_lib::Intake::worst_case(
            limits.pages.manifest_bytes.max(skein_json::tokenizer::largest_demand(&limits.github.json)),
        )?)?
        .checked_add(List::<u64>::worst_case(limits.github.json.depth)?)?
        .checked_add(u64::from(limits.github.string_bytes).checked_mul(2)?)
}
