//! Graph additions are separate from normal retrieval items and their quotas.
use crate::api::explore::{
    ByteSpan, EvidenceRef, GraphNode, NodeRef, QueryExecution, RelationRecord, ViewStamp,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphStatus {
    Complete,
    Partial,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphHit {
    pub source_id: String,
    pub entity_id: String,
    pub fragment_id: String,
    pub source_spans: Vec<ByteSpan>,
    pub priority: u32,
    pub query_sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphSeed {
    pub node: NodeRef,
    pub hits: Vec<GraphHit>,
    pub query_sources: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphSkippedHit {
    pub entity_id: String,
    pub fragment_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphSeedExpansion {
    pub seed: NodeRef,
    pub nodes: Vec<String>,
    pub relations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GraphEvidence {
    Retrieved {
        reference: EvidenceRef,
        entity_id: String,
        item_rank: usize,
    },
    Verified {
        reference: EvidenceRef,
        content: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphSupplement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<GraphScope>,
    pub status: GraphStatus,
    pub view: Option<ViewStamp>,
    pub seeds: Vec<GraphSeed>,
    pub skipped: Vec<GraphSkippedHit>,
    pub expansions: Vec<GraphSeedExpansion>,
    pub nodes: Vec<GraphNode>,
    pub relations: Vec<RelationRecord>,
    pub evidence: Vec<GraphEvidence>,
    pub diagnostics: Vec<String>,
    pub execution: Option<QueryExecution>,
}

impl GraphSupplement {
    pub(crate) fn degraded(reason: String) -> Self {
        Self {
            scope: None,
            status: GraphStatus::Degraded,
            view: None,
            seeds: vec![],
            skipped: vec![],
            expansions: vec![],
            nodes: vec![],
            relations: vec![],
            evidence: vec![],
            diagnostics: vec![reason],
            execution: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphScope {
    pub anchor: crate::api::explore::LocateResult,
    pub complete: bool,
    pub exact_filter: bool,
    /// Exact filter membership does not imply exhaustive ANN recall.
    pub recall_exhaustive: bool,
    pub region_count: usize,
    pub fragment_count: usize,
    pub filtered_fragment_count: usize,
    pub empty_reason: Option<String>,
}
