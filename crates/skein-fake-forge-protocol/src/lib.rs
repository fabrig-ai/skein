//! Forgejo's HTTP vocabulary over the fake forge (fake-forge.md, section 4).
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
pub mod answers;
pub mod delivery;
use alloc::boxed::Box;
use skein_lib::{List, Secret};
pub mod errors;
pub mod git;
pub mod observations;
pub mod response;
pub mod routes;
mod server;
mod service;
pub mod setup;
pub mod tally;
pub use server::{
    Event, MAX_DOWN, MAX_UP, Server, close, closed, down, resume, resume_with_domain, start, up, up_with_domain,
};
pub use service::{Error, Service, valid};
pub use tally::{Route, Tally, route};
/// Caps owned by the protocol layer, in addition to the fake domain's.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub calls: u32,
    pub connections: u32,
    pub http: skein_http::server::Limits,
    pub documents: skein_forge::forgejo::limits::Limits,
    pub raw_bytes: u32,
}
/// The repository's id, path and protected branch from the scenario.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Repository {
    pub id: u32,
    pub full: Box<[u8]>,
    pub protected: Option<Box<[u8]>>,
}
/// Per-request chances in a thousand, drawn before and after the domain call.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Faults {
    pub reset: u32,
    pub drop: u32,
    pub server_error: u32,
}
/// Scenario tables; tokens stay solely on this side of the domain boundary.
#[derive(Debug)]
pub struct Config {
    pub hooks: Option<delivery::Endpoint>,
    pub base: Box<[u8]>,
    pub tokens: List<(Secret, u64)>,
    pub users: List<(u64, Box<[u8]>)>,
    pub repositories: List<Repository>,
    pub max_response_items: u32,
    pub default_paging_num: u32,
    pub faults: Faults,
}
#[cfg(test)]
mod tests;

/// The forge lent for one hosted pass, including direct object-store calls.
#[derive(Debug)]
pub struct Hosted<'a> {
    pub domain: &'a mut skein_fake_forge_domain::Domain,
    pub config: &'a skein_fake_forge_domain::Config,
}
