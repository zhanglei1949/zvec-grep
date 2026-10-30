//! Workspace discovery, extraction, embedding, and storage orchestration.

#[cfg(feature = "knowledge-graph-graphdb")]
mod graph;
mod input_budget;
mod model_progress;
pub(crate) mod pipeline;
pub(crate) mod service;
pub(crate) mod storage;
