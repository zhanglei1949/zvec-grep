use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use zg_knowledge::query::*;

fn view() -> ViewStamp {
    ViewStamp {
        workspace_id: "workspace".into(),
        generation: "generation".into(),
        text_revision: 1,
        graph: Some(GraphStamp {
            build_id: "build".into(),
            schema: "schema".into(),
            rules: "rules".into(),
            configuration: "config".into(),
            catalog_revision: "catalog".into(),
        }),
    }
}
fn context(limits: BudgetLimits) -> RequestContext {
    RequestContext::new(
        limits,
        CancellationToken::new(),
        Instant::now() + Duration::from_secs(30),
    )
    .expect("context")
}
fn augment() -> GraphQueryMode {
    GraphQueryMode::Augment {
        seeds: 5,
        expansion: TraversalSpec::default(),
    }
}

#[test]
fn malformed_requests_cannot_be_interpreted_as_off_or_empty_scope() {
    for invalid in [
        json!({"mode":"constrain"}),
        json!({"mode":"off","scope":{}}),
        json!({"mode":"augment","seeds":-1}),
        json!({"mode":"automatic"}),
        json!({"mode":"constrain","scope":{"anchor":{"by":"cypher","query":"MATCH"}}}),
    ] {
        assert!(serde_json::from_value::<GraphQueryMode>(invalid).is_err());
    }
    let valid: GraphQueryMode =
        serde_json::from_value(json!({"mode":"augment"})).expect("defaults");
    assert_eq!(valid, augment());
    valid.validate().expect("valid augmentation");
    let invalid = GraphQueryMode::Augment {
        seeds: 0,
        expansion: TraversalSpec::default(),
    };
    assert_eq!(
        invalid.validate().expect_err("zero seed count").kind,
        QueryErrorKind::InvalidInput
    );
    assert!(
        GraphQueryMode::Augment {
            seeds: 6,
            expansion: TraversalSpec::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn traversal_and_source_validation_preserve_path_boundaries() {
    let invalid = [
        TraversalSpec {
            min_hops: 0,
            ..TraversalSpec::default()
        },
        TraversalSpec {
            min_hops: 3,
            max_hops: 2,
            ..TraversalSpec::default()
        },
        TraversalSpec {
            max_hops: 9,
            ..TraversalSpec::default()
        },
        TraversalSpec {
            relations: vec![],
            ..TraversalSpec::default()
        },
        TraversalSpec {
            relations: vec![RelationKind::Calls; 2],
            ..TraversalSpec::default()
        },
    ];
    for spec in invalid {
        assert!(spec.validate().is_err());
    }
    for path in ["../x.rs", "a/../x.rs", "/x.rs", "a\\b", "C:/x", "a//b", " "] {
        assert!(
            Locator::Position {
                source_id: path.into(),
                byte_offset: 0,
                kind: None,
            }
            .validate()
            .is_err()
        );
    }
    let explicit_empty = SourceFilter {
        sources: Some(vec![]),
    };
    explicit_empty.validate().expect("valid empty boundary");
    assert_ne!(explicit_empty, SourceFilter::default());
    let locator = Locator::Name {
        name: "validate".into(),
        source_id: Some("src/a.rs".into()),
        kind: None,
    };
    let request = ExploreRequest {
        domain: QueryDomain::Code,
        operation: ExploreOperation::ShortestPath {
            start: locator.clone(),
            target: locator,
            traversal: TraversalSpec::default(),
        },
        boundary: explicit_empty,
        result_filter: SourceFilter::default(),
    };
    request
        .validate()
        .expect("same endpoint can be a recursive path query");
}

#[test]
fn all_view_components_participate_in_binding() {
    let original = view();
    for field in 0..8 {
        let mut different = original.clone();
        let graph = different.graph.as_mut().expect("graph");
        match field {
            0 => different.workspace_id.push('x'),
            1 => different.generation.push('x'),
            2 => different.text_revision += 1,
            3 => graph.build_id.push('x'),
            4 => graph.schema.push('x'),
            5 => graph.rules.push('x'),
            6 => graph.configuration.push('x'),
            _ => graph.catalog_revision.push('x'),
        }
        assert_eq!(
            original
                .require_same(&different)
                .expect_err("different view")
                .kind,
            QueryErrorKind::ViewMismatch
        );
    }
    let stale = NodeRef {
        graph_build_id: "old".into(),
        node_id: "n".into(),
    };
    let request = ExploreRequest {
        domain: QueryDomain::Code,
        operation: ExploreOperation::Locate {
            target: Locator::Node { node: stale },
        },
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    };
    assert_eq!(
        request.validate_for(&original).expect_err("old node").kind,
        QueryErrorKind::ViewMismatch
    );
    let mut old = original.clone();
    old.text_revision += 1;
    let references = ExploreRequest {
        operation: ExploreOperation::References {
            selector: ReferenceSelector::Id {
                reference_id: "r1".into(),
                view: old,
            },
        },
        ..request
    };
    assert_eq!(
        references
            .validate_for(&original)
            .expect_err("old reference")
            .kind,
        QueryErrorKind::ViewMismatch
    );
}

#[test]
fn ranked_hits_keep_real_identities_and_all_query_provenance() {
    let mut hits = RankedHits {
        view: view(),
        hits: vec![RankedHit {
            source_id: "src/a.rs".into(),
            entity_id: "entity".into(),
            fragment_id: "fragment".into(),
            source_spans: vec![ByteSpan { start: 0, end: 10 }],
            priority: 1,
            query_sources: vec!["Q1".into(), "Q2".into()],
        }],
    };
    hits.validate_for(&view()).expect("bound");
    hits.hits[0]
        .source_spans
        .push(ByteSpan { start: 20, end: 30 });
    hits.validate_for(&view())
        .expect("non-contiguous source mapping");
    for spans in [
        vec![],
        vec![ByteSpan { start: 1, end: 1 }],
        vec![
            ByteSpan { start: 10, end: 20 },
            ByteSpan { start: 0, end: 5 },
        ],
        vec![
            ByteSpan { start: 0, end: 10 },
            ByteSpan { start: 9, end: 20 },
        ],
    ] {
        let mut invalid = hits.clone();
        invalid.hits[0].source_spans = spans;
        assert!(invalid.validate_for(&view()).is_err());
    }
    hits.hits[0].query_sources.clear();
    assert!(hits.validate_for(&view()).is_err());
    hits.hits[0].query_sources.push("Q1".into());
    hits.hits[0].fragment_id.clear();
    assert!(hits.validate_for(&view()).is_err());
}

#[test]
fn evidence_ranges_are_decoded_utf8_not_raw_byte_offsets() {
    assert_eq!(
        ByteSpan { start: 1, end: 4 }
            .slice("a中b")
            .expect("CJK boundary"),
        "中"
    );
    for span in [
        ByteSpan { start: 2, end: 4 },
        ByteSpan { start: 0, end: 99 },
        ByteSpan { start: 4, end: 1 },
    ] {
        assert!(span.slice("a中b").is_err());
    }
    let mut evidence = EvidenceRef {
        source_id: "a.rs".into(),
        raw_digest: "a".repeat(64),
        decoded_digest: "b".repeat(64),
        span: ByteSpan { start: 1, end: 4 },
        reference_id: Some("r1".into()),
    };
    evidence.validate().expect("evidence shape");
    evidence.decoded_digest = "bad".into();
    assert!(evidence.validate().is_err());
}

#[test]
fn required_optional_and_text_only_views_retain_resource_lifetimes() {
    struct Lease(Arc<AtomicUsize>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut text = view();
    text.graph = None;
    assert!(
        KgReadView::new(
            text.clone(),
            (),
            &ReadViewRequest {
                graph: GraphRequirement::Required,
                needs: Some(code_needs()),
            },
            vec![],
            None
        )
        .is_err()
    );
    assert!(
        KgReadView::new(
            text.clone(),
            (),
            &ReadViewRequest {
                graph: GraphRequirement::Optional,
                needs: Some(code_needs()),
            },
            vec![],
            None
        )
        .is_err()
    );
    assert!(
        KgReadView::new(
            view(),
            (),
            &ReadViewRequest {
                graph: GraphRequirement::None,
                needs: None,
            },
            vec![],
            None
        )
        .is_err()
    );
    let lease = KgReadView::new(
        text,
        Lease(Arc::clone(&dropped)),
        &ReadViewRequest {
            graph: GraphRequirement::Optional,
            needs: Some(code_needs()),
        },
        vec![],
        Some("not_configured".into()),
    )
    .expect("optional");
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert_eq!(lease.unavailable_reason(), Some("not_configured"));
    drop(lease);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

fn code_needs() -> GraphReadNeeds {
    GraphReadNeeds {
        domain: QueryDomain::Code,
        relations: vec![RelationKind::Calls],
        text_mapping: false,
    }
}

#[test]
fn ready_documents_cannot_authorize_code_or_unavailable_text_mapping() {
    let request = ReadViewRequest {
        graph: GraphRequirement::Required,
        needs: Some(code_needs()),
    };
    let documents = GraphReadCapabilities {
        domain: QueryDomain::Documents,
        relations: vec![RelationKind::Links],
        text_mapping: true,
    };
    assert_eq!(
        KgReadView::new(view(), (), &request, vec![documents], None)
            .expect_err("documents do not authorize callers")
            .kind,
        QueryErrorKind::GraphUnavailable
    );
    let code = GraphReadCapabilities {
        domain: QueryDomain::Code,
        relations: vec![RelationKind::Calls],
        text_mapping: false,
    };
    let lease =
        KgReadView::new(view(), (), &request, vec![code.clone()], None).expect("ready code calls");
    lease.require(&code_needs()).expect("same capability");
    let mapped = GraphReadNeeds {
        text_mapping: true,
        ..code_needs()
    };
    assert!(lease.require(&mapped).is_err());
    assert!(
        lease
            .require(&GraphReadNeeds {
                relations: vec![RelationKind::Imports],
                ..code_needs()
            })
            .is_err()
    );
    let mapped_request = ReadViewRequest {
        needs: Some(mapped),
        ..request
    };
    assert!(KgReadView::new(view(), (), &mapped_request, vec![code], None).is_err());
}

#[test]
fn read_requirements_reject_implicit_domains_and_hidden_graph_needs() {
    for request in [
        ReadViewRequest {
            graph: GraphRequirement::Required,
            needs: None,
        },
        ReadViewRequest {
            graph: GraphRequirement::Optional,
            needs: None,
        },
        ReadViewRequest {
            graph: GraphRequirement::None,
            needs: Some(code_needs()),
        },
        ReadViewRequest {
            graph: GraphRequirement::Required,
            needs: Some(GraphReadNeeds {
                relations: vec![RelationKind::Calls; 2],
                ..code_needs()
            }),
        },
    ] {
        assert!(request.validate().is_err());
    }
    let mut text = view();
    text.graph = None;
    let optional = ReadViewRequest {
        graph: GraphRequirement::Optional,
        needs: Some(code_needs()),
    };
    KgReadView::new(text, (), &optional, vec![], Some("code_not_ready".into()))
        .expect("provider may explicitly return a safe text view");
}

#[test]
fn concurrent_groups_share_budget_and_failed_charge_is_atomic() {
    let ctx = context(BudgetLimits {
        nodes: 20,
        ..BudgetLimits::default()
    });
    std::thread::scope(|scope| {
        let workers = (0..8)
            .map(|_| {
                let ctx = ctx.child(Duration::from_secs(5)).expect("child");
                scope.spawn(move || {
                    (0..20)
                        .filter(|_| {
                            ctx.charge(BudgetUsage {
                                nodes: 1,
                                ..BudgetUsage::default()
                            })
                            .is_ok()
                        })
                        .count()
                })
            })
            .collect::<Vec<_>>();
        let total: usize = workers.into_iter().map(|w| w.join().expect("worker")).sum();
        assert_eq!(total, 20);
    });
    assert!(
        ctx.charge(BudgetUsage {
            edges: 1,
            nodes: 1,
            ..BudgetUsage::default()
        })
        .is_err()
    );
    assert_eq!(ctx.usage().expect("usage").edges, 0);
    assert_eq!(ctx.usage().expect("usage").nodes, 20);
}

#[test]
fn budgets_reject_overflow_and_zero_limits() {
    assert!(
        BudgetLimits {
            output_bytes: 0,
            ..BudgetLimits::default()
        }
        .validate()
        .is_err()
    );
    let ctx = context(BudgetLimits {
        output_bytes: u64::MAX,
        ..BudgetLimits::default()
    });
    ctx.charge(BudgetUsage {
        output_bytes: u64::MAX,
        ..BudgetUsage::default()
    })
    .expect("exact limit");
    assert_eq!(
        ctx.charge(BudgetUsage {
            output_bytes: 1,
            ..BudgetUsage::default()
        })
        .expect_err("overflow")
        .kind,
        QueryErrorKind::ResourceExhausted
    );
    assert_eq!(ctx.usage().expect("usage").output_bytes, u64::MAX);
}

#[test]
fn only_stage_stops_can_degrade_and_cancellation_always_wins() {
    let token = CancellationToken::new();
    let ctx = RequestContext::new(
        BudgetLimits::default(),
        token.clone(),
        Instant::now() + Duration::from_secs(30),
    )
    .expect("ctx");
    let child = ctx.child(Duration::from_millis(1)).expect("child");
    std::thread::sleep(Duration::from_millis(3));
    let error = child.check().expect_err("stage timeout");
    assert_eq!(error.kind, QueryErrorKind::StageDeadline);
    assert!(ctx.may_degrade(&augment(), true, &error));
    assert!(!ctx.may_degrade(&GraphQueryMode::Off {}, true, &error));
    let strict = GraphQueryMode::Constrain {
        scope: ScopeSpec {
            anchor: Locator::Name {
                name: "validate".into(),
                source_id: None,
                kind: None,
            },
            traversal: TraversalSpec::default(),
            region: BodyRegion::Full,
        },
    };
    assert!(!ctx.may_degrade(&strict, true, &error));
    assert!(!ctx.may_degrade(&augment(), false, &error));
    token.cancel();
    assert!(!ctx.may_degrade(&augment(), true, &error));
    assert_eq!(
        child.check().expect_err("cancelled").kind,
        QueryErrorKind::Cancelled
    );
    let expired = RequestContext::new(
        BudgetLimits::default(),
        CancellationToken::new(),
        Instant::now(),
    )
    .expect("expired");
    let error = expired.check().expect_err("root timeout");
    assert_eq!(error.kind, QueryErrorKind::RequestDeadline);
    assert!(!expired.may_degrade(&augment(), true, &error));
}

#[test]
fn execution_limits_can_be_tightened_but_not_raised_by_a_caller() {
    let ceiling = ExecutionOptions::default();
    let smaller = ExecutionOptions {
        request_timeout_ms: 1000,
        ..ceiling
    };
    smaller
        .validate_within(ceiling)
        .expect("shorter than graph stage is clamped at runtime");
    assert!(
        ExecutionOptions {
            graph_timeout_ms: 0,
            ..ceiling
        }
        .validate_within(ceiling)
        .is_err()
    );
    assert!(
        ExecutionOptions {
            budget: BudgetLimits {
                edges: 10_001,
                ..ceiling.budget
            },
            ..ceiling
        }
        .validate_within(ceiling)
        .is_err()
    );
    assert!(
        ExecutionOptions {
            request_timeout_ms: 30_001,
            ..ceiling
        }
        .validate_within(ceiling)
        .is_err()
    );
}

#[test]
fn child_stage_cannot_extend_deadline_or_cancel_siblings() {
    let parent_token = CancellationToken::new();
    let parent = RequestContext::new(
        BudgetLimits::default(),
        parent_token.clone(),
        Instant::now() + Duration::from_secs(1),
    )
    .expect("parent");
    let child = parent
        .child(Duration::from_secs(30))
        .expect("clamped child");
    assert_eq!(parent.deadline(), child.deadline());
    let worker = child.worker_cancellation();
    worker.cancel();
    parent.check().expect("worker stop is isolated");
    let sibling = parent.worker_cancellation();
    assert!(!sibling.is_cancelled());
    parent_token.cancel();
    assert!(sibling.is_cancelled());
}

#[test]
fn piecewise_text_mapping_checks_the_actual_decoded_bytes() {
    let source = "中 gap 😀\r\n";
    let content = "中😀";
    for (source_span, content_span) in [
        (ByteSpan { start: 0, end: 3 }, ByteSpan { start: 0, end: 3 }),
        (
            ByteSpan { start: 8, end: 12 },
            ByteSpan { start: 3, end: 7 },
        ),
    ] {
        TextMap {
            source_id: "code.py".into(),
            source_span,
            entity_id: "joined".into(),
            content_span,
        }
        .validate_text(source, content)
        .expect("non-contiguous source segments");
    }
    let mapping = TextMap {
        source_id: "guide.md".into(),
        source_span: ByteSpan { start: 1, end: 4 },
        entity_id: "entity".into(),
        content_span: ByteSpan { start: 0, end: 3 },
    };
    mapping
        .validate_text("x中y", "中")
        .expect("shifted segment");
    assert!(mapping.validate_text("x中y", "文").is_err());
    let broken = TextMap {
        content_span: ByteSpan { start: 0, end: 2 },
        ..mapping
    };
    assert!(broken.validate().is_err());
    let empty = TextMap {
        source_span: ByteSpan { start: 0, end: 0 },
        content_span: ByteSpan { start: 0, end: 0 },
        ..broken
    };
    assert!(
        empty.validate().is_err(),
        "no body is represented by no map, not an empty segment"
    );
}
