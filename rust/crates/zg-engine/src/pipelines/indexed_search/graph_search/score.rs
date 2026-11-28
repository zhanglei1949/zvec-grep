//! Structural scoring (Path-Decay and Local RWR ablation), containment deduplication,
//! graph-linked same-file decay relief, and two-track guardrailed selection.

use std::collections::{HashMap, HashSet};

use crate::{
    api::context::{
        options::GraphRankingPolicy,
        result::{
            ContentRange, GraphEvidence, GraphHop, GraphSearchDiagnostics, MatchedBy,
            RelatedGraphItem, SearchRankingTrace, SourceFocus, SourceFocusKind,
        },
    },
    domain::{Direction, EntityId, EntityMetadata, FileId, Range},
};

use super::{
    CandidatePath, GraphExpansion, backfilled_trace, expand::provenance_weight,
    is_high_confidence_direct_hit,
};
use crate::pipelines::indexed_search::{
    pipeline::SearchHit,
    ranking::{FILE_DECAY, Policy, QueryFeatures, Scored, compare},
    stage_trace::StageTraceCollector,
};

const HOP_DECAY: f64 = 0.65;
const IN_POOL_GRAPH_WEIGHT: f64 = 0.18;
const P4_SIBLING_BONUS: f64 = 0.04;
const GRAPH_LINKED_FILE_DECAY: f64 = 0.15;
const MAX_RELIEF_FILE_COUNT: u32 = 3;
const OUT_OF_POOL_GATE: f64 = 0.35;
const OUT_OF_POOL_RELEVANCE_GATE: f64 = 0.15;
const OUT_OF_POOL_RELEVANCE_WEIGHT: f64 = 0.45;
const RELATED_GRAPH_GATE: f64 = 0.38;
const RELATED_SAME_FILE_HIGH_GATE: f64 = 0.65;
const MAX_RELATED_ITEMS: usize = 3;
const MAX_RELATED_PER_FILE: usize = 2;
const TAIL_REPLACEABLE_THRESHOLD: f64 = 0.42;
const TAIL_RELATIVE_REPLACEABLE: f64 = 0.45;
const BACKFILL_ADVANTAGE_FACTOR: f64 = 0.85;
const RWR_RESTART_ALPHA: f64 = 0.30;
const RWR_ITERATIONS: usize = 15;

/// Path-decay structural score for a single candidate path.
pub(super) fn path_score(path: &CandidatePath) -> f64 {
    if path.hops.is_empty() {
        return 0.0;
    }
    let hop_exp = i32::try_from(path.hops.len().saturating_sub(1)).unwrap_or(1);
    let mut prov_product = 1.0_f64;
    let mut branch_product = 1.0_f64;
    for hop in &path.hops {
        prov_product *= provenance_weight(hop.provenance);
        let b = f64::from(u32::try_from(hop.branching.max(1)).unwrap_or(u32::MAX));
        branch_product *= b;
    }
    path.seed_score * HOP_DECAY.powi(hop_exp) * prov_product / branch_product.sqrt()
}

/// Aggregate per-target structural score `G(v)` and strongest explanatory path.
fn aggregate_path_scores(
    paths_by_target: &HashMap<EntityId, Vec<CandidatePath>>,
) -> (HashMap<EntityId, f64>, HashMap<EntityId, CandidatePath>) {
    let mut scores = HashMap::with_capacity(paths_by_target.len());
    let mut best_paths = HashMap::with_capacity(paths_by_target.len());

    for (target, paths) in paths_by_target {
        // Deduplicate multiple paths from the same seed file: take strongest per seed file.
        let mut by_seed_file = HashMap::<FileId, (f64, &CandidatePath)>::new();
        let mut overall_best: Option<(f64, &CandidatePath)> = None;

        for path in paths {
            let s = path_score(path);
            if overall_best.is_none_or(|(best_s, _)| s > best_s) {
                overall_best = Some((s, path));
            }
            let entry = by_seed_file.entry(path.seed_file_id).or_insert((s, path));
            if s > entry.0 {
                *entry = (s, path);
            }
        }

        let mut file_scores: Vec<f64> = by_seed_file.into_values().map(|(s, _)| s).collect();
        file_scores.sort_by(|a, b| b.total_cmp(a));

        let g1 = file_scores.first().copied().unwrap_or(0.0);
        let g2 = file_scores.get(1).copied().unwrap_or(0.0);
        let g3 = file_scores.get(2).copied().unwrap_or(0.0);
        let g = (g1 + 0.50 * g2 + 0.25 * g3).min(1.0);

        if g > 0.0 {
            scores.insert(target.clone(), g);
            if let Some((_, best_path)) = overall_best {
                best_paths.insert(target.clone(), best_path.clone());
            }
        }
    }
    (scores, best_paths)
}

/// Local Personalized PageRank / Random Walk with Restart ablation over the expanded subgraph.
fn compute_local_rwr_scores(expansion: &GraphExpansion) -> HashMap<EntityId, f64> {
    if expansion.seeds.is_empty() || expansion.paths_by_target.is_empty() {
        return HashMap::new();
    }
    let mut nodes: Vec<EntityId> = Vec::new();
    let mut index_of: HashMap<EntityId, usize> = HashMap::new();
    let mut ensure_node = |id: &EntityId| -> usize {
        if let Some(&idx) = index_of.get(id) {
            idx
        } else {
            let idx = nodes.len();
            nodes.push(id.clone());
            index_of.insert(id.clone(), idx);
            idx
        }
    };

    for seed in &expansion.seeds {
        ensure_node(&seed.id);
    }
    let mut raw_edges: Vec<(usize, usize, f64)> = Vec::new();
    for paths in expansion.paths_by_target.values() {
        for path in paths {
            for hop in &path.hops {
                let u = ensure_node(&hop.from_id);
                let v = ensure_node(&hop.to_id);
                raw_edges.push((u, v, provenance_weight(hop.provenance)));
            }
        }
    }
    drop(ensure_node);
    let n = nodes.len();
    if n == 0 {
        return HashMap::new();
    }
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (u, v, w) in raw_edges {
        adj[u].push((v, w));
    }

    let mut restart = vec![0.0_f64; n];
    let mut seed_sum = 0.0_f64;
    for seed in &expansion.seeds {
        if let Some(&idx) = index_of.get(&seed.id) {
            restart[idx] += seed.score;
            seed_sum += seed.score;
        }
    }
    if seed_sum <= 0.0 {
        return HashMap::new();
    }
    for r in &mut restart {
        *r /= seed_sum;
    }

    let mut state = restart.clone();
    let mut next = vec![0.0_f64; n];
    for _ in 0..RWR_ITERATIONS {
        for slot in &mut next {
            *slot = 0.0;
        }
        for (u, neighbors) in adj.iter().enumerate() {
            let mass = state[u];
            if mass == 0.0 {
                continue;
            }
            let total_w: f64 = neighbors.iter().map(|(_, w)| *w).sum();
            if total_w > 0.0 {
                for &(v, w) in neighbors {
                    next[v] += (1.0 - RWR_RESTART_ALPHA) * mass * (w / total_w);
                }
            } else {
                // Dangling node teleports back to restart distribution.
                for (i, &r) in restart.iter().enumerate() {
                    next[i] += (1.0 - RWR_RESTART_ALPHA) * mass * r;
                }
            }
        }
        for (i, &r) in restart.iter().enumerate() {
            next[i] += RWR_RESTART_ALPHA * r;
        }
        std::mem::swap(&mut state, &mut next);
    }

    let seed_set: HashSet<EntityId> = expansion.seeds.iter().map(|s| s.id.clone()).collect();
    let max_non_seed = nodes
        .iter()
        .enumerate()
        .filter(|(_, id)| !seed_set.contains(*id))
        .map(|(i, _)| state[i])
        .fold(0.0_f64, f64::max);
    if max_non_seed <= 0.0 {
        return HashMap::new();
    }
    nodes
        .into_iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let normalized = (state[i] / max_non_seed).min(1.0);
            (normalized > 0.0 && expansion.paths_by_target.contains_key(&id))
                .then_some((id, normalized))
        })
        .collect()
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn select_two_track(
    mut scored: Vec<Scored>,
    mut expansion: GraphExpansion,
    query: &QueryFeatures,
    code_ranks: &[usize],
    policy: Policy,
    graph_policy: GraphRankingPolicy,
    policy_name: &str,
    mut observation: Option<&mut StageTraceCollector>,
) -> (Vec<SearchHit>, Vec<RelatedGraphItem>, GraphSearchDiagnostics) {
    let (path_scores, best_paths) = aggregate_path_scores(&expansion.paths_by_target);
    let structural_scores = match graph_policy {
        GraphRankingPolicy::LocalRwr => compute_local_rwr_scores(&expansion),
        GraphRankingPolicy::PatternTwoTrack | GraphRankingPolicy::InPoolRerankOnly => path_scores,
    };

    if let Some(obs) = observation.as_deref_mut() {
        obs.graph_expanded(expansion.out_of_pool_entities.len());
    }

    // Identify the #1 high-confidence direct hit and explicit anchors for prefix protection.
    let mut static_order: Vec<usize> = (0..scored.len()).collect();
    static_order.sort_by(|&a, &b| {
        compare(
            &scored[a],
            &scored[b],
            scored[a].static_score,
            scored[b].static_score,
        )
    });
    let protected_top1: HashSet<EntityId> = static_order
        .iter()
        .take(1)
        .filter_map(|&idx| {
            let item = &scored[idx];
            is_high_confidence_direct_hit(item, &expansion.anchors)
                .then(|| item.hit.entity.id().clone())
        })
        .collect();

    // Compute Track 1 additive graph bonus for in-pool items.
    let mut in_pool_boosted = 0usize;
    let mut graph_bonuses = HashMap::<EntityId, f64>::new();
    for item in &scored {
        let id = item.hit.entity.id();
        let g = structural_scores.get(id).copied().unwrap_or(0.0);
        let p4 = if expansion.p4_siblings.contains(id) {
            P4_SIBLING_BONUS
        } else {
            0.0
        };
        let bonus = IN_POOL_GRAPH_WEIGHT * g + p4;
        if bonus > 0.0 {
            in_pool_boosted += 1;
            graph_bonuses.insert(id.clone(), bonus);
        }
    }

    let single_file = scored
        .iter()
        .all(|s| s.hit.file.id() == scored[0].hit.file.id());

    let mut selected_per_file = HashMap::<FileId, u32>::new();
    let mut selected_ids_by_file = HashMap::<FileId, Vec<EntityId>>::new();
    let mut selected_ranges_by_file = HashMap::<FileId, Vec<(u64, u64)>>::new();
    let mut selected_hits: Vec<SearchHit> = Vec::with_capacity(code_ranks.len());
    let mut selected_effective_scores: Vec<f64> = Vec::with_capacity(code_ranks.len());
    let mut containment_deduped = 0usize;

    // Track 1: Greedy selection with containment deduplication + graph-linked same-file decay relief.
    for (slot_idx, &rank) in code_ranks.iter().enumerate() {
        // Drop any remaining candidates whose span is 100% contained in an already-selected item.
        let before = scored.len();
        scored.retain(|candidate| {
            !is_contained_in_selected(
                candidate.hit.file.id(),
                &candidate.hit.entity.source_range(),
                &selected_ranges_by_file,
            )
        });
        containment_deduped += before.saturating_sub(scored.len());
        if scored.is_empty() {
            break;
        }

        let effective = |item: &Scored| -> (f64, f64) {
            let id = item.hit.entity.id();
            let bonus = graph_bonuses.get(id).copied().unwrap_or(0.0);
            let combined = item.static_score + bonus;
            let count = if single_file {
                0
            } else {
                selected_per_file
                    .get(&item.hit.file.id())
                    .copied()
                    .unwrap_or(0)
            };
            let is_graph_linked = count > 0
                && count <= MAX_RELIEF_FILE_COUNT
                && selected_ids_by_file
                    .get(&item.hit.file.id())
                    .is_some_and(|peers| {
                        peers
                            .iter()
                            .any(|peer| expansion.linked_pairs.contains(&(peer.clone(), id.clone())))
                    });
            let decay = if is_graph_linked {
                GRAPH_LINKED_FILE_DECAY
            } else {
                FILE_DECAY
            };
            let factor = if single_file {
                1.0
            } else {
                1.0 / (1.0 + decay * f64::from(count))
            };
            (combined * factor, factor)
        };

        // Protect the #1 high-confidence direct hit so graph boosts never displace single-target top-1.
        let protect_this_slot = slot_idx == 0
            && scored
                .iter()
                .any(|item| protected_top1.contains(item.hit.entity.id()));

        let best = scored
            .iter()
            .enumerate()
            .filter(|(_, item)| !protect_this_slot || protected_top1.contains(item.hit.entity.id()))
            .min_by(|(_, a), (_, b)| compare(a, b, effective(a).0, effective(b).0))
            .map_or(0, |(index, _)| index);

        let mut item = scored.swap_remove(best);
        let (eff_score, factor) = effective(&item);
        let id = item.hit.entity.id().clone();
        let bonus = graph_bonuses.get(&id).copied().unwrap_or(0.0);
        let count = selected_per_file.entry(item.hit.file.id()).or_default();

        item.hit.score = eff_score;
        item.hit.rank = rank;
        if let Some(g) = structural_scores.get(&id).copied().filter(|&g| g > 0.0)
            && let Some(path) = best_paths.get(&id)
        {
            item.hit.graph_evidence =
                Some(build_graph_evidence(path, g, false, &expansion.node_labels));
        }

        if let Some(trace) = item.hit.trace.as_mut() {
            trace.ranking = Some(SearchRankingTrace {
                policy: format!("{}+{policy_name}", policy.name),
                query_features: query.describe(),
                normalized_base: item.base,
                symbol_match: item.symbol_match.to_owned(),
                symbol_bonus: item.symbol_bonus,
                definition_bonus: item.definition_bonus,
                path_bonus: item.path_bonus,
                file_support_bonus: item.support_bonus,
                file_support_count: item.support_count,
                path_roles: item.roles.names(),
                path_overrides: item.overrides.names(),
                path_factor: item.roles.factor(item.overrides),
                static_score: item.static_score,
                file_count_at_selection: *count,
                diversity_factor: factor,
                graph_bonus: bonus,
                graph_backfilled: false,
                score: item.hit.score,
                rank: item.hit.rank,
            });
        }

        if let Some(span) = byte_interval(&item.hit.entity.source_range()) {
            selected_ranges_by_file
                .entry(item.hit.file.id())
                .or_default()
                .push(span);
        }
        selected_ids_by_file
            .entry(item.hit.file.id())
            .or_default()
            .push(id);
        *count = count.saturating_add(1);
        selected_effective_scores.push(eff_score);
        selected_hits.push(item.hit);
    }

    // Track 2: Gated tail-slot backfill for high-confidence out-of-pool graph candidates.
    let mut backfilled_count = 0usize;
    let selected_files: HashSet<FileId> =
        selected_hits.iter().map(|hit| hit.file.id()).collect();
    let quota = tail_backfill_quota(code_ranks.len(), graph_policy);
    if quota > 0 && !expansion.out_of_pool_entities.is_empty() {
        let mut out_candidates: Vec<(EntityId, f64, f64)> = expansion
            .out_of_pool_entities
            .iter()
            .filter_map(|(id, stored)| {
                let g = structural_scores.get(id).copied()?;
                if g < OUT_OF_POOL_GATE {
                    return None;
                }
                if is_contained_in_selected(
                    stored.file.id(),
                    &stored.entity.source_range(),
                    &selected_ranges_by_file,
                ) {
                    return None;
                }
                let path = best_paths.get(id)?;
                let rel = query.graph_candidate_relevance(stored);
                let high_trust_topology = path.seed_is_anchor || path.pattern == "P3";
                if !high_trust_topology && rel < OUT_OF_POOL_RELEVANCE_GATE {
                    return None;
                }
                let mut priority = g + OUT_OF_POOL_RELEVANCE_WEIGHT * rel;
                if selected_files.contains(&stored.file.id()) {
                    priority += 0.05;
                }
                if path.seed_is_anchor && path.hops.len() == 1 {
                    priority += 0.05;
                }
                if path.pattern == "P3" {
                    priority += 0.04;
                }
                Some((id.clone(), g, priority))
            })
            .collect();
        out_candidates.sort_by(|a, b| {
            b.2.total_cmp(&a.2)
                .then_with(|| b.1.total_cmp(&a.1))
                .then_with(|| a.0.as_str().cmp(b.0.as_str()))
        });

        let mut candidate_iter = out_candidates.into_iter();
        let len = selected_hits.len();
        let start_tail = len.saturating_sub(quota);

        // Fill any unfilled code slots first (e.g. if containment dedup shortened the pool).
        while selected_hits.len() < code_ranks.len() && backfilled_count < quota {
            let Some((id, g, _)) = candidate_iter.next() else {
                break;
            };
            let Some(stored) = expansion.out_of_pool_entities.remove(&id) else {
                continue;
            };
            let Some(path) = best_paths.get(&id) else {
                continue;
            };
            if let Some(span) = byte_interval(&stored.entity.source_range()) {
                selected_ranges_by_file
                    .entry(stored.file.id())
                    .or_default()
                    .push(span);
            }
            let rank = code_ranks[selected_hits.len()];
            let hit = build_backfilled_hit(
                stored,
                rank,
                g,
                path,
                &expansion.node_labels,
                selected_hits.first().is_some_and(|h| h.trace.is_some()),
                code_ranks.len(),
                policy.name,
                policy_name,
                query,
            );
            selected_hits.push(hit);
            selected_effective_scores.push(g);
            backfilled_count += 1;
        }

        // Replace low-confidence tail slots from the bottom up.
        let top_eff = selected_effective_scores
            .first()
            .copied()
            .unwrap_or(1.0)
            .max(1.0);
        for tail_idx in (start_tail..len).rev() {
            if backfilled_count >= quota {
                break;
            }
            let tail_hit = &selected_hits[tail_idx];
            let tail_eff = selected_effective_scores[tail_idx];
            if !is_tail_slot_replaceable(tail_hit, tail_eff, top_eff, &expansion.anchors) {
                continue;
            }
            let Some((id, g, _)) = candidate_iter.next() else {
                break;
            };
            if BACKFILL_ADVANTAGE_FACTOR * g <= tail_eff / top_eff {
                continue;
            }
            let Some(stored) = expansion.out_of_pool_entities.remove(&id) else {
                continue;
            };
            let Some(path) = best_paths.get(&id) else {
                continue;
            };
            if let Some(span) = byte_interval(&stored.entity.source_range()) {
                selected_ranges_by_file
                    .entry(stored.file.id())
                    .or_default()
                    .push(span);
            }
            let rank = tail_hit.rank;
            let want_trace = tail_hit.trace.is_some();
            selected_hits[tail_idx] = build_backfilled_hit(
                stored,
                rank,
                g,
                path,
                &expansion.node_labels,
                want_trace,
                code_ranks.len(),
                policy.name,
                policy_name,
                query,
            );
            selected_effective_scores[tail_idx] = g;
            backfilled_count += 1;
        }
    }

    // Trailing `related:` addendum: surface up to MAX_RELATED_ITEMS highest-confidence remaining
    // out-of-pool structural targets without evicting any Top-K hybrid hit.
    let mut related_items = Vec::new();
    if graph_policy != GraphRankingPolicy::InPoolRerankOnly
        && !expansion.out_of_pool_entities.is_empty()
    {
        let mut related_candidates: Vec<(EntityId, f64, f64)> = expansion
            .out_of_pool_entities
            .iter()
            .filter_map(|(id, stored)| {
                let g = structural_scores.get(id).copied()?;
                if g < RELATED_GRAPH_GATE {
                    return None;
                }
                if is_contained_in_selected(
                    stored.file.id(),
                    &stored.entity.source_range(),
                    &selected_ranges_by_file,
                ) {
                    return None;
                }
                let path = best_paths.get(id)?;
                let rel = query.graph_candidate_relevance(stored);
                let same_file_strong = path.hops.len() == 1
                    && g >= RELATED_SAME_FILE_HIGH_GATE
                    && selected_files.contains(&stored.file.id());
                let high_trust_topology = path.seed_is_anchor || path.pattern == "P3";
                if rel < OUT_OF_POOL_RELEVANCE_GATE && !high_trust_topology && !same_file_strong {
                    return None;
                }
                let mut priority = g + OUT_OF_POOL_RELEVANCE_WEIGHT * rel;
                if selected_files.contains(&stored.file.id()) {
                    priority += 0.05;
                }
                if path.seed_is_anchor && path.hops.len() == 1 {
                    priority += 0.05;
                }
                if path.pattern == "P3" {
                    priority += 0.04;
                }
                Some((id.clone(), g, priority))
            })
            .collect();
        related_candidates.sort_by(|a, b| {
            b.2.total_cmp(&a.2)
                .then_with(|| b.1.total_cmp(&a.1))
                .then_with(|| a.0.as_str().cmp(b.0.as_str()))
        });

        let mut related_per_file = HashMap::<FileId, usize>::new();
        let mut seen_symbols = HashSet::<(FileId, Option<String>)>::new();
        for (id, g, _) in related_candidates {
            if related_items.len() >= MAX_RELATED_ITEMS {
                break;
            }
            let Some(stored) = expansion.out_of_pool_entities.get(&id) else {
                continue;
            };
            let file_id = stored.file.id();
            if related_per_file.get(&file_id).copied().unwrap_or(0) >= MAX_RELATED_PER_FILE {
                continue;
            }
            if is_contained_in_selected(
                file_id,
                &stored.entity.source_range(),
                &selected_ranges_by_file,
            ) {
                continue;
            }
            let (symbol_name, symbol_type, scope) = match stored.entity.metadata() {
                Some(EntityMetadata::Code(code)) => (
                    code.symbol_name.clone(),
                    code.symbol_type,
                    code.scope.clone(),
                ),
                _ => (None, None, None),
            };
            if !seen_symbols.insert((file_id, symbol_name.clone())) {
                continue;
            }
            let Some(path) = best_paths.get(&id) else {
                continue;
            };
            if let Some(span) = byte_interval(&stored.entity.source_range()) {
                selected_ranges_by_file
                    .entry(file_id)
                    .or_default()
                    .push(span);
            }
            *related_per_file.entry(file_id).or_default() += 1;
            related_items.push(RelatedGraphItem {
                relative_path: stored.file.relative_path().to_path_buf(),
                range: ContentRange::from_entity(&stored.entity),
                symbol_name,
                symbol_type,
                scope,
                graph_evidence: build_graph_evidence(path, g, false, &expansion.node_labels),
            });
        }
    }

    if let Some(obs) = observation.as_deref_mut() {
        for hit in &selected_hits {
            let backfilled = hit
                .graph_evidence
                .as_ref()
                .is_some_and(|ev| ev.backfilled);
            obs.graph_rank(hit, hit.rank, backfilled);
        }
    }

    let diagnostics = GraphSearchDiagnostics {
        active: true,
        policy: policy_name.to_owned(),
        seed_count: expansion.seeds.len(),
        anchor_count: expansion.anchors.len(),
        expanded_candidates: expansion.paths_by_target.len(),
        in_pool_boosted,
        backfilled_count,
        containment_deduped,
        truncated_by_budget: expansion.truncated_by_budget,
    };

    (selected_hits, related_items, diagnostics)
}

fn tail_backfill_quota(visible_slots: usize, policy: GraphRankingPolicy) -> usize {
    if policy == GraphRankingPolicy::InPoolRerankOnly || visible_slots < 4 {
        return 0;
    }
    if visible_slots >= 16 {
        3
    } else {
        (visible_slots / 4).min(2)
    }
}

fn is_tail_slot_replaceable(
    hit: &SearchHit,
    effective_score: f64,
    top_effective_score: f64,
    anchors: &HashSet<EntityId>,
) -> bool {
    if anchors.contains(hit.entity.id()) {
        return false;
    }
    if hit.matched_by == MatchedBy::FtsAndVector && effective_score >= 0.55 {
        return false;
    }
    if hit
        .trace
        .as_ref()
        .and_then(|t| t.ranking.as_ref())
        .is_some_and(|r| matches!(r.symbol_match.as_str(), "exact" | "qualified_exact"))
    {
        return false;
    }
    effective_score < TAIL_REPLACEABLE_THRESHOLD
        || effective_score < TAIL_RELATIVE_REPLACEABLE * top_effective_score
}

#[allow(clippy::too_many_arguments)]
fn build_backfilled_hit(
    stored: crate::storage::types::StoredEntity,
    rank: usize,
    graph_score: f64,
    path: &CandidatePath,
    node_labels: &HashMap<EntityId, (String, std::path::PathBuf)>,
    want_trace: bool,
    cutoff_rank: usize,
    base_policy: &str,
    graph_policy: &str,
    query: &QueryFeatures,
) -> SearchHit {
    let evidence = build_graph_evidence(path, graph_score, true, node_labels);
    let mut trace = want_trace.then(|| backfilled_trace(rank, cutoff_rank));
    if let Some(t) = trace.as_mut() {
        t.ranking = Some(SearchRankingTrace {
            policy: format!("{base_policy}+{graph_policy}"),
            query_features: query.describe(),
            normalized_base: 0.0,
            symbol_match: "none".to_owned(),
            symbol_bonus: 0.0,
            definition_bonus: 0.0,
            path_bonus: 0.0,
            file_support_bonus: 0.0,
            file_support_count: 0,
            path_roles: Vec::new(),
            path_overrides: Vec::new(),
            path_factor: 1.0,
            static_score: 0.0,
            file_count_at_selection: 0,
            diversity_factor: 1.0,
            graph_bonus: graph_score,
            graph_backfilled: true,
            score: graph_score,
            rank,
        });
    }
    SearchHit {
        entity: stored.entity,
        file: stored.file,
        evidence: Vec::new(),
        rank,
        score: graph_score,
        matched_by: MatchedBy::Graph,
        trace,
        graph_evidence: Some(evidence),
    }
}

fn build_graph_evidence(
    path: &CandidatePath,
    graph_score: f64,
    backfilled: bool,
    node_labels: &HashMap<EntityId, (String, std::path::PathBuf)>,
) -> GraphEvidence {
    let hops: Vec<GraphHop> = path
        .hops
        .iter()
        .map(|hop| {
            let (from_symbol, from_path) = node_labels
                .get(&hop.from_id)
                .cloned()
                .unwrap_or_else(|| (hop.from_id.as_str().to_owned(), std::path::PathBuf::new()));
            let (to_symbol, to_path) = node_labels
                .get(&hop.to_id)
                .cloned()
                .unwrap_or_else(|| (hop.to_id.as_str().to_owned(), std::path::PathBuf::new()));
            GraphHop {
                kind: hop.kind,
                direction: hop.direction,
                from_symbol,
                from_path,
                from_line: hop.call_line,
                to_symbol,
                to_path,
            }
        })
        .collect();

    // If the final hop arrived via an incoming Call (the returned item is a Caller),
    // focus short previews around its call-site line; otherwise focus on its definition header.
    let source_focus = path.hops.last().and_then(|last| {
        if last.kind == crate::domain::GraphEdgeKind::Calls && last.direction == Direction::In {
            last.call_line.map(|line| SourceFocus {
                kind: SourceFocusKind::CallSite,
                line,
            })
        } else {
            None
        }
    });

    GraphEvidence {
        hops,
        graph_score,
        pattern: path.pattern.to_owned(),
        backfilled,
        source_focus,
    }
}

fn byte_interval(range: &Range) -> Option<(u64, u64)> {
    range.byte_offsets().filter(|(start, end)| start < end)
}

fn is_contained_in_selected(
    file_id: FileId,
    range: &Range,
    selected_ranges_by_file: &HashMap<FileId, Vec<(u64, u64)>>,
) -> bool {
    let Some((start, end)) = byte_interval(range) else {
        return false;
    };
    selected_ranges_by_file
        .get(&file_id)
        .is_some_and(|ranges| ranges.iter().any(|&(s0, s1)| s0 <= start && end <= s1))
}
