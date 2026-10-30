//! Shared storage primitives. No workspace schema, publication policy, model
//! runtime, process initialization, or knowledge lifecycle is owned here.
#![allow(clippy::missing_errors_doc)]

pub mod filesystem;
mod hash;
#[cfg(feature = "zvec")]
pub mod zvec;

pub use hash::{sha256_hex, sha256_hex_parts};
