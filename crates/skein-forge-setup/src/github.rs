//! GitHub's owner-provided values and plans (forge-setup.md, 5).
pub mod accept;
pub(crate) mod app;
pub mod checks;
pub mod convert;
pub mod given;
pub(crate) mod hook;
pub mod install;
pub(crate) mod installations;
mod key;
pub mod limits;
pub mod manifest;
pub use checks::Lacking;
pub mod reconcile;

pub(crate) mod memory;

pub mod paste;
mod write;
