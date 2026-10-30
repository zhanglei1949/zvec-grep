//! Read-only code exploration. Available with or without the optional backend.
#![allow(clippy::missing_errors_doc)]
pub use super::context::graph::{
    Direction, EntityKind, Locator, NodeRef, QueryDomain, RelationKind, SourceFilter, TraversalSpec,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "by", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReferenceSelector {
    Id {
        reference_id: String,
        view: ViewStamp,
    },
    Owner {
        owner: Locator,
    },
    Source {
        source_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExploreOperation {
    Locate {
        target: Locator,
    },
    Neighbors {
        anchor: Locator,
        relations: Vec<RelationKind>,
        direction: Direction,
    },
    Expand {
        anchor: Locator,
        traversal: TraversalSpec,
    },
    ShortestPath {
        start: Locator,
        target: Locator,
        traversal: TraversalSpec,
    },
    References {
        selector: ReferenceSelector,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExploreRequest {
    pub domain: QueryDomain,
    pub operation: ExploreOperation,
    #[serde(default)]
    pub boundary: SourceFilter,
    #[serde(default)]
    pub result_filter: SourceFilter,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewStamp {
    pub workspace_id: String,
    pub generation: String,
    pub text_revision: u64,
    pub graph: Option<GraphStamp>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphStamp {
    pub build_id: String,
    pub schema: String,
    pub rules: String,
    pub configuration: String,
    pub catalog_revision: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ByteSpan {
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub source_id: String,
    pub raw_digest: String,
    pub decoded_digest: String,
    pub span: ByteSpan,
    pub reference_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvidenceRead {
    Verified {
        reference: EvidenceRef,
        content: String,
    },
    Unavailable {
        reference: EvidenceRef,
        reason: String,
    },
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetLimits {
    pub nodes: u64,
    pub edges: u64,
    pub candidates: u64,
    pub mapping_units: u64,
    pub output_bytes: u64,
    pub filter_bytes: u64,
}
impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            nodes: 2000,
            edges: 10_000,
            candidates: 5000,
            mapping_units: 5000,
            output_bytes: 65_536,
            filter_bytes: 262_144,
        }
    }
}
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema,
)]
pub struct BudgetUsage {
    pub nodes: u64,
    pub edges: u64,
    pub candidates: u64,
    pub mapping_units: u64,
    pub output_bytes: u64,
    pub filter_bytes: u64,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionOptions {
    pub request_timeout_ms: u64,
    pub graph_timeout_ms: u64,
    pub budget: BudgetLimits,
}
impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            request_timeout_ms: 30_000,
            graph_timeout_ms: 5000,
            budget: BudgetLimits::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Lookup {
    Name { parts: Vec<String> },
    Import { path: Vec<String> },
    Document { destination: String },
    Dynamic,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    Resolved,
    Ambiguous,
    Unresolved,
    External,
    Unsupported,
    OutsideWorkspace,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct EntityRecord {
    pub id: String,
    pub source_id: String,
    pub kind: EntityKind,
    pub name: String,
    pub qualified_name: String,
    pub span: ByteSpan,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ReferenceRecord {
    pub id: String,
    pub owner: String,
    pub target: Option<String>,
    pub kind: RelationKind,
    pub status: ResolutionStatus,
    pub reason: String,
    pub locator: Lookup,
    pub evidence: EvidenceRef,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RelationRecord {
    pub id: String,
    pub source: String,
    pub target: String,
    pub kind: RelationKind,
    pub reference: Option<ReferenceRecord>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct GraphNode {
    pub node: NodeRef,
    pub source_id: String,
    pub kind: EntityKind,
    pub name: String,
    pub qualified_name: String,
    pub span: ByteSpan,
    pub distance: Option<u32>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocateStatus {
    Unique,
    Ambiguous,
    NotFound,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct LocateResult {
    pub locator: Locator,
    pub status: LocateStatus,
    pub candidates: Vec<GraphNode>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct GraphPath {
    pub nodes: Vec<NodeRef>,
    pub relations: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    Partial,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct QueryExecution {
    pub status: Completion,
    pub stop_reason: Option<String>,
    pub usage: BudgetUsage,
    pub qualification_micros: u64,
    pub query_micros: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct CoverageDiagnostic {
    pub domain: QueryDomain,
    pub ready: bool,
    pub note: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ExploreResult {
    pub view: ViewStamp,
    pub locations: Vec<LocateResult>,
    pub nodes: Vec<GraphNode>,
    pub relations: Vec<RelationRecord>,
    pub paths: Vec<GraphPath>,
    pub references: Vec<ReferenceRecord>,
    pub evidence: Vec<EvidenceRead>,
    pub coverage: CoverageDiagnostic,
    pub execution: QueryExecution,
}

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
        self.execution.validate()
    }
}

impl ExecutionOptions {
    pub fn validate(&self) -> crate::EngineResult<()> {
        let bad = crate::EngineError::invalid_argument;
        let ceiling = ExecutionOptions::default();
        if self.request_timeout_ms == 0
            || self.graph_timeout_ms == 0
            || self.request_timeout_ms > ceiling.request_timeout_ms
            || self.graph_timeout_ms > ceiling.graph_timeout_ms
        {
            return Err(bad("execution deadline exceeds service limit"));
        }
        let b = self.budget;
        let c = ceiling.budget;
        if [
            b.nodes,
            b.edges,
            b.candidates,
            b.mapping_units,
            b.output_bytes,
            b.filter_bytes,
        ]
        .into_iter()
        .zip([
            c.nodes,
            c.edges,
            c.candidates,
            c.mapping_units,
            c.output_bytes,
            c.filter_bytes,
        ])
        .any(|(n, max)| n == 0 || n > max)
        {
            return Err(bad("execution budget exceeds service limit"));
        }
        Ok(())
    }
}

impl ExploreRequest {
    pub fn validate(&self) -> crate::EngineResult<()> {
        let request = self;
        let bad = crate::EngineError::invalid_argument;
        request.boundary.validate()?;
        request.result_filter.validate()?;
        if request.domain != QueryDomain::Code {
            return Err(crate::EngineError::unsupported(
                "only code explore is available",
            ));
        }
        let kinds = |relations: &[RelationKind]| -> crate::EngineResult<()> {
            if relations.is_empty()
                || relations.len() > 6
                || relations
                    .iter()
                    .enumerate()
                    .any(|(i, k)| relations[..i].contains(k))
            {
                return Err(bad("expected distinct relation kinds"));
            }
            if relations.iter().any(|k| {
                !matches!(
                    k,
                    RelationKind::Contains | RelationKind::Calls | RelationKind::Imports
                )
            }) {
                return Err(crate::EngineError::unsupported(
                    "code explore relation unsupported",
                ));
            }
            Ok(())
        };
        match &request.operation {
            ExploreOperation::Locate { target } => target.validate()?,
            ExploreOperation::Neighbors {
                anchor, relations, ..
            } => {
                anchor.validate()?;
                kinds(relations)?;
            }
            ExploreOperation::Expand { anchor, traversal } => {
                anchor.validate()?;
                traversal.validate()?;
                kinds(&traversal.relations)?;
            }
            ExploreOperation::ShortestPath {
                start,
                target,
                traversal,
            } => {
                start.validate()?;
                target.validate()?;
                traversal.validate()?;
                kinds(&traversal.relations)?;
            }
            ExploreOperation::References { selector } => selector.validate()?,
        }
        Ok(())
    }
}
impl ViewStamp {
    pub fn validate(&self) -> crate::EngineResult<()> {
        let valid = |s: &String| !s.trim().is_empty() && s.len() <= 4096 && !s.contains('\0');
        if ![&self.workspace_id, &self.generation]
            .into_iter()
            .all(valid)
            || self.graph.as_ref().is_some_and(|g| {
                ![
                    &g.build_id,
                    &g.schema,
                    &g.rules,
                    &g.configuration,
                    &g.catalog_revision,
                ]
                .into_iter()
                .all(valid)
            })
        {
            return Err(crate::EngineError::invalid_argument("invalid view stamp"));
        }
        Ok(())
    }
}
impl ReferenceSelector {
    fn validate(&self) -> crate::EngineResult<()> {
        match self {
            Self::Owner { owner } => owner.validate(),
            Self::Source { source_id } => Locator::Position {
                source_id: source_id.clone(),
                byte_offset: 0,
                kind: None,
            }
            .validate(),
            Self::Id { reference_id, view } => {
                if reference_id.trim().is_empty()
                    || reference_id.len() > 4096
                    || reference_id.contains('\0')
                    || view.graph.is_none()
                {
                    return Err(crate::EngineError::invalid_argument(
                        "reference ID and graph view required",
                    ));
                }
                view.validate()
            }
        }
    }
}
