//! A checked heap bound for one channel (channel.md, section 10).
//! It prices the owning schema, queue slots and bytes, an in-progress body,
//! bounded control reservations, terms, and the caller's event/lower queues.
//! It never counts a transport's intake or buffers as channel ownership.
use core::mem::size_of;
use skein_lib::{List, Queue};

use crate::machine::Queued;
use crate::{Direction, Event, Kind, Limits, Lower, MAX_DOWN, MAX_UP, Machine, Role, Schema, Term, Version};

/// Whether these limits are representable for this schema and role:
/// the schema accepts them, and every frame and queue they size fits
/// (programming-model.md, 6.3).
#[must_use]
pub fn valid(schema: &Schema, role: Role, limits: &Limits) -> bool {
    schema.check(role, limits).is_ok() && capacity_bound(limits).is_some()
}

fn capacity_bound(limits: &Limits) -> Option<u32> {
    limits.output_frames.checked_add(3)?;
    let terms = limits.kinds.checked_mul(6)?.checked_add(4)?;
    let open = limits.credential.checked_add(14)?;
    open.max(terms).max(262).checked_add(8)
}

/// This role's channel-owned heap: sent queue bytes and received body capacity.
#[must_use]
pub fn worst_case(schema: &Schema, role: Role, limits: &Limits) -> Option<u64> {
    if !valid(schema, role, limits) {
        return None;
    }
    let queue_capacity = limits.output_frames.checked_add(3)?;
    let mut schema_heap = List::<Version>::worst_case(schema.versions.capacity())?;
    let received = match role {
        Role::Initiator => Direction::FromResponder,
        Role::Responder => Direction::FromInitiator,
    };
    let mut largest_body = 0_u32;
    for version in &schema.versions {
        schema_heap = schema_heap.checked_add(List::<Kind>::worst_case(version.kinds.capacity())?)?;
        for kind in &version.kinds {
            if kind.direction == received {
                largest_body = largest_body.max(kind.largest);
            }
        }
    }
    let terms_body = limits.kinds.checked_mul(6)?.checked_add(4)?;
    let open_body = limits.credential.checked_add(14)?;
    let control_body = open_body.max(terms_body).max(262);
    let control_frame = u64::from(control_body.checked_add(8)?);
    let control_reserve = control_frame.checked_mul(3)?;
    let body = u64::from(largest_body.max(control_body));
    let terms = List::<Term>::worst_case(limits.kinds)?.checked_mul(2)?;
    let queues = Queue::<Queued>::worst_case(queue_capacity)?
        .checked_add(Queue::<Event>::worst_case(MAX_UP)?)?
        .checked_add(Queue::<Lower>::worst_case(MAX_DOWN)?)?;
    let state = u64::try_from(size_of::<Machine>()).ok()?;
    schema_heap
        .checked_add(queues)?
        .checked_add(u64::from(limits.output_bytes))?
        .checked_add(control_reserve)?
        .checked_add(body)?
        .checked_add(u64::from(limits.chunk.max(8)))?
        .checked_add(u64::from(limits.credential))?
        .checked_add(terms)?
        .checked_add(state)
}
