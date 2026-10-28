//! The owner's OAuth accounts (oauth.md, sections 6.1–6.7).
//! Keeps bounded records, held grants, issuer exchanges, redirect listeners and deadlines;
//! names configured accounts by index and `add` admits further owner numbers. `down` accepts
//! owner requests and keeper terminals, `up` routes socket events, `filed` routes
//! private file terminals, `fire` advances
//! one child or deadline, and `next_deadline` lets the owner schedule it.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod accounts;
mod boundary;
mod component;
mod exchange;
mod keeper;
mod limits;
mod listener;
mod redirect;
mod route;

pub use accounts::{Account, Endpoint, Keeper, Origin, Transport};
pub use boundary::{Asked, Ends, Event, Failure, Keeping, Locked, Place, Refusal, Request, Unloaded};
pub use component::{Component, FileLower, Lower, LowerEvent, Unusable};
pub use limits::{Limits, valid, worst_case};

/// The largest return location for one sign-in (oauth.md, section 6.5).
pub const RETURN_BYTES: usize = 2048;

/// The `Redirected`s one sign-in refuses for their state before it refuses every later one as
/// full (oauth.md, section 6.5).
pub const STATE_REFUSALS: u32 = 4;

#[cfg(test)]
mod tests;
