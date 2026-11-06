//! Workspace index location, manifest persistence, and process coordination.

pub(crate) mod build;
pub(crate) mod graph_policy;
pub(crate) mod layout;
pub(crate) mod lock;
pub(crate) mod manifest;

/// Compatibility version of the workspace index contents.
pub(crate) const CURRENT_INDEX_VERSION: u32 = 2;

pub(crate) mod publication;
