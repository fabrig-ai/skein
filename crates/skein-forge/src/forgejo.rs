//! The pinned Forgejo's documents (forge.md, sections 2, 3 and 9).
pub mod client;
pub mod credential;
pub mod decode;
pub mod delivery;
pub mod document;
pub mod header;
pub mod id;
pub mod limits;
pub mod path;
pub mod pin;
pub mod request;
pub mod server;
#[cfg(test)]
pub(crate) mod tests;
pub mod time;

pub mod failure;

pub mod plans;

pub mod zip;
