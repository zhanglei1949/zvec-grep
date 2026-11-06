//! Host workspace options around canonical KG query contracts.
#![allow(clippy::missing_errors_doc)]
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub use zg_knowledge::query::{
    AvailableRelationType, BudgetLimits, BudgetUsage, ByteSpan, Completion, CoverageDiagnostic,
    Direction, EntityKind, EntityRecord, EvidenceRead, EvidenceRef, ExecutionOptions,
    ExploreOperation, ExploreRequest, ExploreResult, GraphNode, GraphPath, GraphStamp,
    LocateResult, LocateStatus, Locator, NodeRef, ObjectRef, QueryDomain, QueryExecution,
    ReferenceRecord, ReferenceSelector, RelationKind, RelationRecord, RelationSelector,
    SemanticObject, SemanticProvenance, SemanticQuery, SemanticReadResult, SemanticValue,
    SourceFilter, TraversalSpec, ViewStamp,
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
    if request.domain == QueryDomain::Semantic
        && matches!(
            request.operation,
            ExploreOperation::Semantic { .. }
                | ExploreOperation::Neighbors { .. }
                | ExploreOperation::RelationTypes { .. }
        )
    {
        let requested = match &request.operation {
            ExploreOperation::Neighbors { relations, .. } => relations.as_slice(),
            _ => &[],
        };
        if requested
            .iter()
            .chain(request.relation_filters.iter().map(|s| &s.edge_type))
            .any(|kind| {
                !matches!(
                    kind,
                    RelationKind::Mentions
                        | RelationKind::SemanticRelation
                        | RelationKind::EpisodeLink
                )
            })
        {
            return Err(crate::EngineError::unsupported(
                "semantic adjacency relation unsupported",
            ));
        }
        return Ok(());
    }
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
    if relations
        .iter()
        .chain(request.relation_filters.iter().map(|s| &s.edge_type))
        .any(|kind| !super::context::graph::CODE_RELATION_KINDS.contains(kind))
    {
        return Err(crate::EngineError::unsupported(
            "code explore relation unsupported",
        ));
    }
    Ok(())
}
