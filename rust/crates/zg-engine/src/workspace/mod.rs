//! Workspace index location, manifest persistence, and process coordination.

pub(crate) mod build;
pub(crate) mod graph_policy;
pub(crate) mod layout;
pub(crate) mod lock;
pub(crate) mod manifest;
pub(crate) mod registry;

/// One format version covers the manifest, storage schema, records and indexing semantics.
/// Version 1 belongs to the released Node.js implementation; incompatible indexes require rebuild.
/// Version 2 is the unreleased Rust format and already folds in the `CommonMark`
/// Markdown segmentation (zg-markdown); no bump was needed because v2 has not
/// shipped. Local v2 indexes built before that change need `zg index --rebuild`
/// to re-segment Markdown.
pub(crate) const CURRENT_INDEX_VERSION: u32 = 2;
