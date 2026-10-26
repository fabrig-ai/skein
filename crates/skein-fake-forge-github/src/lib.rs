//! GitHub's HTTP over the fake forge (fake-forge.md, section 10):
//! App authentication, installations, reads, writes and signed job logs over the shared domain.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;
mod answers;
mod auth;
mod comment_edit;
mod enqueue;
mod git;
pub use git::{GitCall, Minted};
mod mutations;
pub use skein_fake_forge_protocol::Faults;
pub mod app;
pub mod delivery;
pub mod installations;
pub mod keys;
pub mod logs;
pub mod manifest;
pub mod pages;
pub mod rates;
mod reads;
mod routes;
pub mod server;
pub mod service;
pub use server::{
    Event, MAX_DOWN, MAX_UP, Server, close, closed, down, resume, resume_with_domain, start, up, up_with_domain,
};
pub use service::{Config, Limits, Problem, Service, worst_case};
mod texts;
mod writes;

#[cfg(test)]
mod tests;
