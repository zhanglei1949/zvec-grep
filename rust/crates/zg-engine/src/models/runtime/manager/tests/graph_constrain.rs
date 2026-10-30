//! Exact scope membership is tested against the real FTS/vector collections.
use super::*;
use super::{graph_augment::build_code, graph_indexing::models};
use crate::{
    api::context::{
        ContextOptions,
        graph::{BodyRegion, GraphQueryMode, Locator, ScopeSpec, TraversalSpec},
        options::{ContextRoute, ContextRouteMode, RefreshPolicy},
    },
    pipelines::indexed_search,
};

const CODE: &str = "fn target() { let marker = \"scopeword\"; }\r\nfn caller() { target(); }\r\nfn outside() { let marker = \"scopeword scopeword scopeword\"; }\r\n";
fn options(root: &std::path::Path) -> ContextOptions {
    ContextOptions {
        root: Some(root.into()),
        auto_update: false,
        refresh: Some(RefreshPolicy::Off),
        allow_remote: true,
        prefer_symbol: true,
        routes: vec![ContextRoute {
            mode: ContextRouteMode::Fts,
            query: "scopeword".into(),
        }],
        graph: GraphQueryMode::Constrain {
            scope: ScopeSpec {
                anchor: Locator::Name {
                    name: "caller".into(),
                    source_id: None,
                    kind: None,
                },
                traversal: TraversalSpec {
                    max_hops: 1,
                    ..Default::default()
                },
                region: BodyRegion::Full,
            },
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn graph_constrain_all_routes_and_groups_stay_inside_scope() {
    let dir = tempfile::tempdir().expect("root");
    let calls = Arc::new(AtomicUsize::new(0));
    let models = models(&calls);
    let service = build_code(dir.path(), &models, true, CODE).await;
    for fuse in [false, true] {
        let mut opts = options(dir.path());
        opts.fuse = fuse;
        opts.queries = vec!["scopeword".into(), "outside".into()];
        opts.routes.push(ContextRoute {
            mode: ContextRouteMode::Vector,
            query: "outside".into(),
        });
        let result = indexed_search::service::context(&service, &models, &opts, None, false)
            .await
            .expect("strict query");
        assert!(!result.items.is_empty());
        for item in result
            .items
            .iter()
            .chain(result.group_results.iter().flat_map(|g| &g.items))
        {
            assert!(item.content.contains("fn target()"), "{}", item.content);
            assert!(!item.content.contains("outside"));
            assert!(item.outline.is_none());
            assert!(item.container.is_none());
        }
        let graph = result.graph.expect("scope");
        assert!(graph.scope.expect("scope").exact_filter);
        assert!(graph.nodes.iter().all(|n| n.name == "target"));
    }
    let mut lexical = options(dir.path());
    lexical.routes[0].query = "outside".into();
    let result = indexed_search::service::context(&service, &models, &lexical, None, false)
        .await
        .expect("no in-scope lexical match");
    assert!(result.items.is_empty());
    assert!(
        result
            .graph
            .expect("graph")
            .scope
            .expect("scope")
            .filtered_fragment_count
            > 0
    );
}

#[tokio::test]
async fn graph_constrain_empty_scope_and_user_intersection_skip_models_and_consent() {
    let dir = tempfile::tempdir().expect("root");
    let calls = Arc::new(AtomicUsize::new(0));
    let models = models(&calls);
    let service = build_code(dir.path(), &models, true, CODE).await;
    let before = calls.load(Ordering::Acquire);
    for user_empty in [false, true] {
        let mut opts = options(dir.path());
        opts.allow_remote = false;
        opts.queries = vec!["scopeword".into()];
        if user_empty {
            opts.filter.globs = vec!["missing/**".into()];
        } else if let GraphQueryMode::Constrain { scope } = &mut opts.graph {
            scope.anchor = Locator::Name {
                name: "target".into(),
                source_id: None,
                kind: None,
            };
        }
        assert!(
            crate::authorization::query_authorizations(&opts)
                .expect("scope consent")
                .is_empty()
        );
        let engine = crate::ZvecGrep::new();
        let result = engine
            .context(opts.clone())
            .await
            .expect("public empty scope");
        assert!(result.items.is_empty());
        assert!(result.group_results.iter().all(|g| g.items.is_empty()));
        assert_eq!(engine.runtime_snapshot().loaded_models, 0);
        engine.close();
        let result = indexed_search::service::context(&service, &models, &opts, None, false)
            .await
            .expect("empty scope");
        let scope = result.graph.expect("graph").scope.expect("scope");
        assert_eq!(scope.filtered_fragment_count, 0);
        assert!(scope.empty_reason.is_some());
    }
    assert_eq!(calls.load(Ordering::Acquire), before);
}

#[tokio::test]
async fn graph_constrain_missing_graph_ambiguity_budget_and_cancel_never_degrade() {
    let dir = tempfile::tempdir().expect("root");
    let calls = Arc::new(AtomicUsize::new(0));
    let models = models(&calls);
    let service = build_code(dir.path(), &models, false, CODE).await;
    let error =
        indexed_search::service::context(&service, &models, &options(dir.path()), None, false)
            .await
            .expect_err("required graph");
    assert_eq!(error.code(), crate::EngineError::RESOURCE_BUSY);
    drop(service);
    let valid = tempfile::tempdir().expect("valid root");
    let service = build_code(valid.path(), &models, true, CODE).await;
    let before = calls.load(Ordering::Acquire);
    let mut filter_limited = options(valid.path());
    filter_limited.queries = vec!["scopeword".into()];
    filter_limited.graph_execution = Some(crate::api::explore::ExecutionOptions {
        budget: crate::api::explore::BudgetLimits {
            filter_bytes: 1,
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(
        indexed_search::service::context(&service, &models, &filter_limited, None, false)
            .await
            .expect_err("scope filter budget before model")
            .code(),
        crate::EngineError::RESOURCE_BUSY
    );
    let mut opts = options(valid.path());
    if let GraphQueryMode::Constrain { scope } = &mut opts.graph {
        scope.anchor = Locator::Name {
            name: "missing".into(),
            source_id: None,
            kind: None,
        };
    }
    assert!(
        indexed_search::service::context(&service, &models, &opts, None, false)
            .await
            .is_err()
    );
    opts = options(valid.path());
    opts.graph_execution = Some(crate::api::explore::ExecutionOptions {
        budget: crate::api::explore::BudgetLimits {
            edges: 1,
            ..Default::default()
        },
        ..Default::default()
    });
    if let GraphQueryMode::Constrain { scope } = &mut opts.graph {
        scope.traversal.relations = vec![crate::api::context::graph::RelationKind::Contains];
        scope.anchor = Locator::Position {
            source_id: "src/lib.rs".into(),
            byte_offset: 0,
            kind: Some(crate::api::context::graph::EntityKind::File),
        };
    }
    assert!(
        indexed_search::service::context(&service, &models, &opts, None, false)
            .await
            .is_err()
    );
    opts = options(valid.path());
    let signal = tokio_util::sync::CancellationToken::new();
    signal.cancel();
    opts.signal = Some(signal);
    assert_eq!(
        indexed_search::service::context(&service, &models, &opts, None, false)
            .await
            .expect_err("cancel")
            .code(),
        crate::EngineError::CANCELLED
    );
    assert_eq!(
        calls.load(Ordering::Acquire),
        before,
        "queries must not embed"
    );
}

#[tokio::test]
async fn graph_constrain_own_region_rejects_unrepresentable_crossing_fragments() {
    let dir = tempfile::tempdir().expect("root");
    let calls = Arc::new(AtomicUsize::new(0));
    let models = models(&calls);
    let service = build_code(
        dir.path(),
        &models,
        true,
        "fn parent() { fn child() {} child(); }\n",
    )
    .await;
    let mut opts = options(dir.path());
    if let GraphQueryMode::Constrain { scope } = &mut opts.graph {
        scope.anchor = Locator::Position {
            source_id: "src/lib.rs".into(),
            byte_offset: 0,
            kind: Some(crate::api::context::graph::EntityKind::File),
        };
        scope.traversal.relations = vec![crate::api::context::graph::RelationKind::Contains];
        scope.traversal.include_start = false;
        scope.traversal.max_hops = 1;
        scope.region = BodyRegion::Own;
        scope.traversal.result_filter.sources = Some(vec!["src/lib.rs".into()]);
    }
    let before = calls.load(Ordering::Acquire);
    let error = indexed_search::service::context(&service, &models, &opts, None, false)
        .await
        .expect_err("cannot expand a crossing fragment");
    assert_eq!(error.code(), crate::EngineError::UNSUPPORTED, "{error}");
    assert!(error.message().contains("scope_not_representable"));
    assert_eq!(calls.load(Ordering::Acquire), before);
}

struct WaitingModel {
    info: EmbeddingModelInfo,
    entered: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl EmbeddingModel for WaitingModel {
    fn info(&self) -> &EmbeddingModelInfo {
        &self.info
    }
    async fn embed(
        &self,
        _: &[Vec<Content>],
        options: EmbeddingOptions,
    ) -> Result<EmbeddingResult, ModelError> {
        self.entered.notify_one();
        options
            .signal
            .expect("query cancellation")
            .cancelled()
            .await;
        Err(ModelError::cancelled("query cancelled"))
    }
}

#[tokio::test]
async fn graph_constrain_cancellation_keeps_other_reader_safe_and_releases_admission() {
    use crate::workspace::lock::{LockMode, acquire_read_write_lock};
    let dir = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let builder = models(&calls);
    let service = Arc::new(build_code(dir.path(), &builder, true, CODE).await);
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let manager = ModelRuntimeManager::with_factory(move |_, _, _| {
        Ok(Arc::new(WaitingModel {
            info: EmbeddingModelInfo {
                model: crate::domain::model::ModelInfo {
                    provider: "qwen".into(),
                    name: "text-embedding-v4".into(),
                    endpoint: Some("https://example.invalid/v1".into()),
                },
                dimension: 2,
                metric: Metric::Cosine,
                max_batch_size: 8,
                max_input_tokens: Some(64),
                max_image_bytes: None,
            },
            entered: notify.clone(),
        }))
    });
    let mut opts = options(dir.path());
    opts.queries = vec!["scopeword".into()];
    let signal = tokio_util::sync::CancellationToken::new();
    opts.signal = Some(signal.clone());
    let query_service = service.clone();
    let task = tokio::spawn(async move {
        indexed_search::service::context(&query_service, &manager, &opts, None, false).await
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .expect("query started after scope");
    let lock = dir.path().join(".zvec-grep/locks/home");
    assert!(acquire_read_write_lock(&lock, LockMode::Write, "test writer").is_err());
    let mut ordinary = options(dir.path());
    ordinary.graph = GraphQueryMode::Off {};
    let other = indexed_search::service::context(&service, &builder, &ordinary, None, false)
        .await
        .expect("independent reader");
    assert!(!other.items.is_empty());
    signal.cancel();
    let error = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("worker exit")
        .expect("join")
        .expect_err("cancel");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
    drop(
        acquire_read_write_lock(&lock, LockMode::Write, "test writer after query")
            .expect("admission released"),
    );
}
