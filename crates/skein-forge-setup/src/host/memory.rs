//! Checked whole-host admission (forge-setup.md, 6.5).
use super::{Connection, Forge, Host, Kind, Limits, Run, Settings, Transport, connection::row_bound, limits};
use alloc::boxed::Box;
use skein_forge::{forgejo, github};
use skein_json::{tokenizer, writer};
use skein_lib::{Intake, List, Map, Queue, Stack, Token};

/// The most heap a `Host` built from these limits and fixed settings holds.
/// Counts tables, states, the largest configured plan and transport per run,
/// copied settings, request signing and io's share of each opened stream.
/// A kind or transport no forge uses costs nothing. Items, outcomes and
/// failures handed to the owner are the owner's, as is shared TLS trust.
/// The owner adds this bound to its own before starting; io's overlapping
/// stream bound overstates, never understates. `None` means refused inputs or
/// checked arithmetic past a `u64` (forge-setup.md, 6.5).
#[must_use]
pub fn worst_case(l: &Limits, settings: &Settings) -> Option<u64> {
    limits::check(l, settings).ok()?;
    let mut plan = 0;
    let mut connection = 0;
    let mut github = false;
    let mut configured = List::<Forge>::worst_case(settings.forges.capacity())?
        .checked_add(u64::try_from(settings.user_agent.len()).ok()?)?;
    for forge in &settings.forges {
        let (run, given_bytes) = match &forge.kind {
            Kind::Forgejo(g) => (crate::memory::per_run(g, l)?, crate::memory::copied(g)?),
            Kind::GitHub(g) => {
                github = true;
                let mut bytes = 0_u64;
                for text in [&g.word, &g.web, &g.url, &g.redirect_url, &g.setup_url] {
                    bytes = bytes.checked_add(u64::try_from(text.len()).ok()?)?;
                }
                (crate::github::memory::per_run(g)?, bytes)
            }
        };
        // Run's pending need/send may coexist with its machine's credential.
        let item = match &forge.kind {
            Kind::Forgejo(_) => u64::from(l.token_bytes.max(l.string_bytes)),
            Kind::GitHub(g) => u64::from(g.limits.key_bytes)
                .checked_mul(2)?
                .max(u64::from(g.limits.installations).checked_mul(8)?)
                .max(u64::from(l.token_bytes.max(l.string_bytes))),
        };
        plan =
            plan.max(run.checked_add(item.checked_mul(4)?)?.checked_add(u64::from(l.body_bytes_in).checked_mul(2)?)?);
        connection = connection.max(connection_heap(l, forge)?);
        configured = configured.checked_add(given_bytes)?.checked_add(match &forge.transport {
            Transport::Plaintext { name } => match name {
                Some(name) => u64::try_from(name.len()).ok()?,
                None => 0,
            },
            Transport::Tls { server_name, .. } => match server_name.server() {
                skein_tls::Server::Dns(text) => u64::try_from(text.len()).ok()?,
                skein_tls::Server::Ip(_) => 0,
            },
        })?;
    }
    Map::<Token, Run>::worst_case(l.runs)?
        .checked_add(Map::<Token, Connection>::worst_case(l.runs)?)?
        .checked_add(List::<Token>::worst_case(l.runs)?)?
        .checked_add(if github {
            Host::github_states_worst_case(l.states)?
        } else {
            Host::states_table_worst_case(l.states)?
        })?
        .checked_add(plan.checked_add(connection)?.checked_mul(u64::from(l.runs))?)?
        .checked_add(configured)
}
fn connection_heap(l: &Limits, forge: &Forge) -> Option<u64> {
    let tls = match &forge.transport {
        Transport::Plaintext { .. } => false,
        Transport::Tls { .. } => true,
    };
    let h = limits::http_transport(l, tls);
    let capacity = largest_page(l, &forge.kind)?;
    let (rows, decoder, jwt) = match &forge.kind {
        Kind::Forgejo(_) => (List::<forgejo::document::Row>::worst_case(capacity)?, forgejo_decoder(l)?, 0),
        Kind::GitHub(g) => (
            List::<github::document::Row>::worst_case(capacity)?,
            github_decoder(l)?,
            // Signing input, signature, token/bearer and signing-key scratch.
            u64::try_from(github::jwt::JWT_BYTES)
                .ok()?
                .checked_mul(4)?
                .checked_add(u64::from(g.limits.key_bytes).checked_mul(4)?)?,
        ),
    };
    skein_http::client::worst_case(&h)?
        .checked_add(if tls { skein_tls::client::worst_case(&l.tls)? } else { 0 })?
        .checked_add(tokenizer::worst_case(&limits::documents(l).json)?)?
        // Queued pieces of HTTP, TLS and JSON, even while closing.
        .checked_add(u64::from(h.send.max(l.chunk)).checked_mul(8)?)?
        .checked_add(if tls { u64::from(l.io.output).checked_mul(8)? } else { 0 })?
        .checked_add(u64::from(l.chunk).checked_mul(16)?)?
        // Shrinking the page into its boxed reply can retain both allocations.
        .checked_add(rows.checked_mul(2)?)?
        // Every retained row text consumes input; builder/document copies overlap.
        .checked_add(u64::from(l.body_bytes_in).checked_mul(3)?)?
        .checked_add(decoder)?
        // Call headers/target and their HTTP encoding, upload/writer overlap.
        .checked_add(List::<skein_http::Header>::worst_case(6)?.checked_mul(2)?)?
        .checked_add(u64::from(h.request).checked_mul(3)?)?
        .checked_add(u64::from(l.body_bytes_in).checked_mul(2)?)?
        .checked_add(writer::worst_case(&limits::documents(l).writer)?)?
        .checked_add(jwt)?
        .checked_add(Intake::worst_case(l.io.intake)?)?
        .checked_add(u64::from(l.io.receive))?
        .checked_add(u64::from(l.io.output))?
        .checked_add(Queue::<Box<[u8]>>::worst_case(l.io.sends)?)
}
// The private walkers' frames fit 128 bytes, including field masks and want.
// Setup selects no boxed item/file/review decoder. Its builder, delivered
// document and pending request fit these scalar and inner-list envelopes.
fn forgejo_decoder(l: &Limits) -> Option<u64> {
    u64::try_from(size_of::<forgejo::client::Decoder>())
        .ok()?
        .checked_add(Stack::<[u8; 128]>::worst_case(l.depth)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(l.inner)?.checked_mul(4)?)?
        .checked_add(u64::from(l.string_bytes).checked_mul(u64::from(l.inner).checked_add(32)?)?)?
        .checked_add(u64::from(l.body_bytes).checked_mul(2)?)
}
fn github_decoder(l: &Limits) -> Option<u64> {
    u64::try_from(size_of::<github::decode::Decoder>())
        .ok()?
        .checked_add(Stack::<[u8; 128]>::worst_case(l.depth)?)?
        // App builder and delivered permissions/events; decoder's eagerly
        // reserved labels, reviewers and errors even for setup documents.
        .checked_add(List::<github::document::Permission>::worst_case(l.inner)?.checked_mul(2)?)?
        .checked_add(List::<Box<[u8]>>::worst_case(l.inner)?.checked_mul(4)?)?
        .checked_add(List::<u64>::worst_case(l.inner)?)?
        .checked_add(u64::from(l.string_bytes).checked_mul(u64::from(l.inner).checked_mul(4)?.checked_add(32)?)?)?
        .checked_add(u64::from(l.body_bytes).checked_mul(2)?)
}

// Price the largest page any setup plan for this configured kind can request.
pub(super) fn largest_page(l: &Limits, kind: &Kind) -> Option<u32> {
    match kind {
        Kind::Forgejo(given) => {
            use forgejo::request::{Page, Request};
            let listing = Request::Tokens { login: Box::new([]), page: Page { page: Some(1), limit: Some(l.inner) } };
            let picker = Request::RepoSearch { page: Page { page: Some(1), limit: Some(given.limits.reachable) } };
            Some(row_bound(&listing.into())?.max(row_bound(&picker.into())?))
        }
        Kind::GitHub(given) => {
            use github::request::Request;
            let picker = Request::Reachable { page: 1, per_page: given.limits.reachable.min(github::pin::PAGE_CAP) };
            Some(row_bound(&Request::Installations { page: 1 }.into())?.max(row_bound(&picker.into())?))
        }
    }
}
