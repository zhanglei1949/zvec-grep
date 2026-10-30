//! Read-only query contracts. No database, model, scheduler or publication state.
mod execution;
mod explore;
mod reader;
mod request;
mod result;
mod seeds;
mod view;
pub use explore::{
    AugmentResult, ScopeRegion, ScopeResolution, ScopeTextEntity, ScopeTextFragment,
    ScopeTextProvider, ScopeUnit, SeedExpansion, augment, explore, resolve_scope,
};
pub use reader::*;
pub use result::*;
pub use seeds::*;

pub use execution::{BudgetLimits, BudgetUsage, ExecutionOptions, RequestContext};
pub use request::*;
pub use view::*;

pub type QueryResult<T> = std::result::Result<T, QueryError>;

/// Query errors do not change the graph construction error contract.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{kind:?} at {stage}: {reason}")]
pub struct QueryError {
    pub kind: QueryErrorKind,
    pub stage: &'static str,
    pub reason: &'static str,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryErrorKind {
    InvalidInput,
    ViewMismatch,
    Unsupported,
    GraphUnavailable,
    ResourceExhausted,
    RequestDeadline,
    StageDeadline,
    Cancelled,
    RecoveryRequired,
    Internal,
}

impl QueryError {
    #[must_use]
    pub const fn new(kind: QueryErrorKind, stage: &'static str, reason: &'static str) -> Self {
        Self {
            kind,
            stage,
            reason,
            detail: None,
        }
    }
    #[must_use]
    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }
    pub(crate) const fn invalid(reason: &'static str) -> Self {
        Self::new(QueryErrorKind::InvalidInput, "validate", reason)
    }
}

fn nonempty(value: &str) -> QueryResult<()> {
    if value.trim().is_empty() || value.len() > 4096 || value.contains('\0') {
        return Err(QueryError::invalid(
            "expected a non-empty bounded identifier",
        ));
    }
    Ok(())
}

fn source_id(value: &str) -> QueryResult<()> {
    nonempty(value)?;
    crate::relations::validate_path(value)
        .map_err(|_| QueryError::invalid("expected normalized workspace-relative source"))
}
