//! Native retrieval and graph augmentation use the same published text view.
use super::graph_indexing::{models, options, policy};
use super::*;
use crate::{
    api::context::{
        ContextOptions,
        graph::{GraphQueryMode, TraversalSpec},
        graph_result::{GraphEvidence, GraphStatus},
        options::{ContextRoute, ContextRouteMode, RefreshPolicy},
    },
    pipelines::{indexed_search, indexing::service::WorkspaceIndexService},
    storage::read_session::ReadSessionCache,
};

fn query_options(root: &std::path::Path) -> ContextOptions {
    ContextOptions {
        root: Some(root.into()),
        refresh: Some(RefreshPolicy::Off),
        auto_update: false,
        routes: vec![ContextRoute {
            mode: ContextRouteMode::Fts,
            query: "caller".into(),
        }],
        graph: GraphQueryMode::Augment {
            seeds: 5,
            expansion: TraversalSpec {
                max_hops: 1,
                ..Default::default()
            },
        },
        ..Default::default()
    }
}

async fn build(root: &std::path::Path, manager: &ModelRuntimeManager, graph: bool) {
    drop(build_code(root, manager, graph, "fn leaf() {}\r\nfn target() { leaf(); }\r\nfn caller() { target(); target(); caller(); }\r\n").await);
}

pub(super) async fn build_code(
    root: &std::path::Path,
    manager: &ModelRuntimeManager,
    graph: bool,
    code: &str,
) -> WorkspaceIndexService {
    std::fs::create_dir(root.join("src")).expect("src");
    std::fs::write(root.join("src/lib.rs"), code).expect("code");
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = options(root);
    opts.graph = graph.then(policy);
    service.index(manager, opts).await.expect("index");
    service
}

#[tokio::test]
async fn graph_augment_preserves_normal_items_and_references_existing_text() {
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    build(directory.path(), &manager, true).await;
    let service = WorkspaceIndexService::with_test_registry();
    let opts = query_options(directory.path());
    let mut off = opts.clone();
    off.graph = GraphQueryMode::Off {};
    let ordinary = indexed_search::service::context(&service, &manager, &off, None, false)
        .await
        .expect("ordinary retrieval");
    let before = calls.load(Ordering::Acquire);
    let cache = ReadSessionCache::new(Duration::from_secs(10)).expect("cache");
    for cache in [None, Some(&cache)] {
        let result = indexed_search::service::context(&service, &manager, &opts, cache, false)
            .await
            .expect("augmentation");
        assert_eq!(result.items, ordinary.items);
        assert_eq!(result.group_results, ordinary.group_results);
        let graph = result.graph.expect("graph supplement");
        assert_eq!(graph.status, GraphStatus::Complete);
        assert!(graph.view.is_some());
        assert_eq!(graph.seeds.len(), 1);
        assert_eq!(graph.relations.len(), 2, "keep both call occurrences");
        assert!(graph.nodes.iter().any(|n| n.name == "target"));
        assert!(
            graph
                .evidence
                .iter()
                .any(|e| matches!(e, GraphEvidence::Retrieved { .. }))
        );
        assert!(
            graph
                .evidence
                .iter()
                .any(|e| matches!(e, GraphEvidence::Verified { .. }))
        );
        assert_eq!(graph.seeds[0].hits[0].query_sources, ["Q1"]);
    }
    cache.close();
    assert_public_augment(&opts, &ordinary.items).await;
    assert_eq!(
        calls.load(Ordering::Acquire),
        before,
        "FTS and graph do not embed"
    );
}

async fn assert_public_augment(
    opts: &ContextOptions,
    ordinary: &[crate::api::context::result::ContextItem],
) {
    let engine = crate::ZvecGrep::new();
    let result = engine.context(opts.clone()).await.expect("public API");
    assert_eq!(result.items, ordinary);
    assert_eq!(result.graph.expect("graph").status, GraphStatus::Complete);
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}

#[tokio::test]
async fn graph_augment_missing_graph_degrades_but_cancelled_text_request_fails() {
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    build(directory.path(), &manager, false).await;
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = query_options(directory.path());
    let result = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("safe text fallback");
    assert!(!result.items.is_empty());
    let graph = result.graph.expect("explicit degradation");
    assert_eq!(graph.status, GraphStatus::Degraded);
    assert!(graph.view.is_none());
    let signal = tokio_util::sync::CancellationToken::new();
    signal.cancel();
    opts.signal = Some(signal);
    let error = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect_err("request cancellation is not degradation");
    assert_eq!(error.code(), crate::EngineError::CANCELLED);
}

#[tokio::test]
async fn graph_augment_two_hops_contains_and_shared_budget_are_bounded() {
    use crate::api::context::graph::{Direction, RelationKind};
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    build(directory.path(), &manager, true).await;
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = query_options(directory.path());
    opts.graph = GraphQueryMode::Augment {
        seeds: 5,
        expansion: TraversalSpec::default(),
    };
    let result = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("two hops");
    let graph = result.graph.expect("graph");
    assert_eq!(graph.status, GraphStatus::Complete);
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| n.name == "leaf" && n.distance == Some(2))
    );
    assert_eq!(graph.relations.len(), 3);
    let mut execution = crate::api::explore::ExecutionOptions::default();
    execution.budget.edges = 1;
    opts.graph_execution = Some(execution);
    let result = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("graph exhaustion preserves safe text");
    assert!(!result.items.is_empty());
    assert_eq!(
        result.graph.expect("explicit graph stop").status,
        GraphStatus::Degraded
    );
    opts.graph_execution = None;
    opts.graph = GraphQueryMode::Augment {
        seeds: 1,
        expansion: TraversalSpec {
            relations: vec![RelationKind::Contains],
            direction: Direction::Incoming,
            max_hops: 1,
            ..Default::default()
        },
    };
    let result = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("direct parent");
    let graph = result.graph.expect("graph");
    assert_eq!(graph.status, GraphStatus::Complete);
    assert_eq!(graph.relations.len(), 1);
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| n.kind == crate::api::explore::EntityKind::File)
    );
}

#[tokio::test]
async fn graph_augment_total_deadline_during_admission_is_not_degradation() {
    use crate::workspace::{
        layout::find_nearest_workspace,
        lock::{LockMode, acquire_home_lock},
    };
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    build(directory.path(), &manager, true).await;
    let location = find_nearest_workspace(directory.path())
        .expect("location")
        .expect("workspace");
    let lock = acquire_home_lock(&location.home, LockMode::Write, "augment-test")
        .expect("writer blocks readers");
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = query_options(directory.path());
    let execution = crate::api::explore::ExecutionOptions {
        request_timeout_ms: 5,
        ..Default::default()
    };
    opts.graph_execution = Some(execution);
    let before = calls.load(Ordering::Acquire);
    let error = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect_err("total deadline");
    assert_eq!(error.code(), crate::EngineError::DEADLINE_EXCEEDED);
    assert_eq!(calls.load(Ordering::Acquire), before);
    drop(lock);
}

#[tokio::test]
async fn graph_augment_imports_share_business_traversal_and_domain_admission() {
    use crate::api::context::graph::{Direction, RelationKind};
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    drop(
        build_code(
            directory.path(),
            &manager,
            true,
            "mod dep { pub fn target() {} }\nuse dep::target;\nfn caller() { target(); }\n",
        )
        .await,
    );
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = query_options(directory.path());
    opts.graph = GraphQueryMode::Augment {
        seeds: 1,
        expansion: TraversalSpec {
            relations: vec![RelationKind::Contains, RelationKind::Imports],
            direction: Direction::Both,
            max_hops: 2,
            ..Default::default()
        },
    };
    let graph = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("import traversal")
        .graph
        .expect("graph");
    assert_eq!(graph.status, GraphStatus::Complete);
    assert!(
        graph
            .relations
            .iter()
            .any(|r| r.kind == RelationKind::Imports)
    );
    let import = graph
        .relations
        .iter()
        .find(|r| r.kind == RelationKind::Imports)
        .expect("import edge");
    assert!(
        graph
            .nodes
            .iter()
            .any(|n| n.node.node_id == import.target && n.distance == Some(2)),
        "import endpoint in the same view: {graph:?}"
    );
}

#[tokio::test]
async fn graph_augment_incomplete_code_domain_degrades_without_relaxing_ready() {
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    drop(
        build_code(
            directory.path(),
            &manager,
            true,
            "macro_rules! bind { ($x:ident) => { $x }; }\nfn target() {}\nfn caller() { let bind!(target) = || {}; target(); }\n",
        )
        .await,
    );
    let service = WorkspaceIndexService::with_test_registry();
    let opts = query_options(directory.path());
    let result = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("safe text");
    assert!(!result.items.is_empty());
    let graph = result.graph.expect("degraded");
    assert_eq!(graph.status, GraphStatus::Degraded);
    assert!(graph.view.is_none());
    assert!(graph.diagnostics.iter().any(|d| d.contains("not_ready")));
}

#[tokio::test]
async fn graph_augment_writer_text_degrades_and_unstable_text_fails() {
    use crate::{
        storage::graph_state::{TextPhase, TextState},
        workspace::{layout::find_nearest_workspace, manifest::read_workspace_manifest},
    };
    let directory = tempfile::tempdir().expect("workspace");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = build_code(
        directory.path(),
        &manager,
        true,
        "fn target() {}\nfn caller() { target(); }\n",
    )
    .await;
    let mut opts = query_options(directory.path());
    opts.api_key = Some("test-only-not-a-secret".into());
    opts.allow_remote = true;
    opts.endpoint = Some("https://example.invalid/v1".into());
    opts.model_cache = Some(directory.path().join("unused-model-cache"));
    let location = find_nearest_workspace(directory.path())
        .expect("location")
        .expect("workspace");
    let manifest = read_workspace_manifest(&location.home)
        .expect("manifest")
        .expect("manifest");
    let writer = register_writer(&service, &manager, &manifest, &opts);
    let result = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect("writer text");
    assert_eq!(
        result.graph.expect("explicit degradation").diagnostics,
        ["writer_view_unavailable"]
    );
    writer.retire().await;
    drop(writer);
    drop(service);
    let storage_home = manifest.storage_home();
    let mut state = TextState::read(&storage_home)
        .expect("state")
        .expect("state");
    state.state = TextPhase::Mutating;
    std::fs::write(
        storage_home.join("graph/STATE.json"),
        serde_json::to_vec(&state).expect("state JSON"),
    )
    .expect("simulate interrupted text update");
    let service = WorkspaceIndexService::with_test_registry();
    let error = indexed_search::service::context(&service, &manager, &opts, None, false)
        .await
        .expect_err("unsafe text cannot degrade");
    assert_eq!(error.code(), crate::EngineError::STORAGE_FAILURE);
}

fn register_writer(
    service: &WorkspaceIndexService,
    manager: &ModelRuntimeManager,
    manifest: &crate::workspace::manifest::WorkspaceManifest,
    opts: &ContextOptions,
) -> crate::pipelines::indexed_search::writer::WriterRegistration {
    use crate::{
        domain::model::ModelConfig,
        storage::{IndexStore, types::WorkspaceIndexStorageOptions},
        workspace::lock::{LockMode, acquire_home_lock},
    };
    let schema = manifest.embedding().expect("embedding");
    let model = manager
        .acquire(ModelRuntimeRequest::new(
            schema.model.reference(),
            ModelConfig {
                api_key: opts.api_key.clone(),
                endpoint: opts.endpoint.clone(),
                cache_dir: opts.model_cache.clone(),
                device: None,
            },
            None,
        ))
        .expect("fixture model lease");
    let lock = Arc::new(
        acquire_home_lock(&manifest.path, LockMode::Write, "augment-writer")
            .expect("writer admission"),
    );
    let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadWrite {
        storage_path: manifest.storage_home(),
        embeddings: vec![schema.clone()],
    })
    .expect("writer storage");
    service.writers.register(
        crate::pipelines::indexed_search::writer::WriterSession::new(
            storage,
            vec![model],
            manifest.clone(),
            lock,
        ),
        true,
    )
}
