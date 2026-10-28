//! Startup limits and checked memory bounds (llm-connection.md, section 7).

use alloc::boxed::Box;
use skein_lib::{Duration, Id, Intake, List, Queue, Slab};

use crate::call::Connection;

/// Bounded pool and child machine limits, fixed at startup.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub endpoints: u32,
    pub connections: u32,
    pub calls: u32,
    pub per_endpoint: u32,
    /// Bytes calls may reserve at once from the owner's LLM memory pool.
    pub memory: u64,
    pub idle_keep: Duration,
    pub io: skein_io::Limits,
    pub tls: skein_tls::client::Limits,
}

/// Whether these limits are representable: every edge bound they size
/// is finite and every capacity they derive fits (programming-model.md, 6.3).
/// Each endpoint's largest call reservation fits the shared memory pool.
#[must_use]
pub fn valid(limits: &Limits, endpoints: &List<crate::Endpoint>) -> bool {
    if limits.endpoints == 0
        || limits.connections == 0
        || limits.calls == 0
        || limits.per_endpoint == 0
        || limits.connections < limits.calls
        || limits.per_endpoint > limits.connections
        || endpoints.len() > limits.endpoints
    {
        return false;
    }
    for endpoint in endpoints {
        let mut client = endpoint.limits;
        client.http.request = match skein_llm::client::request_head(&endpoint.llm, &endpoint.credential, &client) {
            Ok(head) => head,
            Err(_) => return false,
        };
        if !skein_llm::client::valid(&client) {
            return false;
        }
        match skein_llm::client::largest_reservation(&client) {
            Some(reservation) if reservation <= limits.memory => {}
            Some(_) | None => return false,
        }
        match &endpoint.transport {
            crate::Transport::Tls { .. } => {
                if !skein_tls::client::valid(&limits.tls) {
                    return false;
                }
            }
            crate::Transport::Plaintext => {}
        }
    }
    true
}

/// Conservative bytes for the complete pool and component tables.
#[must_use]
pub fn worst_case(limits: &Limits, endpoints: &List<crate::Endpoint>) -> Option<u64> {
    if !valid(limits, endpoints) {
        return None;
    }
    // One physical socket owns these buffers. The owner prices io's entity,
    // flight and deadline tables once; they are never a pool multiplier.
    let socket = Intake::worst_case(limits.io.intake)?
        .checked_add(u64::from(limits.io.receive))?
        .checked_add(u64::from(limits.io.output))?
        .checked_add(Queue::<Box<[u8]>>::worst_case(limits.io.sends)?)?;
    let mut largest = 0_u64;
    let mut scratch = 0_u64;
    let mut waiting_endpoint = 0_u64;
    for endpoint in endpoints {
        let mut owned = u64::try_from(endpoint.llm.authority.len())
            .ok()?
            .checked_add(u64::try_from(endpoint.llm.target.len()).ok()?)?
            .checked_add(u64::try_from(size_of_val(endpoint.llm.headers.as_ref())).ok()?)?;
        for header in &endpoint.llm.headers {
            owned = owned
                .checked_add(u64::try_from(header.name.len()).ok()?)?
                .checked_add(u64::try_from(header.value.len()).ok()?)?;
        }
        waiting_endpoint = waiting_endpoint.max(owned);
        let mut client = endpoint.limits;
        client.http.request = match skein_llm::client::request_head(&endpoint.llm, &endpoint.credential, &client) {
            Ok(head) => head,
            Err(_) => return None,
        };
        if skein_llm::client::largest_reservation(&client)? > limits.memory {
            return None;
        }
        let fixed = skein_llm::client::worst_case(&client)?;
        let transport = match &endpoint.transport {
            crate::Transport::Tls { .. } => skein_tls::client::worst_case(&limits.tls)?,
            crate::Transport::Plaintext => 0,
        };
        let each = socket.checked_add(transport)?.checked_add(fixed)?;
        largest = largest.max(each);
        // One entrance measures or prepares one call, never one per connection.
        // Its prompt stays borrowed; only depth-bounded encoding state and
        // one fresh client's overlap during reuse belong to this scratch.
        scratch = scratch.max(skein_llm::client::encoding_worst_case(&client)?.checked_add(fixed)?);
    }
    let pool = largest.checked_mul(u64::from(limits.connections))?;
    // Slab and waiting-record sizes include each call's measure table and
    // anchors. Measures move in its existing terminal, adding no routing queue.
    pool.checked_add(limits.memory)?
        .checked_add(scratch)?
        .checked_add(Slab::<Connection>::worst_case(limits.connections)?)?
        .checked_add(List::<Option<Id<Connection>>>::worst_case(limits.connections)?)?
        .checked_add(List::<Option<crate::component::Waiting>>::worst_case(limits.calls)?)?
        // Each owned measurement retains its endpoint until preparation.
        .checked_add(waiting_endpoint.checked_mul(u64::from(limits.calls))?)?
        .checked_add(List::<crate::Endpoint>::worst_case(limits.endpoints)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_scratch_is_charged_once_for_all_connections() {
        let small = crate::tests::endpoint();
        let mut large = crate::tests::endpoint();
        large.limits.depth = small.limits.depth + 2;
        let scratch_growth = skein_llm::client::encoding_worst_case(&large.limits).unwrap()
            - skein_llm::client::encoding_worst_case(&small.limits).unwrap();
        let fixed_growth = skein_llm::client::worst_case(&large.limits).unwrap()
            - skein_llm::client::worst_case(&small.limits).unwrap();
        assert!(scratch_growth > 0);
        let mut small_endpoints = List::with_capacity(1);
        small_endpoints.push(small).unwrap();
        let mut large_endpoints = List::with_capacity(1);
        large_endpoints.push(large).unwrap();
        for connections in [1, 4] {
            let mut limits = crate::tests::limits();
            limits.connections = connections;
            limits.io.sockets = connections;
            let small = worst_case(&limits, &small_endpoints).unwrap();
            let large = worst_case(&limits, &large_endpoints).unwrap();
            assert_eq!(
                large - small - u64::from(connections + 1) * fixed_growth,
                scratch_growth,
                "one encoding scratch, even with {connections} sockets"
            );
        }
    }
}
