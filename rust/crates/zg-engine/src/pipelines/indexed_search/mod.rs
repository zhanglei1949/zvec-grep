//! Search over an existing workspace index using FTS and vectors.

pub(crate) mod context;
mod format_filter;
mod path_filter;
pub(in crate::pipelines) mod pipeline;
pub(in crate::pipelines) mod retained;
pub(crate) mod service;
pub(crate) mod storage;

pub(crate) mod writer;
