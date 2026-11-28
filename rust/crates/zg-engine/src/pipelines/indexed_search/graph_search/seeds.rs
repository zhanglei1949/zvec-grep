//! Seed selection balancing explicit symbol anchors (Quarry) and conceptual hits (SWE-QA).

use std::collections::{HashMap, HashSet};

use crate::{
    EngineResult,
    api::context::result::MatchedBy,
    domain::{EntityId, EntityMetadata, FileId, FileRecord},
    storage::{ReadView, types::StorageSearchFilter},
};

use super::super::ranking::{QueryFeatures, Roles, Scored};

const MAX_EXPLICIT_ANCHORS: usize = 4;
const MAX_TOTAL_SEEDS: usize = 10;
const MAX_SEEDS_PER_FILE: usize = 3;
const FTS_TOP_SEEDS: usize = 2;
const VECTOR_TOP_SEEDS: usize = 2;
const STATIC_TOP_SEEDS: usize = 9;
const MIN_SEED_STATIC_SCORE: f64 = 0.20;
const RELATIVE_SEED_THRESHOLD: f64 = 0.45;

#[derive(Clone, Debug)]
pub(super) struct GraphSeed {
    pub id: EntityId,
    pub file_id: FileId,
    pub score: f64,
    pub is_anchor: bool,
    pub is_test: bool,
}

/// Select explicit unique symbol anchors `H` and multi-channel representative seeds from `C0`.
pub(super) fn select_seeds(
    view: &ReadView<'_>,
    scored: &[Scored],
    query: &QueryFeatures,
    storage_filter: Option<&StorageSearchFilter>,
) -> EngineResult<(HashSet<EntityId>, Vec<GraphSeed>)> {
    let mut anchors = HashSet::new();
    let mut seeds = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut per_file = HashMap::<FileId, usize>::new();

    // 1. Explicit symbol channel: resolve unique non-declaration definitions for explicit names.
    for raw_name in query.explicit_names() {
        if anchors.len() >= MAX_EXPLICIT_ANCHORS {
            break;
        }
        let Some((id, file)) = resolve_unique_explicit_symbol(view, raw_name, storage_filter)?
        else {
            continue;
        };
        if seen_ids.insert(id.clone()) {
            let is_test = Roles::from_path(&file.relative_path().display().to_string()).tests;
            anchors.insert(id.clone());
            *per_file.entry(file.id()).or_default() += 1;
            seeds.push(GraphSeed {
                id,
                file_id: file.id(),
                score: 1.0,
                is_anchor: true,
                is_test,
            });
        }
    }

    if scored.is_empty() {
        return Ok((anchors, seeds));
    }

    // Verify file revisions of C0 candidates against the active ReadView snapshot.
    let c0_file_ids: Vec<FileId> = scored
        .iter()
        .map(|s| s.hit.file.id())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let live_files = view.fetch_files(&c0_file_ids)?;

    let is_consistent = |item: &Scored| -> bool {
        live_files
            .get(&item.hit.file.id())
            .is_some_and(|live| live.index_status().is_indexed() && live == &item.hit.file)
    };

    let s_max = scored
        .iter()
        .filter(|item| is_consistent(item))
        .map(|item| item.static_score)
        .fold(0.0_f64, f64::max);
    if s_max <= 0.0 {
        return Ok((anchors, seeds));
    }
    let threshold = MIN_SEED_STATIC_SCORE.max(RELATIVE_SEED_THRESHOLD * s_max);

    // Build indices sorted by static_score descending (deterministic tie-break).
    let mut by_static: Vec<usize> = (0..scored.len())
        .filter(|&i| is_consistent(&scored[i]))
        .collect();
    by_static.sort_by(|&a, &b| {
        scored[b]
            .static_score
            .total_cmp(&scored[a].static_score)
            .then_with(|| scored[a].hit.rank.cmp(&scored[b].hit.rank))
            .then_with(|| {
                scored[a]
                    .hit
                    .entity
                    .id()
                    .as_str()
                    .cmp(scored[b].hit.entity.id().as_str())
            })
    });

    // Helper to add a conceptual seed from `C0` respecting caps and thresholds.
    let mut try_add_seed = |index: usize| {
        if seeds.len() >= MAX_TOTAL_SEEDS {
            return;
        }
        let item = &scored[index];
        if item.static_score < threshold {
            return;
        }
        let id = item.hit.entity.id().clone();
        if seen_ids.contains(&id) {
            return;
        }
        let file_count = per_file.entry(item.hit.file.id()).or_default();
        if *file_count >= MAX_SEEDS_PER_FILE {
            return;
        }
        seen_ids.insert(id.clone());
        *file_count += 1;
        seeds.push(GraphSeed {
            id,
            file_id: item.hit.file.id(),
            score: (item.static_score / s_max).clamp(0.25, 1.0),
            is_anchor: false,
            is_test: item.roles.tests,
        });
    };

    // 2a. Top-2 FTS representatives.
    let mut fts_added = 0usize;
    for &idx in &by_static {
        if fts_added >= FTS_TOP_SEEDS {
            break;
        }
        if matches!(
            scored[idx].hit.matched_by,
            MatchedBy::Fts | MatchedBy::FtsAndVector
        ) {
            try_add_seed(idx);
            fts_added += 1;
        }
    }

    // 2b. Top-2 Vector representatives.
    let mut vec_added = 0usize;
    for &idx in &by_static {
        if vec_added >= VECTOR_TOP_SEEDS {
            break;
        }
        if matches!(
            scored[idx].hit.matched_by,
            MatchedBy::Vector | MatchedBy::FtsAndVector
        ) {
            try_add_seed(idx);
            vec_added += 1;
        }
    }

    // 2c. Top-6 overall static_score representatives.
    for &idx in by_static.iter().take(STATIC_TOP_SEEDS) {
        try_add_seed(idx);
    }

    // Mark any C0 item that matched an exact qualified symbol as anchor if not already.
    for item in scored {
        if item.symbol_match == "qualified_exact" && anchors.len() < MAX_EXPLICIT_ANCHORS {
            anchors.insert(item.hit.entity.id().clone());
        }
    }

    Ok((anchors, seeds))
}

fn resolve_unique_explicit_symbol(
    view: &ReadView<'_>,
    raw_name: &str,
    storage_filter: Option<&StorageSearchFilter>,
) -> EngineResult<Option<(EntityId, FileRecord)>> {
    let normalized = raw_name.replace('.', "::");
    let (scope, bare_name) = match normalized.rsplit_once("::") {
        Some((scope, name)) if !name.is_empty() => (Some(scope), name),
        _ => (None, raw_name),
    };
    if bare_name.chars().count() < 3 {
        return Ok(None);
    }
    let ids = view.find_symbol_entity_ids(bare_name, storage_filter)?;
    if ids.is_empty() || ids.len() > 32 {
        return Ok(None);
    }
    let metadata_map = view.fetch_code_metadata(&ids)?;
    let mut candidates = Vec::new();
    for id in &ids {
        let Some((file_id, meta)) = metadata_map.get(id) else {
            continue;
        };
        if meta.symbol_name.as_deref() != Some(bare_name) {
            continue;
        }
        if let Some(expected_scope) = scope {
            let actual_scope = meta.scope.as_deref().unwrap_or("").replace('.', "::");
            if actual_scope != expected_scope
                && !actual_scope.ends_with(&format!("::{expected_scope}"))
            {
                continue;
            }
        }
        candidates.push((id.clone(), *file_id, meta.is_declaration));
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let has_definition = candidates.iter().any(|(_, _, is_decl)| !*is_decl);
    if has_definition {
        candidates.retain(|(_, _, is_decl)| !*is_decl);
    }
    if candidates.len() != 1 {
        return Ok(None);
    }
    let (id, file_id, _) = candidates.remove(0);
    let mut files = view.fetch_files(&[file_id])?;
    let Some(file) = files
        .remove(&file_id)
        .filter(|file| file.index_status().is_indexed())
    else {
        return Ok(None);
    };
    // Exclude pure value constants unless qualified so accidental singletons do not lock #1.
    if scope.is_none()
        && metadata_map
            .get(&id)
            .is_some_and(|(_, meta)| meta.symbol_type == Some(crate::domain::SymbolType::Value))
    {
        return Ok(None);
    }
    let _ = EntityMetadata::Code;
    Ok(Some((id, file)))
}
