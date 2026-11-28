//! Pattern-constrained bounded graph expansion (P1, P2, P3, P4) inside one ReadView snapshot.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{
    EngineResult,
    api::context::options::{GraphSearchDirection, GraphSearchOptions, QueryFilter},
    domain::{
        CodeMetadata, Direction, EdgeProvenance, EntityId, FileId, FileRecord, GraphEdgeKind,
        SymbolType,
    },
    file_selection::GlobMatcher,
    storage::{ReadView, graph::PagedEdge, types::StorageSearchFilter},
};

use super::{
    CandidatePath, ExpandedHop, GraphExpansion, blocked_by_test_barrier, display_symbol_name,
    seeds::select_seeds,
};
use crate::pipelines::indexed_search::{
    format_filter::{compile_format_filter, matches_file_name},
    ranking::{QueryFeatures, Roles, Scored},
    storage::GraphSearchVisitor,
};

const GRAPH_BUDGET: Duration = Duration::from_millis(200);
const HOP1_PAGE_LIMIT: usize = 64;
const HOP2_PAGE_LIMIT: usize = 32;
const MAX_HUB_DEGREE: usize = 32;
const MAX_RELATED_CLASSES: usize = 64;
const MAX_CLASS_MEMBERS: usize = 64;
const MAX_OUT_OF_POOL_HYDRATE: usize = 64;

/// Generic/builtin names that must never act as 2-hop bridges or noisy call targets.
const BLOCKED_HUB_NAMES: &[&str] = &[
    "type",
    "isinstance",
    "issubclass",
    "len",
    "str",
    "repr",
    "print",
    "get",
    "set",
    "new",
    "from",
    "into",
    "clone",
    "fmt",
    "default",
    "init",
    "__init__",
    "__str__",
    "__repr__",
    "to_string",
    "tostring",
    "equals",
    "hashcode",
    "hash_code",
    "value_of",
];

pub(super) struct GraphExpansionVisitor<'a> {
    scored: &'a [Scored],
    in_pool_ids: &'a HashSet<EntityId>,
    in_pool_files: &'a HashMap<EntityId, FileRecord>,
    query: &'a QueryFeatures,
    overrides: Roles,
    filter: &'a QueryFilter,
    storage_filter: Option<&'a StorageSearchFilter>,
    workspace_root: &'a Path,
    options: &'a GraphSearchOptions,
    expansion: GraphExpansion,
}

impl<'a> GraphExpansionVisitor<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        scored: &'a [Scored],
        in_pool_ids: &'a HashSet<EntityId>,
        in_pool_files: &'a HashMap<EntityId, FileRecord>,
        query: &'a QueryFeatures,
        overrides: Roles,
        filter: &'a QueryFilter,
        storage_filter: Option<&'a StorageSearchFilter>,
        workspace_root: &'a Path,
        options: &'a GraphSearchOptions,
    ) -> Self {
        Self {
            scored,
            in_pool_ids,
            in_pool_files,
            query,
            overrides,
            filter,
            storage_filter,
            workspace_root,
            options,
            expansion: GraphExpansion::default(),
        }
    }

    pub(super) fn into_expansion(self) -> GraphExpansion {
        self.expansion
    }

    fn effective_directions(&self) -> Vec<Direction> {
        match self.options.direction {
            GraphSearchDirection::Incoming => vec![Direction::In],
            GraphSearchDirection::Outgoing => vec![Direction::Out],
            GraphSearchDirection::Both => vec![Direction::Out, Direction::In],
            GraphSearchDirection::Auto => {
                if self.query.usages {
                    vec![Direction::In]
                } else {
                    vec![Direction::Out, Direction::In]
                }
            }
        }
    }
}

impl GraphSearchVisitor for GraphExpansionVisitor<'_> {
    fn visit(&mut self, view: &ReadView<'_>) -> EngineResult<()> {
        let started = Instant::now();
        let max_hops = self.options.max_hops.unwrap_or(2).clamp(1, 2);
        let directions = self.effective_directions();

        let (anchors, seeds) =
            select_seeds(view, self.scored, self.query, self.storage_filter)?;
        self.expansion.anchors = anchors;
        self.expansion.seeds = seeds.clone();
        if seeds.is_empty() {
            return Ok(());
        }

        // Pre-populate labels for in-pool items.
        for item in self.scored {
            self.expansion.node_labels.insert(
                item.hit.entity.id().clone(),
                (
                    display_symbol_name(&item.hit.entity),
                    item.hit.file.relative_path().to_path_buf(),
                ),
            );
        }

        // Cache visibility and metadata within this snapshot to avoid redundant reads.
        let mut visible_files: HashMap<EntityId, FileRecord> = self.in_pool_files.clone();
        let mut code_meta: HashMap<EntityId, (FileId, CodeMetadata)> = HashMap::new();

        // Preload seed metadata so P3 knows whether each seed is a class or method.
        let seed_ids: Vec<EntityId> = seeds.iter().map(|s| s.id.clone()).collect();
        code_meta.extend(view.fetch_code_metadata(&seed_ids)?);
        let missing_seed_files: Vec<EntityId> = seed_ids
            .iter()
            .filter(|id| !visible_files.contains_key(*id))
            .cloned()
            .collect();
        if !missing_seed_files.is_empty() {
            visible_files.extend(view.visible_endpoint_files(&missing_seed_files)?);
        }

        // P1 + P2: Bounded directed Calls expansion (1-hop + monotonic 2-hop).
        for seed in &seeds {
            if started.elapsed() >= GRAPH_BUDGET {
                self.expansion.truncated_by_budget = true;
                break;
            }
            for &direction in &directions {
                if started.elapsed() >= GRAPH_BUDGET {
                    self.expansion.truncated_by_budget = true;
                    break;
                }
                let hop1_edges = admitted_call_edges(
                    view,
                    seed.id.as_str(),
                    direction,
                    HOP1_PAGE_LIMIT,
                    &mut visible_files,
                )?;
                let branching1 = hop1_edges.len().max(1);
                let mut hop1_targets = Vec::new();

                for paged in &hop1_edges {
                    let target_id = other_endpoint(&paged.edge, seed.id.as_str());
                    if target_id == seed.id || is_file_endpoint(target_id.as_str()) {
                        continue;
                    }
                    let Some(target_file) = visible_files.get(&target_id) else {
                        continue;
                    };
                    if blocked_by_test_barrier(
                        target_file.relative_path().as_path(),
                        self.overrides,
                        seed.is_test,
                    ) {
                        continue;
                    }

                    let hop1 = ExpandedHop {
                        kind: GraphEdgeKind::Calls,
                        direction,
                        from_id: seed.id.clone(),
                        to_id: target_id.clone(),
                        call_line: paged.edge.line,
                        provenance: paged.edge.provenance,
                        branching: branching1,
                    };
                    self.expansion
                        .linked_pairs
                        .insert((seed.id.clone(), target_id.clone()));
                    self.expansion
                        .linked_pairs
                        .insert((target_id.clone(), seed.id.clone()));
                    self.expansion
                        .paths_by_target
                        .entry(target_id.clone())
                        .or_default()
                        .push(CandidatePath {
                            seed_file_id: seed.file_id,
                            seed_score: seed.score,
                            seed_is_anchor: seed.is_anchor,
                            pattern: "P1",
                            hops: vec![hop1.clone()],
                        });
                    hop1_targets.push((target_id, hop1));
                }

                // P2: Monotonic 2-hop Calls (same direction only, hub degree < MAX_HUB_DEGREE).
                if max_hops >= 2 && !hop1_targets.is_empty() {
                    let mid_ids: Vec<EntityId> = hop1_targets
                        .iter()
                        .map(|(id, _)| id.clone())
                        .filter(|id| !code_meta.contains_key(id))
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect();
                    if !mid_ids.is_empty() {
                        code_meta.extend(view.fetch_code_metadata(&mid_ids)?);
                    }

                    for (mid_id, hop1) in hop1_targets {
                        if started.elapsed() >= GRAPH_BUDGET {
                            self.expansion.truncated_by_budget = true;
                            break;
                        }
                        if is_blocked_hub_symbol(code_meta.get(&mid_id).map(|(_, m)| m)) {
                            continue;
                        }
                        let hop2_edges = admitted_call_edges(
                            view,
                            mid_id.as_str(),
                            direction,
                            HOP2_PAGE_LIMIT + 1,
                            &mut visible_files,
                        )?;
                        if hop2_edges.len() >= MAX_HUB_DEGREE {
                            continue;
                        }
                        let branching2 = hop2_edges.len().max(1);
                        for paged2 in &hop2_edges {
                            let target2 = other_endpoint(&paged2.edge, mid_id.as_str());
                            if target2 == seed.id
                                || target2 == mid_id
                                || is_file_endpoint(target2.as_str())
                            {
                                continue;
                            }
                            let Some(target2_file) = visible_files.get(&target2) else {
                                continue;
                            };
                            if blocked_by_test_barrier(
                                target2_file.relative_path().as_path(),
                                self.overrides,
                                seed.is_test,
                            ) {
                                continue;
                            }
                            let hop2 = ExpandedHop {
                                kind: GraphEdgeKind::Calls,
                                direction,
                                from_id: mid_id.clone(),
                                to_id: target2.clone(),
                                call_line: paged2.edge.line,
                                provenance: paged2.edge.provenance,
                                branching: branching2,
                            };
                            self.expansion
                                .linked_pairs
                                .insert((seed.id.clone(), target2.clone()));
                            self.expansion
                                .linked_pairs
                                .insert((target2.clone(), seed.id.clone()));
                            self.expansion
                                .linked_pairs
                                .insert((mid_id.clone(), target2.clone()));
                            self.expansion
                                .linked_pairs
                                .insert((target2.clone(), mid_id.clone()));
                            self.expansion
                                .paths_by_target
                                .entry(target2)
                                .or_default()
                                .push(CandidatePath {
                                    seed_file_id: seed.file_id,
                                    seed_score: seed.score,
                                    seed_is_anchor: seed.is_anchor,
                                    pattern: "P2",
                                    hops: vec![hop1.clone(), hop2],
                                });
                        }
                    }
                }
            }
        }

        // P3: Polymorphic Extends/Implements + matching member override.
        if !self.expansion.truncated_by_budget {
            self.expand_p3_polymorphic(view, &seeds, &mut visible_files, &mut code_meta, started)?;
        }

        // P4: Co-container Contains sibling boost (strictly for candidates already in C0).
        if !self.expansion.truncated_by_budget {
            self.collect_p4_siblings(view, &seeds, started)?;
        }

        // Hydrate surviving out-of-pool targets that pass QueryFilter (bounded).
        self.hydrate_out_of_pool(view, &visible_files, &mut code_meta)?;

        // Populate labels for any intermediate or seed nodes referenced in hops.
        self.populate_hop_labels(view, &visible_files, &mut code_meta)?;

        Ok(())
    }
}

impl GraphExpansionVisitor<'_> {
    fn expand_p3_polymorphic(
        &mut self,
        view: &ReadView<'_>,
        seeds: &[super::seeds::GraphSeed],
        visible_files: &mut HashMap<EntityId, FileRecord>,
        code_meta: &mut HashMap<EntityId, (FileId, CodeMetadata)>,
        started: Instant,
    ) -> EngineResult<()> {
        let inheritance_kinds = [GraphEdgeKind::Extends, GraphEdgeKind::Implements];
        for seed in seeds {
            if started.elapsed() >= GRAPH_BUDGET {
                self.expansion.truncated_by_budget = true;
                break;
            }
            let seed_meta = code_meta.get(&seed.id).map(|(_, m)| m.clone());
            let is_type_seed = seed_meta.as_ref().is_some_and(|m| {
                matches!(
                    m.symbol_type,
                    Some(SymbolType::Class | SymbolType::Interface)
                )
            });

            // Determine enclosing class `owner_class` and optional method name `member_name`.
            let (owner_class, member_name) = if is_type_seed {
                (seed.id.clone(), None)
            } else {
                let containers = view.neighborhood_paged(
                    seed.id.as_str(),
                    Direction::In,
                    &[GraphEdgeKind::Contains],
                    0,
                    4,
                )?;
                let Some(enclosing) = containers
                    .into_iter()
                    .map(|p| EntityId::from_string(p.edge.source))
                    .find(|id| !is_file_endpoint(id.as_str()))
                else {
                    continue;
                };
                let name = seed_meta.and_then(|m| m.symbol_name);
                (enclosing, name)
            };

            // Find subclasses (In) and base classes (Out) of `owner_class`.
            let related = view.neighborhood_paged(
                owner_class.as_str(),
                Direction::Both,
                &inheritance_kinds,
                0,
                MAX_RELATED_CLASSES,
            )?;
            if related.is_empty() {
                continue;
            }

            let related_ids: Vec<EntityId> = related
                .iter()
                .map(|p| other_endpoint(&p.edge, owner_class.as_str()))
                .filter(|id| !is_file_endpoint(id.as_str()) && !visible_files.contains_key(id))
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            if !related_ids.is_empty() {
                visible_files.extend(view.visible_endpoint_files(&related_ids)?);
            }

            let branching = related.len().max(1);
            let mut matched_overrides: Vec<(EntityId, GraphEdgeKind, Direction, Option<usize>, EdgeProvenance)> =
                Vec::new();
            for paged in related {
                if started.elapsed() >= GRAPH_BUDGET {
                    self.expansion.truncated_by_budget = true;
                    break;
                }
                let related_class = other_endpoint(&paged.edge, owner_class.as_str());
                if related_class == owner_class || is_file_endpoint(related_class.as_str()) {
                    continue;
                }
                let Some(class_file) = visible_files.get(&related_class) else {
                    continue;
                };
                if blocked_by_test_barrier(
                    class_file.relative_path().as_path(),
                    self.overrides,
                    seed.is_test,
                ) {
                    continue;
                }
                let direction = if paged.edge.source == owner_class.as_str() {
                    Direction::Out
                } else {
                    Direction::In
                };

                // If query explicitly asks for implementations, surface the subclass itself.
                if is_type_seed && direction == Direction::In && self.query.implementations() {
                    let hop = ExpandedHop {
                        kind: paged.edge.kind,
                        direction,
                        from_id: seed.id.clone(),
                        to_id: related_class.clone(),
                        call_line: paged.edge.line,
                        provenance: paged.edge.provenance,
                        branching,
                    };
                    self.record_p3_path(seed, related_class.clone(), hop);
                }

                // Scan members of `related_class` and match strictly by member name or query token.
                let member_edges = view.neighborhood_paged(
                    related_class.as_str(),
                    Direction::Out,
                    &[GraphEdgeKind::Contains],
                    0,
                    MAX_CLASS_MEMBERS,
                )?;
                let member_ids: Vec<EntityId> = member_edges
                    .iter()
                    .map(|p| EntityId::from_string(p.edge.target.clone()))
                    .filter(|id| !is_file_endpoint(id.as_str()))
                    .collect();
                let missing_meta: Vec<EntityId> = member_ids
                    .iter()
                    .filter(|id| !code_meta.contains_key(*id))
                    .cloned()
                    .collect();
                if !missing_meta.is_empty() {
                    code_meta.extend(view.fetch_code_metadata(&missing_meta)?);
                }
                let missing_vis: Vec<EntityId> = member_ids
                    .iter()
                    .filter(|id| !visible_files.contains_key(*id))
                    .cloned()
                    .collect();
                if !missing_vis.is_empty() {
                    visible_files.extend(view.visible_endpoint_files(&missing_vis)?);
                }

                for member_id in member_ids {
                    if member_id == seed.id {
                        continue;
                    }
                    let Some((_, meta)) = code_meta.get(&member_id) else {
                        continue;
                    };
                    let Some(name) = meta.symbol_name.as_deref() else {
                        continue;
                    };
                    let matches_member = match member_name.as_deref() {
                        Some(expected) => name == expected,
                        None => self.query.has_informative_token_match(name),
                    };
                    if !matches_member {
                        continue;
                    }
                    let Some(member_file) = visible_files.get(&member_id) else {
                        continue;
                    };
                    if blocked_by_test_barrier(
                        member_file.relative_path().as_path(),
                        self.overrides,
                        seed.is_test,
                    ) {
                        continue;
                    }
                    matched_overrides.push((
                        member_id,
                        paged.edge.kind,
                        direction,
                        paged.edge.line,
                        paged.edge.provenance,
                    ));
                }
            }

            let override_branching = matched_overrides.len().max(1);
            for (member_id, kind, direction, call_line, provenance) in matched_overrides {
                let hop = ExpandedHop {
                    kind,
                    direction,
                    from_id: seed.id.clone(),
                    to_id: member_id.clone(),
                    call_line,
                    provenance,
                    branching: override_branching,
                };
                self.record_p3_path(seed, member_id, hop);
            }
        }
        Ok(())
    }

    fn record_p3_path(
        &mut self,
        seed: &super::seeds::GraphSeed,
        target_id: EntityId,
        hop: ExpandedHop,
    ) {
        self.expansion
            .linked_pairs
            .insert((seed.id.clone(), target_id.clone()));
        self.expansion
            .linked_pairs
            .insert((target_id.clone(), seed.id.clone()));
        self.expansion
            .paths_by_target
            .entry(target_id)
            .or_default()
            .push(CandidatePath {
                seed_file_id: seed.file_id,
                seed_score: seed.score,
                seed_is_anchor: seed.is_anchor,
                pattern: "P3",
                hops: vec![hop],
            });
    }

    fn collect_p4_siblings(
        &mut self,
        view: &ReadView<'_>,
        seeds: &[super::seeds::GraphSeed],
        started: Instant,
    ) -> EngineResult<()> {
        let mut seed_containers = HashSet::new();
        for seed in seeds {
            if started.elapsed() >= GRAPH_BUDGET {
                self.expansion.truncated_by_budget = true;
                return Ok(());
            }
            let parents = view.neighborhood_paged(
                seed.id.as_str(),
                Direction::In,
                &[GraphEdgeKind::Contains],
                0,
                4,
            )?;
            for paged in parents {
                if !is_file_endpoint(&paged.edge.source) {
                    seed_containers.insert(paged.edge.source);
                }
            }
        }
        for container in seed_containers {
            if started.elapsed() >= GRAPH_BUDGET {
                self.expansion.truncated_by_budget = true;
                return Ok(());
            }
            let children = view.neighborhood_paged(
                &container,
                Direction::Out,
                &[GraphEdgeKind::Contains],
                0,
                MAX_CLASS_MEMBERS,
            )?;
            for paged in children {
                let child_id = EntityId::from_string(paged.edge.target);
                if self.in_pool_ids.contains(&child_id)
                    && !seeds.iter().any(|s| s.id == child_id)
                {
                    self.expansion.p4_siblings.insert(child_id);
                }
            }
        }
        Ok(())
    }

    fn hydrate_out_of_pool(
        &mut self,
        view: &ReadView<'_>,
        visible_files: &HashMap<EntityId, FileRecord>,
        code_meta: &mut HashMap<EntityId, (FileId, CodeMetadata)>,
    ) -> EngineResult<()> {
        let mut out_of_pool: Vec<(EntityId, f64)> = self
            .expansion
            .paths_by_target
            .iter()
            .filter(|(id, _)| !self.in_pool_ids.contains(*id))
            .map(|(id, paths)| {
                let max_preview = paths
                    .iter()
                    .map(super::score::path_score)
                    .fold(0.0_f64, f64::max);
                (id.clone(), max_preview)
            })
            .collect();
        if out_of_pool.is_empty() {
            return Ok(());
        }
        out_of_pool.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.0.as_str().cmp(b.0.as_str()))
        });

        let matcher = GlobMatcher::new(self.workspace_root, &self.filter.globs)?;
        let types = crate::file_selection::file_types(
            &self.filter.file_types,
            &self.filter.excluded_file_types,
        )?;
        let format_filter = compile_format_filter(self.filter);

        let mut filtered_ids = Vec::new();
        for (id, _) in out_of_pool {
            if filtered_ids.len() >= MAX_OUT_OF_POOL_HYDRATE {
                break;
            }
            let Some(file) = visible_files.get(&id) else {
                continue;
            };
            if !file_matches_query_filter(
                self.filter,
                &matcher,
                &types,
                format_filter.as_ref(),
                file,
            ) {
                continue;
            }
            filtered_ids.push(id);
        }

        if !self.filter.symbol_types.is_empty() && !filtered_ids.is_empty() {
            let missing_meta: Vec<EntityId> = filtered_ids
                .iter()
                .filter(|id| !code_meta.contains_key(*id))
                .cloned()
                .collect();
            if !missing_meta.is_empty() {
                code_meta.extend(view.fetch_code_metadata(&missing_meta)?);
            }
            filtered_ids.retain(|id| {
                code_meta.get(id).is_some_and(|(_, meta)| {
                    meta.symbol_type
                        .is_some_and(|kind| self.filter.symbol_types.contains(&kind))
                })
            });
        }

        if !filtered_ids.is_empty() {
            let loaded = view.fetch_stored_entities(&filtered_ids)?;
            for (id, stored) in loaded {
                self.expansion.node_labels.insert(
                    id.clone(),
                    (
                        display_symbol_name(&stored.entity),
                        stored.file.relative_path().to_path_buf(),
                    ),
                );
                self.expansion.out_of_pool_entities.insert(id, stored);
            }
        }
        Ok(())
    }

    fn populate_hop_labels(
        &mut self,
        view: &ReadView<'_>,
        visible_files: &HashMap<EntityId, FileRecord>,
        code_meta: &mut HashMap<EntityId, (FileId, CodeMetadata)>,
    ) -> EngineResult<()> {
        let mut needed = HashSet::new();
        for paths in self.expansion.paths_by_target.values() {
            for path in paths {
                for hop in &path.hops {
                    if !self.expansion.node_labels.contains_key(&hop.from_id) {
                        needed.insert(hop.from_id.clone());
                    }
                    if !self.expansion.node_labels.contains_key(&hop.to_id) {
                        needed.insert(hop.to_id.clone());
                    }
                }
            }
        }
        if needed.is_empty() {
            return Ok(());
        }
        let ids: Vec<EntityId> = needed.into_iter().collect();
        let missing_meta: Vec<EntityId> = ids
            .iter()
            .filter(|id| !code_meta.contains_key(*id))
            .cloned()
            .collect();
        if !missing_meta.is_empty() {
            code_meta.extend(view.fetch_code_metadata(&missing_meta)?);
        }
        let missing_files: Vec<EntityId> = ids
            .iter()
            .filter(|id| !visible_files.contains_key(*id))
            .cloned()
            .collect();
        let extra_files = if missing_files.is_empty() {
            HashMap::new()
        } else {
            view.visible_endpoint_files(&missing_files)?
        };
        for id in ids {
            let path = visible_files
                .get(&id)
                .or_else(|| extra_files.get(&id))
                .map_or_else(PathBuf::new, |file| file.relative_path().to_path_buf());
            let label = code_meta
                .get(&id)
                .and_then(|(_, meta)| {
                    let name = meta.symbol_name.as_deref().filter(|s| !s.is_empty())?;
                    Some(match meta.scope.as_deref().filter(|s| !s.is_empty()) {
                        Some(scope) => format!("{scope}.{name}"),
                        None => name.to_owned(),
                    })
                })
                .unwrap_or_else(|| id.as_str().to_owned());
            self.expansion.node_labels.insert(id, (label, path));
        }
        Ok(())
    }
}

fn admitted_call_edges(
    view: &ReadView<'_>,
    id: &str,
    direction: Direction,
    limit: usize,
    visible_files: &mut HashMap<EntityId, FileRecord>,
) -> EngineResult<Vec<PagedEdge>> {
    let raw = view.neighborhood_paged(id, direction, &[GraphEdgeKind::Calls], 0, limit)?;
    let high_prov: Vec<PagedEdge> = raw
        .into_iter()
        .filter(|paged| is_high_confidence_provenance(paged.edge.provenance))
        .collect();
    if high_prov.is_empty() {
        return Ok(Vec::new());
    }
    let missing: Vec<EntityId> = high_prov
        .iter()
        .flat_map(|p| [&p.edge.source, &p.edge.target])
        .map(|s| EntityId::from_string(s.clone()))
        .filter(|ep| !is_file_endpoint(ep.as_str()) && !visible_files.contains_key(ep))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if !missing.is_empty() {
        visible_files.extend(view.visible_endpoint_files(&missing)?);
    }
    Ok(high_prov
        .into_iter()
        .filter(|p| {
            let src = EntityId::from_string(p.edge.source.clone());
            let dst = EntityId::from_string(p.edge.target.clone());
            (is_file_endpoint(src.as_str()) || visible_files.contains_key(&src))
                && (is_file_endpoint(dst.as_str()) || visible_files.contains_key(&dst))
        })
        .collect())
}

pub(super) const fn is_high_confidence_provenance(provenance: EdgeProvenance) -> bool {
    matches!(
        provenance,
        EdgeProvenance::FileLocal
            | EdgeProvenance::QualifiedNameMatch
            | EdgeProvenance::FileScoped
            | EdgeProvenance::ImportBound
            | EdgeProvenance::ImportScoped
    )
}

pub(super) fn provenance_weight(provenance: EdgeProvenance) -> f64 {
    match provenance {
        EdgeProvenance::FileLocal => 1.0,
        EdgeProvenance::QualifiedNameMatch => 0.95,
        EdgeProvenance::FileScoped => 0.92,
        EdgeProvenance::ImportBound => 0.90,
        EdgeProvenance::ImportScoped => 0.85,
        EdgeProvenance::ArityBound
        | EdgeProvenance::ExportBound
        | EdgeProvenance::WorkspaceUnique => 0.60,
    }
}

fn is_blocked_hub_symbol(meta: Option<&CodeMetadata>) -> bool {
    let Some(name) = meta.and_then(|m| m.symbol_name.as_deref()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower.len() <= 2 || BLOCKED_HUB_NAMES.contains(&lower.as_str())
}

fn other_endpoint(edge: &crate::domain::FileEdge, current: &str) -> EntityId {
    if edge.source == current {
        EntityId::from_string(edge.target.clone())
    } else {
        EntityId::from_string(edge.source.clone())
    }
}

fn is_file_endpoint(id: &str) -> bool {
    crate::storage::graph::parse_file_endpoint(id).is_some()
}

fn file_matches_query_filter(
    filter: &QueryFilter,
    matcher: &GlobMatcher,
    types: &ignore::types::Types,
    format_filter: Option<&crate::storage::types::StoragePathFilter>,
    file: &FileRecord,
) -> bool {
    let relative = file.relative_path().as_path();
    let modified = file.snapshot().modified_epoch_ms;
    matcher.matches_path(relative)
        && !types.matched(relative, false).is_ignore()
        && filter
            .modified_after_epoch_ms
            .is_none_or(|after| modified.is_some_and(|m| m >= after))
        && filter
            .modified_before_epoch_ms
            .is_none_or(|before| modified.is_some_and(|m| m <= before))
        && format_filter.is_none_or(|predicate| {
            relative
                .file_name()
                .is_some_and(|name| matches_file_name(predicate, name))
        })
}
