//! Source-grounded graph facts, resolution and optional graphdb persistence.
//! The engine owns source ingestion, models, text/vector storage and publication.
#![allow(clippy::missing_errors_doc)] // Shared error contracts are documented in README.md.
pub mod error;
pub mod processing;
pub mod query;
pub mod relations;
pub mod semantic;

#[cfg(feature = "graphdb")]
pub mod graph;

#[cfg(feature = "graphdb")]
pub mod service;
#[cfg(feature = "graphdb")]
pub use service::{GraphHandle, KgService};

pub use error::{Error, Result};
