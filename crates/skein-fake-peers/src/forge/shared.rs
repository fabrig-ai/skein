//! Shared pending ownership and hosted configuration checks.
use super::layer as protocol;
use super::{Forge, call_capacity};
use crate::{Limits, face};
use skein_fake_forge_domain as domain;
use skein_lib::{Queue, Time, Token};

const EXTERNAL: u64 = 7 << 60;
impl Forge {
    #[must_use]
    pub fn new(domain: domain::Domain, config: domain::Config) -> Self {
        let capacity = config.limits.calls.checked_add(config.limits.hooks).expect("bounded pending capacity");
        Self { domain, config, now: Time::ZERO, pending: Queue::with_capacity(capacity), hooks: None, serial: 0 }
    }
    /// Allocates a shared reply identity for a peer or another domain caller.
    /// Peers translate their connection-local reply rights through this range.
    pub fn next_reply(&mut self) -> Token {
        self.serial =
            self.serial.checked_add(1).filter(|serial| *serial < (1 << 60)).expect("finite external reply rights");
        Token::new(EXTERNAL.checked_add(self.serial).expect("disjoint external reply range"))
    }
}

/// Whether these limits are representable: every edge bound they size is finite
/// and every capacity they derive fits (programming-model.md, 6.3).
/// Hosted calls, sockets, buffers, domain configuration and hook payloads fit.
pub(super) fn valid(limits: &Limits, protocol: &protocol::Limits, forge: &Forge) -> bool {
    capacities(limits, protocol, forge).is_some()
}

fn capacities(limits: &Limits, protocol: &protocol::Limits, forge: &Forge) -> Option<()> {
    let config = &forge.config;
    let domain = &config.limits;
    let listeners = match protocol {
        protocol::Limits::Github(_) => 2,
        protocol::Limits::Forgejo(_) => 1,
    };
    if !face::valid(limits)
        || !domain::valid(domain)
        || !skein_http::server::valid(&protocol.http())
        || limits.io.sockets < limits.connections.checked_mul(2)?.checked_add(listeners)?
        || limits.connections > protocol.connections(limits.connections)
        || protocol.calls(limits.connections) >= (7 << 28)
        || limits.plaintext < skein_http::server::largest_read(&protocol.http())
        || limits.io.output < skein_http::server::largest_room(&protocol.http())
        || forge.pending.capacity() < domain.calls.checked_add(domain.hooks)?
        || domain.repositories == 0
        || domain.users == 0
        || domain.items == 0
        || domain.commits == 0
        || domain.files == 0
        || domain.branches == 0
        || domain.page_size == 0
        || domain.calls == 0
        || config.latency_min > config.latency_max
        || config.settle_min > config.settle_max
        || config.late_min > config.late_max
        || config.land_min > config.land_max
        || config.hook_min > config.hook_max
        || (config.rate_limit > 0 && config.rate_window == skein_lib::Duration::ZERO)
        || [config.late, config.unavailable, config.timeouts, config.landing, config.hooks_late, config.hooks_lost]
            .into_iter()
            .any(|chance| chance > 1000)
    {
        return None;
    }
    if let protocol::Limits::Forgejo(limits) = protocol
        && !skein_fake_forge_protocol::git::fits(limits, domain)
    {
        return None;
    }
    if let protocol::Limits::Github(github) = protocol {
        // Domain text may need six JSON bytes per input byte. Leave room
        // for the delivery envelope before admitting any hook source.
        let payload = domain.body_bytes.checked_mul(6)?.checked_add(1024)?;
        let named = domain.name_bytes.checked_mul(12)?.checked_add(1024)?;
        if github.deliveries.payload_bytes < payload.max(named)
            || github.http.response < github.github.string_bytes.checked_mul(2)?.checked_add(512)?
        {
            return None;
        }
    }
    call_capacity(protocol, forge, limits.connections)?;
    if let protocol::Limits::Github(github) = protocol
        && !super::github_delivery::valid(github)
    {
        return None;
    }
    Some(())
}
