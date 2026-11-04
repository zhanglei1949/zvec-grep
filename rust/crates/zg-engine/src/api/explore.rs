//! Host workspace options around canonical KG query contracts.
#![allow(clippy::missing_errors_doc)]
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use zg_knowledge::query::{
    BudgetLimits, BudgetUsage, ByteSpan, Completion, CoverageDiagnostic, Direction, EntityKind,
    EntityRecord, EvidenceRead, EvidenceRef, ExecutionOptions, ExploreOperation, ExploreRequest,
    ExploreResult, GraphNode, GraphPath, GraphStamp, LocateResult, LocateStatus, Locator, NodeRef,
    ObjectRef, QueryDomain, QueryExecution, ReferenceRecord, ReferenceSelector, RelationKind,
    RelationRecord, SemanticObject, SemanticProvenance, SemanticQuery, SemanticReadResult,
    SemanticValue, SourceFilter, TraversalSpec, ViewStamp,
};
pub use zg_knowledge::relations::{Lookup, ResolutionStatus};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExploreOptions {
    pub root: Option<PathBuf>,
    pub request: ExploreRequest,
    #[serde(default)]
    pub execution: ExecutionOptions,
    pub lock_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub signal: Option<tokio_util::sync::CancellationToken>,
}
impl ExploreOptions {
    #[must_use]
    pub fn new(request: ExploreRequest) -> Self {
        Self {
            root: None,
            request,
            execution: ExecutionOptions::default(),
            lock_timeout_ms: None,
            signal: None,
        }
    }

    pub fn validate(&self) -> crate::EngineResult<()> {
        let request = &self.request;
        request.validate()?;
        self.execution
            .validate_within(ExecutionOptions::default())?;
        ensure_supported(request)
    }
}

fn ensure_supported(request: &ExploreRequest) -> crate::EngineResult<()> {
    if matches!(request.operation, ExploreOperation::Semantic { .. }) {
        return Ok(());
    }
    if request.domain != QueryDomain::Code {
        return Err(crate::EngineError::unsupported(
            "only code explore is available",
        ));
    }
    let relations = match &request.operation {
        ExploreOperation::Neighbors { relations, .. } => relations.as_slice(),
        ExploreOperation::Expand { traversal, .. }
        | ExploreOperation::ShortestPath { traversal, .. } => traversal.relations.as_slice(),
        ExploreOperation::References {
            selector: ReferenceSelector::Id { view, .. },
        } if view.graph.is_none() => {
            return Err(crate::EngineError::invalid_argument(
                "reference ID and graph view required",
            ));
        }
        _ => &[],
    };
    if relations.iter().any(|kind| {
        !matches!(
            kind,
            RelationKind::Contains
                | RelationKind::Calls
                | RelationKind::Imports
                | RelationKind::Extends
                | RelationKind::Implements
        )
    }) {
        return Err(crate::EngineError::unsupported(
            "code explore relation unsupported",
        ));
    }
    Ok(())
}
