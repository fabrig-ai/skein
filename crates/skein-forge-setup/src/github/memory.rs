//! Checked heap admission for pure GitHub plans (forge-setup.md, 7).
use super::{
    accept::Accept,
    checks,
    convert::Convert,
    given::{self, Given},
    install::Install,
    manifest::Manifest,
    paste::Paste,
    reconcile,
};
use crate::{
    host::{Host, run::Machine},
    reachable::{Reachable, Reached},
};
use skein_json::writer;
use skein_lib::{
    List,
    base64::{self, Alphabet},
};

/// Heap for `runs` pure GitHub plan machines and the host's `states` table.
/// Includes copied Given text, boxed reconcile machines, child scratch, maximum
/// keys, conversion code/slug, Accepted and missing-access scratch, and pending
/// App credential copies. Paste includes PEM/DER overlap, checked identities,
/// missing access, hook scratch and pending identity writes (5.8). Each run retains
/// its pending need.
///
/// Excludes the outer host's run/connection tables, HTTP/io/JSON stack, signing,
/// caller-owned item store and reply documents, and handed-out output payloads
/// (including minted bearer tokens). Owner-held form targets and manifests are separately
/// bounded by `states * 2 * page_bytes` (5.1 and 5.7). This is not a bound for `Host`.
/// Returns None for invalid Given or any capacity/byte arithmetic overflow.
#[must_use]
pub fn worst_case(given: &Given, runs: u32, states: u32) -> Option<u64> {
    per_run(given)?.checked_mul(u64::from(runs))?.checked_add(Host::github_states_worst_case(states)?)
}
/// One pure plan, shared with whole-host admission.
pub(crate) fn per_run(given: &Given) -> Option<u64> {
    given::check(given).ok()?;
    let plan = manifest(given)?
        .max(convert(given)?)
        .max(paste(given)?)
        .max(install(given)?)
        .max(accept(given)?)
        .max(reconcile::worst_case(given)?)
        .max(reachable(given)?);
    plan.checked_add(u64::try_from(size_of::<Machine>()).ok()?)
}

fn copied(given: &Given) -> Option<u64> {
    let mut bytes = 0_u64;
    for text in [&given.word, &given.web, &given.url, &given.redirect_url, &given.setup_url] {
        bytes = bytes.checked_add(u64::try_from(text.len()).ok()?)?;
    }
    Some(bytes)
}
fn manifest(given: &Given) -> Option<u64> {
    let name = u64::try_from(given.word.len()).ok()?.checked_add(7)?;
    // Retained inputs, generated name/hook, JSON writer and form target.
    u64::try_from(size_of::<Manifest>())
        .ok()?
        .checked_add(copied(given)?)?
        .checked_add(u64::from(given.limits.name_bytes))?
        .checked_add(32)?
        .checked_add(name)?
        .checked_add(name.checked_add(17)?)?
        .checked_add(writer::worst_case(&writer::Limits { depth: 2, length: given.limits.page_bytes })?)?
        .checked_add(u64::from(given.limits.page_bytes))
}
fn convert(given: &Given) -> Option<u64> {
    let key = given.limits.key_bytes;
    // Pending code; PEM's admitted envelope, whitespace-stripped base64, DER,
    // and outgoing proof credential. Later writes retain only slug and key.
    u64::try_from(size_of::<Convert>())
        .ok()?
        .checked_add(u64::try_from(given::URL_BYTES).ok()?)?
        .checked_add(u64::try_from(given::URL_BYTES).ok()?)?
        .checked_add(u64::from(key).checked_mul(2)?)?
        .checked_add(u64::from(base64::encoded_len(Alphabet::Standard, key)?))?
        .checked_add(u64::from(key))?
        .checked_add(u64::from(key))?
        .checked_add(u64::from(given.limits.name_bytes))?
        .checked_add(u64::from(given.limits.name_bytes))?
        .checked_add(u64::from(given.limits.name_bytes))?
        .checked_add(40)
}
fn install(given: &Given) -> Option<u64> {
    u64::try_from(size_of::<Install>())
        .ok()?
        .checked_add(copied(given)?)?
        .checked_add(32)?
        .checked_add(u64::from(given.limits.name_bytes))?
        .checked_add(u64::from(given.limits.page_bytes))
}
fn accept(given: &Given) -> Option<u64> {
    // Decoded ids, insertion list (and its shrinking allocation), encoded item;
    // the retained and pending App credentials, decimal subject and its copy.
    let ids = List::<u64>::worst_case(given.limits.installations)?;
    u64::try_from(size_of::<Accept>())
        .ok()?
        .checked_add(ids)?
        .checked_add(ids)?
        .checked_add(ids)?
        .checked_add(ids)?
        .checked_add(u64::from(given.limits.key_bytes))?
        .checked_add(u64::from(given.limits.key_bytes))?
        .checked_add(40)
}
fn reachable(given: &Given) -> Option<u64> {
    // GitHub retains neither minted tokens nor repository names: both move to
    // the outgoing need. Count the reached array while it is built, Accepted
    // decoding, and the retained and pending credentials during the mint.
    u64::try_from(size_of::<Reachable>())
        .ok()?
        .checked_add(List::<u64>::worst_case(given.limits.installations)?)?
        .checked_add(List::<u64>::worst_case(given.limits.installations)?)?
        .checked_add(List::<Reached>::worst_case(given.limits.reachable.min(skein_forge::github::pin::PAGE_CAP))?)?
        .checked_add(u64::from(given.limits.key_bytes))?
        .checked_add(u64::from(given.limits.key_bytes))?
        .checked_add(40)
}

// PEM, compact base64 and DER overlap while decoding. Hook repair retains three
// credential copies; comparison scratch and pending writes are bounded as 5.8.
fn paste(given: &Given) -> Option<u64> {
    let key = given.limits.key_bytes;
    u64::try_from(size_of::<Paste>())
        .ok()?
        .checked_add(copied(given)?)?
        .checked_add(u64::from(key).checked_mul(5)?)?
        .checked_add(u64::from(base64::encoded_len(Alphabet::Standard, key)?))?
        .checked_add(u64::from(given.limits.name_bytes).checked_mul(4)?)?
        .checked_add(u64::try_from(b"https://.invalid/".len()).ok()?)?
        .checked_add(checks::worst_case()?)?
        .checked_add(40)
}
