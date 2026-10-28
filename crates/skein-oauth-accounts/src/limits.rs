//! Account storage, exchange and transport bounds (oauth.md, section 6.7).

use crate::accounts::Source;
use crate::component::{Name, State};
use crate::exchange::Exchange;
use skein_lib::{Duration, Id, List, Slab};
use skein_oauth::ClientLimits;

pub(crate) const MAX_ORIGINS: u32 = 3;

/// The component's limits supplied by its owner before the loop.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// Configured and added accounts, including removals still settling.
    pub accounts: u32,
    /// Deadline stated on each private keeper request, including cleanup.
    pub file_stall: Duration,
    /// How long a shared write waits for the owner lock.
    pub lock_wait: Duration,
    pub exchanges: u32,
    /// Public redirect listeners admitted at once; zero permits only confidential sign-ins.
    pub listeners: u32,
    pub server: skein_http::server::Limits,
    /// How long before expiry a held record refreshes or an access-only one announces Expiring.
    pub refresh_lead: Duration,
    pub client: ClientLimits,
    pub http: skein_http::client::Limits,
    pub tls: skein_tls::client::Limits,
    /// The owner's socket bounds, included conservatively once in this footprint.
    pub io: skein_io::Limits,
}

/// Whether these limits are representable: every edge bound they size
/// is finite and every capacity they derive fits (programming-model.md, 6.3).
/// Account timing, socket tags, stacked demands and child limits are usable.
#[must_use]
pub fn valid(limits: &Limits) -> bool {
    limits.file_stall > Duration::ZERO
        && limits.lock_wait > Duration::ZERO
        && limits.accounts > 0
        && limits.exchanges > 0
        && limits.exchanges <= u32::MAX.checked_div(3).expect("nonzero divisor")
        && match socket_count(limits) {
            Some(sockets) => limits.io.sockets >= sockets,
            None => false,
        }
        && (limits.listeners == 0
            || (skein_http::server::valid(&limits.server)
                && skein_http::server::largest_read(&limits.server) <= limits.io.intake
                && skein_http::server::largest_room(&limits.server) <= limits.io.output))
        && limits.client.state_bytes >= 32
        && limits.client.document.token_bytes > 0
        && limits.client.document.record_bytes > 0
        && limits.http.head >= 2
        && limits.http.headers >= 2
        && limits.http.read > 0
        && limits.http.send > 0
        && limits.http.request > 0
        && limits.tls.read >= skein_http::client::largest_read(&limits.http)
        && limits.tls.send >= skein_http::client::largest_room(&limits.http)
        && limits.io.intake >= skein_tls::client::LARGEST_READ
        && limits.io.output >= skein_tls::client::largest_room(&limits.tls)
        && skein_oauth::client_valid(&limits.client)
        && skein_http::client::valid(&limits.http)
        && skein_tls::client::valid(&limits.tls)
        && io_capacity_bound(limits).is_some()
}

/// Account generations and registration, each exchange's queues and child machines, and socket buffers.
/// The owner's shared trust roots are counted once by the owner (tls.md, section 3.4).
#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64> {
    let record = u64::from(limits.client.document.record_bytes);
    // The client and the prepared SignIn each own a bounded registration and hints.
    let exchange = skein_oauth::client_worst_case(&limits.client)?
        .checked_mul(2)?
        .checked_add(skein_http::client::worst_case(&limits.http)?)?
        .checked_add(skein_tls::client::worst_case(&limits.tls)?)?
        .checked_add(u64::from(limits.client.scope_bytes))?
        .checked_add(u64::from(limits.client.document.subject_bytes))?
        .checked_add(u64::try_from(crate::RETURN_BYTES).ok()?)?
        .checked_add(
            u64::from(
                limits
                    .client
                    .document
                    .document_bytes
                    .max(limits.client.metadata_bytes)
                    .max(limits.client.key_set_bytes),
            )
            .checked_mul(2)?,
        )?
        .checked_add(
            u64::from(
                limits
                    .client
                    .document
                    .document_bytes
                    .max(limits.client.metadata_bytes)
                    .max(limits.client.key_set_bytes),
            )
            .checked_add(1)?,
        )?;
    let listener = if limits.listeners == 0 {
        0
    } else {
        skein_http::server::worst_case(&limits.server)?.checked_add(crate::listener::worst_case(limits)?)?
    };
    // Directory, filename and one outstanding request path, plus encoded load/store bytes.
    let private = 4095_u64.checked_mul(3)?.checked_add(record.checked_mul(2)?)?;
    let origins = List::<crate::Origin>::worst_case(MAX_ORIGINS)?
        .checked_add(u64::from(MAX_ORIGINS).checked_mul(u64::from(limits.client.uri_bytes).checked_add(253)?)?)?;
    List::<Name>::worst_case(limits.accounts)?
        .checked_add(List::<State>::worst_case(limits.accounts)?)?
        .checked_add(List::<Source>::worst_case(limits.accounts)?)?
        .checked_add(List::<crate::keeper::Store>::worst_case(limits.accounts)?)?
        .checked_add(u64::from(limits.accounts).checked_mul(private.checked_add(origins)?)?)?
        .checked_add(List::<Option<Id<Exchange>>>::worst_case(limits.accounts)?)?
        .checked_add(
            u64::from(limits.accounts)
                .checked_mul(record.checked_mul(3)?.checked_add(skein_oauth::client_worst_case(&limits.client)?)?)?,
        )?
        .checked_add(Slab::<Exchange>::worst_case(limits.exchanges)?)?
        .checked_add(u64::from(limits.exchanges).checked_mul(exchange)?)?
        .checked_add(u64::from(limits.listeners).checked_mul(listener)?)?
        .checked_add(skein_io::worst_case(&limits.io)?)
}

fn socket_count(limits: &Limits) -> Option<u32> {
    limits.listeners.checked_mul(2)?.checked_add(limits.exchanges)
}

// io retires operation slots until reclaim and reserves a second set.
// This bound also implies its two deadline slots per socket fit u32.
fn io_capacity_bound(limits: &Limits) -> Option<u32> {
    skein_io::operations(&limits.io)?.checked_mul(2)
}
