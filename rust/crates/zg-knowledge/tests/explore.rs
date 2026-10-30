//! Algorithm contracts; real graphdb/zvec coverage lives in engine `graph_explore`.
use std::time::{Duration, Instant};
use zg_knowledge::{
    query::*,
    relations::{
        Lookup, ResolutionStatus,
        bound::{FileRecord, IndexStatus},
    },
};
struct Graph {
    nodes: Vec<EntityRecord>,
    edges: Vec<RelationRecord>,
}
const TEXT: &str = "abcdefghijklmnopqrstuvwxyz";
fn entity(id: &str) -> EntityRecord {
    EntityRecord {
        id: id.into(),
        source_id: format!("{id}.rs"),
        kind: EntityKind::Function,
        name: id.into(),
        qualified_name: id.into(),
        span: ByteSpan { start: 0, end: 1 },
    }
}
fn evidence(source: &str, id: Option<&str>) -> EvidenceRef {
    let digest = zg_storage::sha256_hex(TEXT.as_bytes());
    EvidenceRef {
        source_id: source.into(),
        raw_digest: digest.clone(),
        decoded_digest: digest,
        span: ByteSpan { start: 0, end: 1 },
        reference_id: id.map(str::to_owned),
    }
}
fn edge(id: &str, from: &str, to: &str) -> RelationRecord {
    RelationRecord {
        id: id.into(),
        source: from.into(),
        target: to.into(),
        kind: RelationKind::Calls,
        reference: Some(ReferenceRecord {
            id: id.into(),
            owner: from.into(),
            target: Some(to.into()),
            kind: RelationKind::Calls,
            status: ResolutionStatus::Resolved,
            reason: "static".into(),
            locator: Lookup::Name {
                parts: vec![to.into()],
            },
            evidence: evidence(&format!("{from}.rs"), Some(id)),
        }),
    }
}
fn page<T: Clone>(rows: &[T], offset: u64) -> Vec<T> {
    rows.iter()
        .skip(usize::try_from(offset).expect("test offset"))
        .take(256)
        .cloned()
        .collect()
}
impl GraphReader for Graph {
    fn span_nodes(
        &self,
        source: &str,
        span: ByteSpan,
        offset: u64,
        c: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        c.check()?;
        Ok(page(
            &self
                .nodes
                .iter()
                .filter(|n| {
                    n.source_id == source && n.span.start < span.end && n.span.end > span.start
                })
                .cloned()
                .collect::<Vec<_>>(),
            offset,
        ))
    }
    fn locate(
        &self,
        l: &Locator,
        offset: u64,
        c: &RequestContext,
    ) -> QueryResult<Vec<EntityRecord>> {
        c.check()?;
        Ok(page(
            &self
                .nodes
                .iter()
                .filter(|n| match l {
                    Locator::Name {
                        name,
                        source_id,
                        kind,
                    } => {
                        (&n.name == name || &n.qualified_name == name)
                            && source_id.as_ref().is_none_or(|s| s == &n.source_id)
                            && kind.is_none_or(|k| k == n.kind)
                    }
                    Locator::Node { node } => node.node_id == n.id,
                    Locator::Position {
                        source_id,
                        byte_offset,
                        ..
                    } => {
                        source_id == &n.source_id
                            && *byte_offset >= n.span.start
                            && *byte_offset < n.span.end
                    }
                })
                .cloned()
                .collect::<Vec<_>>(),
            offset,
        ))
    }
    fn get_nodes(&self, ids: &[String], c: &RequestContext) -> QueryResult<Vec<EntityRecord>> {
        c.check()?;
        Ok(self
            .nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect())
    }
    fn neighbors(
        &self,
        ids: &[String],
        kind: RelationKind,
        d: Direction,
        offset: u64,
        c: &RequestContext,
    ) -> QueryResult<Vec<RelationRecord>> {
        c.check()?;
        let mut rows = self
            .edges
            .iter()
            .filter(|e| {
                e.kind == kind
                    && match d {
                        Direction::Outgoing => ids.contains(&e.source),
                        Direction::Incoming => ids.contains(&e.target),
                        Direction::Both => ids.contains(&e.source) || ids.contains(&e.target),
                    }
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(page(&rows, offset))
    }
    fn references(
        &self,
        partition: ReferencePartition,
        owner: Option<&str>,
        source: Option<&str>,
        id: Option<&str>,
        offset: u64,
        c: &RequestContext,
    ) -> QueryResult<Vec<ReferenceRecord>> {
        c.check()?;
        if partition != ReferencePartition::Calls {
            return Ok(vec![]);
        }
        Ok(page(
            &self
                .edges
                .iter()
                .filter_map(|e| e.reference.clone())
                .filter(|r| {
                    owner.is_none_or(|o| o == r.owner)
                        && source.is_none_or(|s| s == r.evidence.source_id)
                        && id.is_none_or(|i| i == r.id)
                })
                .collect::<Vec<_>>(),
            offset,
        ))
    }
    fn source(&self, s: &str, c: &RequestContext) -> QueryResult<SourceRecord> {
        c.check()?;
        let digest = zg_storage::sha256_hex(TEXT.as_bytes());
        Ok(SourceRecord {
            record: FileRecord {
                path: s.into(),
                file_id: 1,
                index_status: IndexStatus::Indexed,
                content_hash: Some(digest.clone()),
                size_bytes: TEXT.len() as u64,
                text_revision: 1,
                storage_generation: "generation".into(),
                entity_count: 0,
                entity_ids_digest: digest.clone(),
            },
            language: Some(zg_knowledge::relations::Language::Rust),
            complete: true,
            raw_digest: Some(digest.clone()),
            decoded_digest: Some(digest),
            text_map: vec![TextMap {
                source_id: s.into(),
                entity_id: format!("{s}:entity"),
                source_span: ByteSpan {
                    start: 0,
                    end: TEXT.len() as u64,
                },
                content_span: ByteSpan {
                    start: 0,
                    end: TEXT.len() as u64,
                },
            }],
            text_binding_digest: Some("attested".into()),
        })
    }
}
struct Evidence {
    unavailable: bool,
}
impl EvidenceProvider<()> for Evidence {
    fn read_evidence(
        &self,
        _: &KgReadView<()>,
        refs: &[EvidenceRef],
        c: &RequestContext,
    ) -> QueryResult<Vec<EvidenceRead>> {
        c.check()?;
        refs.iter()
            .map(|r| {
                if self.unavailable {
                    Ok(EvidenceRead::Unavailable {
                        reference: r.clone(),
                        reason: "changed".into(),
                    })
                } else {
                    Ok(EvidenceRead::Verified {
                        reference: r.clone(),
                        content: r.span.slice(TEXT)?.into(),
                    })
                }
            })
            .collect()
    }
}
fn view() -> KgReadView<()> {
    KgReadView::new(
        ViewStamp {
            workspace_id: "workspace".into(),
            generation: "generation".into(),
            text_revision: 1,
            graph: Some(GraphStamp {
                build_id: "build".into(),
                schema: "v2".into(),
                rules: "rules".into(),
                configuration: "config".into(),
                catalog_revision: "catalog".into(),
            }),
        },
        (),
        &ReadViewRequest {
            graph: GraphRequirement::Required,
            needs: Some(GraphReadNeeds {
                domain: QueryDomain::Code,
                relations: vec![],
                text_mapping: false,
            }),
        },
        vec![GraphReadCapabilities {
            domain: QueryDomain::Code,
            relations: vec![RelationKind::Calls, RelationKind::Imports],
            text_mapping: true,
        }],
        None,
    )
    .expect("view")
}
fn context(limits: BudgetLimits) -> RequestContext {
    RequestContext::new(
        limits,
        tokio_util::sync::CancellationToken::new(),
        Instant::now() + Duration::from_secs(30),
    )
    .expect("context")
}

fn ranked(source: &str, spans: Vec<ByteSpan>, priority: u32, query: &str) -> RankedHit {
    RankedHit {
        source_id: source.into(),
        entity_id: format!("{source}:entity"),
        fragment_id: format!("fragment-{priority}"),
        source_spans: spans,
        priority,
        query_sources: vec![query.into()],
    }
}

#[test]
fn seed_mapping_uses_spans_and_merges_provenance_after_unique_seed_limit() {
    let graph = graph();
    let view = view();
    let hits = RankedHits {
        view: view.stamp().clone(),
        hits: vec![
            ranked("root.rs", vec![ByteSpan { start: 0, end: 1 }], 1, "Q1"),
            ranked("leaf.rs", vec![ByteSpan { start: 0, end: 1 }], 2, "Q2"),
            ranked("root.rs", vec![ByteSpan { start: 0, end: 1 }], 3, "Q3"),
        ],
    };
    let selected = select_seeds(
        &graph,
        &view,
        &hits,
        1,
        &SourceFilter::default(),
        &context(BudgetLimits::default()),
    )
    .expect("mapping");
    assert_eq!(selected.seeds.len(), 1);
    assert_eq!(selected.seeds[0].node.node_id, "root");
    assert_eq!(selected.seeds[0].hit_indices, [0, 2]);
    assert_eq!(selected.seeds[0].query_sources, ["Q1", "Q3"]);
    assert_eq!(selected.skipped[0].reason, "seed_limit");
}

#[test]
fn seed_mapping_rejects_container_ambiguity_missing_mapping_and_partial_location() {
    let mut root = entity("root");
    root.span = ByteSpan { start: 0, end: 26 };
    root.kind = EntityKind::File;
    let mut left = entity("left");
    left.source_id = "root.rs".into();
    left.span = ByteSpan { start: 0, end: 10 };
    let mut right = entity("right");
    right.source_id = "root.rs".into();
    right.span = ByteSpan { start: 15, end: 25 };
    let mut graph = Graph {
        nodes: vec![root, left, right],
        edges: vec![],
    };
    let view = view();
    let mut hits = RankedHits {
        view: view.stamp().clone(),
        hits: vec![ranked(
            "root.rs",
            vec![
                ByteSpan { start: 2, end: 8 },
                ByteSpan { start: 16, end: 20 },
            ],
            1,
            "Q1",
        )],
    };
    let run = |graph: &Graph, hits: &RankedHits| {
        select_seeds(
            graph,
            &view,
            hits,
            5,
            &SourceFilter::default(),
            &context(BudgetLimits::default()),
        )
        .expect("mapping result")
    };
    assert_eq!(run(&graph, &hits).skipped[0].reason, "container_hit");
    hits.hits[0].source_spans = vec![ByteSpan { start: 2, end: 4 }, ByteSpan { start: 6, end: 8 }];
    assert_eq!(run(&graph, &hits).seeds[0].node.node_id, "left");
    let mut duplicate = graph.nodes[1].clone();
    duplicate.id = "same-span".into();
    graph.nodes.push(duplicate);
    assert_eq!(run(&graph, &hits).skipped[0].reason, "ambiguous_owner");
    hits.hits[0].entity_id = "missing".into();
    assert_eq!(run(&graph, &hits).skipped[0].reason, "mapping_incomplete");
    hits.hits[0].entity_id = "root.rs:entity".into();
    let error = select_seeds(
        &graph,
        &view,
        &hits,
        5,
        &SourceFilter::default(),
        &context(BudgetLimits {
            candidates: 1,
            ..Default::default()
        }),
    )
    .expect_err("incomplete location cannot select a seed");
    assert_eq!(error.kind, QueryErrorKind::ResourceExhausted);
}

#[test]
fn augmentation_handles_all_seeds_in_one_budget_and_keeps_reference_edges() {
    let mut graph = graph();
    graph.edges.push(edge("second-call", "root", "middle"));
    let view = view();
    let hits = RankedHits {
        view: view.stamp().clone(),
        hits: vec![
            ranked("root.rs", vec![ByteSpan { start: 0, end: 1 }], 1, "Q1"),
            ranked("middle.rs", vec![ByteSpan { start: 0, end: 1 }], 2, "Q2"),
        ],
    };
    let mode = GraphQueryMode::Augment {
        seeds: 5,
        expansion: TraversalSpec {
            max_hops: 1,
            include_start: true,
            ..Default::default()
        },
    };
    let run = || {
        augment(
            &graph,
            &Evidence { unavailable: false },
            &view,
            &hits,
            &mode,
            &context(BudgetLimits::default()),
        )
        .expect("augmentation")
    };
    let result = run();
    assert_eq!(result.selection.seeds.len(), 2);
    assert_eq!(result.expansions.len(), 2);
    assert_eq!(
        result
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>(),
        ["root", "middle", "other", "leaf"],
        "seed priority and each seed's hop order precede stable IDs"
    );
    assert_eq!(
        result.relations.len(),
        5,
        "two calls, recursion and distinct outgoing targets"
    );
    assert_eq!(result.relations, run().relations);
    assert!(result.execution.usage.edges >= 5);
    assert!(result.execution.usage.output_bytes > 0);
    assert!(
        result
            .evidence
            .iter()
            .all(|e| matches!(e, EvidenceRead::Verified { .. }))
    );
    let error = augment(
        &graph,
        &Evidence { unavailable: true },
        &view,
        &hits,
        &mode,
        &context(BudgetLimits::default()),
    )
    .expect_err("evidence is required");
    assert_eq!(error.kind, QueryErrorKind::ViewMismatch);
    let error = augment(
        &graph,
        &Evidence { unavailable: false },
        &view,
        &hits,
        &mode,
        &context(BudgetLimits {
            edges: 4,
            ..Default::default()
        }),
    )
    .expect_err("seed loops cannot replenish budget");
    assert_eq!(error.kind, QueryErrorKind::ResourceExhausted);
}
fn named(name: &str) -> Locator {
    Locator::Name {
        name: name.into(),
        source_id: None,
        kind: None,
    }
}
fn request(operation: ExploreOperation) -> ExploreRequest {
    ExploreRequest {
        domain: QueryDomain::Code,
        operation,
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    }
}
fn graph() -> Graph {
    Graph {
        nodes: ["root", "middle", "leaf", "other"].map(entity).into(),
        edges: vec![
            edge("a", "root", "middle"),
            edge("b", "middle", "leaf"),
            edge("c", "root", "other"),
            edge("d", "other", "leaf"),
            edge("self", "root", "root"),
        ],
    }
}
fn execute(graph: &Graph, request: &ExploreRequest) -> ExploreResult {
    explore(
        graph,
        &Evidence { unavailable: false },
        &view(),
        request,
        &context(BudgetLimits::default()),
    )
    .expect("explore")
}
#[test]
fn display_filter_does_not_prune_paths_and_boundaries_intersect() {
    let graph = graph();
    let mut traversal = TraversalSpec::default();
    traversal.result_filter.sources = Some(vec!["leaf.rs".into()]);
    let result = execute(
        &graph,
        &request(ExploreOperation::Expand {
            anchor: named("root"),
            traversal: traversal.clone(),
        }),
    );
    assert_eq!(
        result
            .nodes
            .iter()
            .map(|n| (&n.name, n.distance))
            .collect::<Vec<_>>(),
        vec![(&"leaf".to_string(), Some(2))]
    );
    traversal.boundary.sources = Some(vec!["root.rs".into(), "leaf.rs".into()]);
    let result = execute(
        &graph,
        &request(ExploreOperation::Expand {
            anchor: named("root"),
            traversal,
        }),
    );
    assert!(result.nodes.is_empty());
    let mut request = request(ExploreOperation::Expand {
        anchor: named("root"),
        traversal: TraversalSpec {
            boundary: SourceFilter {
                sources: Some(vec!["root.rs".into(), "leaf.rs".into()]),
            },
            ..Default::default()
        },
    });
    request.boundary.sources = Some(vec!["root.rs".into(), "middle.rs".into()]);
    assert!(execute(&graph, &request).nodes.is_empty());
}
#[test]
fn shortest_paths_are_stable_simple_and_respect_minimum_hops() {
    let mut graph = graph();
    graph.edges.push(edge("0-direct", "root", "leaf"));
    let mut traversal = TraversalSpec {
        min_hops: 2,
        max_hops: 3,
        ..Default::default()
    };
    traversal.result_filter.sources = Some(vec!["leaf.rs".into()]);
    let query = request(ExploreOperation::ShortestPath {
        start: named("root"),
        target: named("leaf"),
        traversal,
    });
    let a = execute(&graph, &query);
    let b = execute(&graph, &query);
    assert_eq!(a.paths, b.paths);
    assert_eq!(a.paths[0].relations, vec!["a", "b"]);
    assert_eq!(a.paths[0].nodes.len(), 3);
    let no_path = execute(
        &graph,
        &request(ExploreOperation::ShortestPath {
            start: named("leaf"),
            target: named("root"),
            traversal: TraversalSpec::default(),
        }),
    );
    assert!(no_path.paths.is_empty());
    assert_eq!(no_path.execution.status, Completion::Complete);
}
#[test]
fn expansion_minimum_uses_shortest_distance_and_recursion_keeps_its_edge() {
    let mut graph = graph();
    graph.edges.push(edge("direct", "root", "leaf"));
    let result = execute(
        &graph,
        &request(ExploreOperation::Expand {
            anchor: named("root"),
            traversal: TraversalSpec {
                min_hops: 2,
                ..Default::default()
            },
        }),
    );
    assert!(result.nodes.is_empty());
    let neighbors = execute(
        &graph,
        &request(ExploreOperation::Neighbors {
            anchor: named("root"),
            relations: vec![RelationKind::Calls],
            direction: Direction::Outgoing,
        }),
    );
    assert!(neighbors.relations.iter().any(|e| e.id == "self"));
}
#[test]
fn ambiguity_and_incomplete_location_never_choose_an_anchor() {
    let mut graph = graph();
    let mut second = entity("copy");
    second.name = "root".into();
    graph.nodes.push(second);
    let result = execute(
        &graph,
        &request(ExploreOperation::Neighbors {
            anchor: named("root"),
            relations: vec![RelationKind::Calls],
            direction: Direction::Outgoing,
        }),
    );
    assert_eq!(result.locations[0].status, LocateStatus::Ambiguous);
    assert!(result.relations.is_empty());
    graph.nodes.extend((0..300).map(|i| {
        let mut n = entity(&format!("copy-{i}"));
        n.name = "root".into();
        n
    }));
    let result = explore(
        &graph,
        &Evidence { unavailable: false },
        &view(),
        &request(ExploreOperation::Neighbors {
            anchor: named("root"),
            relations: vec![RelationKind::Calls],
            direction: Direction::Outgoing,
        }),
        &context(BudgetLimits {
            candidates: 100,
            ..Default::default()
        }),
    );
    assert_eq!(
        result.expect_err("incomplete location").kind,
        QueryErrorKind::ResourceExhausted
    );
}

#[test]
fn location_candidates_share_the_node_budget() {
    let mut graph = graph();
    let mut duplicate = entity("copy");
    duplicate.name = "root".into();
    graph.nodes.push(duplicate);
    let result = explore(
        &graph,
        &Evidence { unavailable: false },
        &view(),
        &request(ExploreOperation::Locate {
            target: named("root"),
        }),
        &context(BudgetLimits {
            nodes: 1,
            ..Default::default()
        }),
    );
    assert_eq!(
        result.expect_err("two candidates exceed one node").kind,
        QueryErrorKind::ResourceExhausted
    );
    let result = execute(
        &graph,
        &request(ExploreOperation::Locate {
            target: named("root"),
        }),
    );
    assert_eq!(result.locations[0].status, LocateStatus::Ambiguous);
    assert_eq!(result.execution.usage.nodes, 2);
}
#[test]
fn unavailable_evidence_and_exhausted_path_search_are_errors() {
    let graph = graph();
    let query = request(ExploreOperation::ShortestPath {
        start: named("root"),
        target: named("leaf"),
        traversal: TraversalSpec::default(),
    });
    assert_eq!(
        explore(
            &graph,
            &Evidence { unavailable: true },
            &view(),
            &query,
            &context(BudgetLimits::default())
        )
        .expect_err("source drift")
        .kind,
        QueryErrorKind::ViewMismatch
    );
    assert_eq!(
        explore(
            &graph,
            &Evidence { unavailable: false },
            &view(),
            &query,
            &context(BudgetLimits {
                edges: 1,
                ..Default::default()
            })
        )
        .expect_err("not no-path")
        .kind,
        QueryErrorKind::ResourceExhausted
    );
}
#[test]
fn truncation_is_explicit_and_retains_only_cited_evidence() {
    let graph = graph();
    let query = request(ExploreOperation::References {
        selector: ReferenceSelector::Source {
            source_id: "root.rs".into(),
        },
    });
    let complete = execute(&graph, &query);
    let limit = complete.execution.usage.output_bytes * 2 / 3;
    let partial = explore(
        &graph,
        &Evidence { unavailable: false },
        &view(),
        &query,
        &context(BudgetLimits {
            output_bytes: limit,
            ..Default::default()
        }),
    )
    .expect("bounded output");
    assert_eq!(partial.execution.status, Completion::Partial);
    assert!(partial.execution.usage.output_bytes <= limit);
    let returned = partial
        .references
        .iter()
        .map(|r| r.id.as_str())
        .collect::<Vec<_>>();
    assert!(partial.evidence.iter().all(|e| {
        match e {
            EvidenceRead::Verified { reference, .. }
            | EvidenceRead::Unavailable { reference, .. } => reference
                .reference_id
                .as_deref()
                .is_none_or(|id| returned.contains(&id)),
        }
    }));
}

#[test]
fn positive_hop_paths_do_not_return_zero_hop_results() {
    let graph = graph();
    let result = execute(
        &graph,
        &request(ExploreOperation::ShortestPath {
            start: named("root"),
            target: named("root"),
            traversal: TraversalSpec {
                include_start: true,
                ..Default::default()
            },
        }),
    );
    assert!(result.paths.is_empty());
}

struct ScopeText {
    crossing: bool,
}
impl ScopeTextProvider for ScopeText {
    fn text_entities(
        &self,
        ids: &[String],
        ctx: &RequestContext,
    ) -> QueryResult<Vec<ScopeTextEntity>> {
        ctx.check()?;
        Ok(ids
            .iter()
            .map(|id| ScopeTextEntity {
                id: id.clone(),
                content_bytes: 26,
                fragments: if self.crossing {
                    vec![ScopeTextFragment {
                        id: format!("{id}:all"),
                        span: ByteSpan { start: 0, end: 26 },
                    }]
                } else {
                    vec![
                        ScopeTextFragment {
                            id: format!("{id}:safe"),
                            span: ByteSpan { start: 0, end: 1 },
                        },
                        ScopeTextFragment {
                            id: format!("{id}:rest"),
                            span: ByteSpan { start: 1, end: 26 },
                        },
                    ]
                },
            })
            .collect())
    }
}
#[test]
fn scope_resolution_is_complete_unique_and_rejects_unrepresentable_ranges() {
    let graph = graph();
    let view = view();
    let spec = ScopeSpec {
        anchor: Locator::Name {
            name: "root".into(),
            source_id: None,
            kind: None,
        },
        traversal: TraversalSpec {
            max_hops: 1,
            ..Default::default()
        },
        region: BodyRegion::Full,
    };
    let resolved = resolve_scope(
        &graph,
        &ScopeText { crossing: false },
        &view,
        &spec,
        &context(BudgetLimits::default()),
    )
    .expect("scope");
    resolved
        .scope
        .validate_for(view.stamp())
        .expect("same view");
    assert_eq!(resolved.scope.units().len(), 2);
    assert!(
        resolved
            .scope
            .units()
            .iter()
            .all(|u| u.source_spans == [ByteSpan { start: 0, end: 1 }])
    );
    assert_eq!(
        resolve_scope(
            &graph,
            &ScopeText { crossing: true },
            &view,
            &spec,
            &context(BudgetLimits::default())
        )
        .expect_err("crossing")
        .reason,
        "scope_not_representable"
    );
    let mut missing = spec.clone();
    missing.anchor = Locator::Name {
        name: "missing".into(),
        source_id: None,
        kind: None,
    };
    assert_eq!(
        resolve_scope(
            &graph,
            &ScopeText { crossing: false },
            &view,
            &missing,
            &context(BudgetLimits::default())
        )
        .expect_err("missing anchor")
        .reason,
        "scope_anchor_not_unique"
    );
    let mut empty = spec;
    empty.traversal.result_filter.sources = Some(vec![]);
    assert!(
        resolve_scope(
            &graph,
            &ScopeText { crossing: false },
            &view,
            &empty,
            &context(BudgetLimits::default())
        )
        .expect("complete empty")
        .scope
        .is_empty()
    );
}
