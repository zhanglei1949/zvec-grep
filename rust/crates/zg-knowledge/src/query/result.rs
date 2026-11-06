//! Business query records. Native rows and construction auxiliaries stay private.
use super::{
    BudgetUsage, ByteSpan, EntityKind, EvidenceRead, EvidenceRef, Locator, NodeRef, RelationKind,
    ViewStamp,
};
use serde::{Deserialize, Serialize};

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
    pub status: crate::relations::ResolutionStatus,
    pub reason: String,
    pub locator: crate::relations::Lookup,
    pub evidence: EvidenceRef,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RelationRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention: Option<crate::semantic::Mention>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_link: Option<crate::semantic::EpisodeLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_object: Option<crate::semantic::ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_object: Option<crate::semantic::ObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<crate::semantic::Fact>,
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
    pub domain: super::QueryDomain,
    pub ready: bool,
    pub note: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ExploreResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relation_types: Vec<super::AvailableRelationType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<super::SemanticReadResult>,
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
