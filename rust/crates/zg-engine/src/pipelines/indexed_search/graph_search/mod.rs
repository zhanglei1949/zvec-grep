//! Bounded, pattern-constrained code-graph expansion and two-track fusion ranking.

pub(super) mod expand;
pub(super) mod score;
pub(super) mod seeds;
#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use crate::{
    EngineError, EngineResult,
    api::context::{
        options::{GraphRankingPolicy, GraphSearchMode, GraphSearchOptions, QueryFilter},
        result::{GraphSearchDiagnostics, RelatedGraphItem, SearchHitTrace},
    },
    domain::{Direction, EdgeProvenance, EntityId, GraphEdgeKind},
    storage::types::{StorageSearchFilter, StoredEntity},
};

use super::{
    pipeline::SearchHit,
    ranking::{Policy, QueryFeatures, Roles, Scored},
    stage_trace::StageTraceCollector,
    storage::SearchStorage,
};

/// One directed hop recorded during pattern-constrained expansion.
#[derive(Clone, Debug)]
pub(super) struct ExpandedHop {
    pub kind: GraphEdgeKind,
    pub direction: Direction,
    pub from_id: EntityId,
    pub to_id: EntityId,
    pub call_line: Option<usize>,
    pub provenance: EdgeProvenance,
    pub branching: usize,
}

/// One candidate path from a seed to a target node.
#[derive(Clone, Debug)]
struct CandidatePath {
    pub seed_file_id: crate::domain::FileId,
    pub seed_score: f64,
    pub seed_is_anchor: bool,
    pub pattern: &'static str,
    pub hops: Vec<ExpandedHop>,
}

/// Expanded graph evidence collected inside a single storage snapshot.
#[derive(Default)]
struct GraphExpansion {
    pub anchors: HashSet<EntityId>,
    pub seeds: Vec<seeds::GraphSeed>,
    pub paths_by_target: HashMap<EntityId, Vec<CandidatePath>>,
    /// Direct P1/P2/P3 adjacency pairs `(a, b)` for same-file decay relief.
    pub linked_pairs: HashSet<(EntityId, EntityId)>,
    /// In-pool candidates sharing an enclosing container with a seed (`P4`).
    pub p4_siblings: HashSet<EntityId>,
    /// Hydrated out-of-pool entities that passed visibility and `QueryFilter`.
    pub out_of_pool_entities: HashMap<EntityId, StoredEntity>,
    /// Display labels `(symbol_name, relative_path)` for nodes appearing in `via:` chains.
    pub node_labels: HashMap<EntityId, (String, std::path::PathBuf)>,
    pub truncated_by_budget: bool,
}

/// Run graph expansion and two-track selection for the visible code slots.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_graph_ranking(
    scored: Vec<Scored>,
    query: &QueryFeatures,
    code_ranks: &[usize],
    policy: Policy,
    filter: &QueryFilter,
    storage_filter: Option<&StorageSearchFilter>,
    workspace_root: &Path,
    storage: &dyn SearchStorage,
    options: &GraphSearchOptions,
    observation: Option<&mut StageTraceCollector>,
) -> EngineResult<Option<(Vec<SearchHit>, Vec<RelatedGraphItem>, GraphSearchDiagnostics)>> {
    if options.mode == GraphSearchMode::Off || scored.is_empty() || code_ranks.is_empty() {
        return Ok(None);
    }

    let overrides = scored[0].overrides;
    let in_pool_ids: HashSet<EntityId> = scored.iter().map(|s| s.hit.entity.id().clone()).collect();
    let in_pool_files: HashMap<EntityId, crate::domain::FileRecord> = scored
        .iter()
        .map(|s| (s.hit.entity.id().clone(), s.hit.file.clone()))
        .collect();

    let mut visitor = expand::GraphExpansionVisitor::new(
        &scored,
        &in_pool_ids,
        &in_pool_files,
        query,
        overrides,
        filter,
        storage_filter,
        workspace_root,
        options,
    );

    let graph_available = storage.visit_graph(&mut visitor)?;
    if !graph_available {
        if options.mode == GraphSearchMode::On {
            return Err(EngineError::unsupported("graph support is disabled")
                .with_help("Rebuild the index with `zg --index --rebuild --graph`."));
        }
        return Ok(None);
    }

    let expansion = visitor.into_expansion();
    let policy_name = match options.policy {
        GraphRankingPolicy::PatternTwoTrack => "pattern_two_track",
        GraphRankingPolicy::InPoolRerankOnly => "in_pool_rerank_only",
        GraphRankingPolicy::LocalRwr => "local_rwr",
    };

    let (selected, related, diag) = score::select_two_track(
        scored,
        expansion,
        query,
        code_ranks,
        policy,
        options.policy,
        policy_name,
        observation,
    );
    Ok(Some((selected, related, diag)))
}

/// Extract a concise human-readable symbol label for a stored entity.
pub(super) fn display_symbol_name(entity: &crate::domain::Entity) -> String {
    if let Some(crate::domain::EntityMetadata::Code(code)) = entity.metadata() {
        if let Some(name) = code.symbol_name.as_deref().filter(|s| !s.is_empty()) {
            return match code.scope.as_deref().filter(|s| !s.is_empty()) {
                Some(scope) => format!("{scope}.{name}"),
                None => name.to_owned(),
            };
        }
    }
    entity.id().as_str().to_owned()
}

/// True if a `SearchHit` is a high-confidence direct hit eligible for Top-3 prefix protection.
pub(super) fn is_high_confidence_direct_hit(item: &Scored, anchors: &HashSet<EntityId>) -> bool {
    item.base >= 0.80 || anchors.contains(item.hit.entity.id())
}

/// Build an empty trace for a pure-graph backfilled hit when `--trace` is enabled.
pub(super) fn backfilled_trace(rank: usize, cutoff_rank: usize) -> SearchHitTrace {
    SearchHitTrace {
        recall: Vec::new(),
        fusion: crate::api::context::result::SearchFusionTrace {
            rank: 0,
            score: 0.0,
            forced: false,
        },
        ranking: None,
        final_selection: crate::api::context::result::SearchFinalTrace {
            returned_by_limit: rank <= cutoff_rank,
            cutoff_rank,
        },
    }
}

/// Returns whether a candidate file's path is a test file when test intent is not active.
pub(super) fn blocked_by_test_barrier(path: &Path, overrides: Roles, seed_is_test: bool) -> bool {
    if overrides.tests || seed_is_test {
        return false;
    }
    Roles::from_path(&path.display().to_string()).tests
}
