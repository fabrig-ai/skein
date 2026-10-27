//! Portable parts of a world in a browser (playground.md).
#![forbid(unsafe_code)]

pub mod replay;
pub mod visitor;

pub mod native;
pub mod slices;
#[cfg(test)]
mod tests;
#[cfg(target_arch = "wasm32")]
pub mod worker;
