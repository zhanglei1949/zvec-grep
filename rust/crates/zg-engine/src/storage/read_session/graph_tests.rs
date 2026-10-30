use super::*;
use crate::{
    api::index::{
        IndexOptions,
        graph::{GraphOptions, GraphRecoveryAction, GraphRecoveryOptions, GraphScope},
    },
    models::ModelRuntimeManager,
    pipelines::indexing::service::WorkspaceIndexService,
    storage::graph_state::{self, GraphHead, TextPhase, TextState},
    workspace::{
        graph_policy::GraphPolicy, lock::acquire_home_lock, manifest::read_workspace_manifest,
    },
};
use std::fs;
use zg_knowledge::{
    graph::{GraphDbStore, GraphLimits, QueryControl, QueryStopReason},
    processing::ProcessingContext,
};

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 64 * 1024 * 1024,
        max_output_items: 1_000_000,
        deadline: Some(Instant::now() + Duration::from_secs(30)),
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}

fn database_files(
    path: &Path,
) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime, String)> {
    let mut result = std::collections::BTreeMap::new();
    let mut directories = vec![path.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).expect("database directory") {
            let entry = entry.expect("entry");
            let meta = entry.metadata().expect("metadata");
            if meta.is_dir() {
                directories.push(entry.path());
            } else {
                let bytes = std::fs::read(entry.path()).expect("file bytes");
                result.insert(
                    entry
                        .path()
                        .strip_prefix(path)
                        .expect("relative")
                        .to_path_buf(),
                    (
                        meta.len(),
                        meta.modified().expect("mtime"),
                        crate::utils::sha256_hex(&bytes),
                    ),
                );
            }
        }
    }
    result
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one native fixture exercises pointer changes, shared owners and per-read cancellation"
)]
async fn graph_leases_follow_head_and_reject_stale_state_without_poisoning_later_reads() {
    let dir = tempfile::tempdir().expect("root");
    let service = WorkspaceIndexService::with_test_registry();
    let models = ModelRuntimeManager::new();
    let index = |scope| IndexOptions {
        root: Some(dir.path().into()),
        graph: Some(GraphOptions {
            scope,
            ..Default::default()
        }),
        ..Default::default()
    };
    service
        .index(&models, index(GraphScope::Documents))
        .await
        .expect("first real graph");
    let home = dir.path().join(".zvec-grep");
    let manifest = read_workspace_manifest(&home)
        .expect("manifest")
        .expect("selected");
    let storage_home = manifest.storage_home();
    let first_head = GraphHead::read(&storage_home)
        .expect("first head")
        .expect("head");
    let first_policy = GraphPolicy::read(&home).expect("policy").expect("present");
    service
        .index(&models, index(GraphScope::Both))
        .await
        .expect("second graph");
    let second_head = GraphHead::read(&storage_home)
        .expect("head")
        .expect("second");
    let second_policy = GraphPolicy::read(&home).expect("policy").expect("second");
    assert_ne!(first_head.graph_build_id, second_head.graph_build_id);
    let old_path = storage_home
        .join("graph/builds")
        .join(&first_head.graph_build_id)
        .join("db");
    let current_path = storage_home
        .join("graph/builds")
        .join(&second_head.graph_build_id)
        .join("db");
    let before_old = database_files(&old_path);
    let before_current = database_files(&current_path);
    // Set up a prior selection before borrowing any read leases.
    {
        let _writer = acquire_home_lock(&home, LockMode::Write, "graph.fixture").expect("writer");
        graph_state::write_record(&home.join("graph/POLICY.json"), &first_policy)
            .expect("first policy");
        graph_state::write_record(&storage_home.join("graph/HEAD.json"), &first_head)
            .expect("first head");
    }
    let cache = ReadSessionCache::new(Duration::from_secs(60)).expect("cache");
    let other_cache = ReadSessionCache::new(Duration::from_secs(60)).expect("second cache");
    let reader = acquire_home_lock(&home, LockMode::Read, "graph.read").expect("reader");
    let old = cache.acquire_graph(&manifest).expect("old lease");
    let same = cache.acquire_graph(&manifest).expect("cache hit");
    assert!(Arc::ptr_eq(&old.session, &same.session));
    let other = other_cache
        .acquire_graph(&manifest)
        .expect("same native owner across engines");
    let diagnostic_path = old_path.parent().expect("build").join("attempt.json");
    let diagnostic_bytes = fs::read(&diagnostic_path).expect("diagnostic");
    let mut diagnostic: serde_json::Value =
        serde_json::from_slice(&diagnostic_bytes).expect("json");
    diagnostic["selectionEffect"] = "unknown".into();
    graph_state::write_record(&diagnostic_path, &diagnostic).expect("lost selection response");
    assert!(
        cache.acquire_graph(&manifest).is_err(),
        "unknown selection cannot grant a new lease"
    );
    assert!(
        old.graph_facts(&manifest, GraphLimits::default(), &context())
            .is_err(),
        "unknown selection invalidates borrowed graph results"
    );
    fs::write(&diagnostic_path, diagnostic_bytes).expect("restore reconciled proof");
    let build_path = old_path.parent().expect("build").join("build.json");
    let build_bytes = fs::read(&build_path).expect("immutable build input");
    let mut changed: serde_json::Value = serde_json::from_slice(&build_bytes).expect("json");
    changed["baseTextRevision"] = serde_json::json!(u64::MAX);
    graph_state::write_record(&build_path, &changed).expect("corrupted build input");
    assert!(
        old.graph_facts(&manifest, GraphLimits::default(), &context())
            .is_err(),
        "cached owners cannot hide changed immutable build inputs"
    );
    fs::write(&build_path, build_bytes).expect("restore immutable input");
    let cancel = QueryControl::new().expect("control");
    cancel.request(QueryStopReason::Cancelled);
    let error = old
        .graph_facts(
            &manifest,
            GraphLimits {
                control: Some(cancel),
                ..Default::default()
            },
            &context(),
        )
        .expect_err("native cancelled read");
    assert_eq!(error.code(), EngineError::CANCELLED);
    assert!(
        old.graph_facts(&manifest, GraphLimits::default(), &context())
            .expect("fresh read control")
            .files
            .is_empty()
    );
    // Simulate a publisher bypassing admission. Cache identity must still follow
    // HEAD within the same text generation and old leases must refuse results.
    graph_state::write_record(&home.join("graph/POLICY.json"), &second_policy)
        .expect("policy switch");
    graph_state::write_record(&storage_home.join("graph/HEAD.json"), &second_head)
        .expect("HEAD switch");
    let current = cache
        .acquire_graph(&manifest)
        .expect("new graph same generation");
    assert!(!Arc::ptr_eq(&old.session, &current.session));
    assert!(
        old.graph_facts(&manifest, GraphLimits::default(), &context())
            .is_err()
    );
    current
        .graph_facts(&manifest, GraphLimits::default(), &context())
        .expect("current graph");
    assert!(
        GraphDbStore::open(&old_path, false, GraphLimits::default()).is_err(),
        "old leases keep native owner alive"
    );
    drop(same);
    drop(old);
    drop(other);
    other_cache.close();
    assert_eq!(
        database_files(&old_path),
        before_old,
        "readonly open/query/close leaves files and mtimes unchanged"
    );
    drop(
        GraphDbStore::open(&old_path, false, GraphLimits::default())
            .expect("last old owner released"),
    );
    let mut state = TextState::read(&storage_home)
        .expect("state")
        .expect("present");
    state.state = TextPhase::Mutating;
    graph_state::write_record(&storage_home.join("graph/STATE.json"), &state).expect("dirty fence");
    assert!(cache.acquire_graph(&manifest).is_err());
    assert!(
        current
            .graph_facts(&manifest, GraphLimits::default(), &context())
            .is_err()
    );
    state.state = TextPhase::Stable;
    graph_state::write_record(&storage_home.join("graph/STATE.json"), &state)
        .expect("restore state");
    GraphPolicy::save(&home, GraphOptions::default()).expect("Off");
    assert!(cache.acquire_graph(&manifest).is_err());
    drop(reader);
    let writer_home = home.clone();
    let waiting = std::thread::spawn(move || {
        acquire_home_lock(&writer_home, LockMode::Write, "graph.writer")
    });
    std::thread::sleep(Duration::from_millis(30));
    assert!(!waiting.is_finished(), "live graph lease retains residency");
    drop(current);
    drop(
        waiting
            .join()
            .expect("writer thread")
            .expect("leases drained"),
    );
    cache.close();
    assert_eq!(
        database_files(&current_path),
        before_current,
        "current read session closes without file writes"
    );
    let request = |id: String| GraphRecoveryOptions {
        root: Some(dir.path().into()),
        graph_build_id: id,
        ..Default::default()
    };
    let mut current_request = request(second_head.graph_build_id.clone());
    let inspected = service
        .recover_graph(current_request.clone())
        .await
        .expect("inspect current");
    current_request.action = GraphRecoveryAction::Discard;
    current_request.expected_operation_id = Some(inspected.operation_id);
    current_request.expected_attempt = Some(inspected.attempt);
    assert_eq!(
        service
            .recover_graph(current_request)
            .await
            .expect_err("HEAD protects selected candidate")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    let mut old_request = request(first_head.graph_build_id);
    let inspected = service
        .recover_graph(old_request.clone())
        .await
        .expect("inspect retired candidate");
    old_request.action = GraphRecoveryAction::Discard;
    old_request.expected_operation_id = Some(inspected.operation_id);
    old_request.expected_attempt = Some(inspected.attempt + 1);
    assert!(
        service.recover_graph(old_request.clone()).await.is_err(),
        "discard requires inspected identity"
    );
    assert!(old_path.exists());
    old_request.expected_attempt = Some(inspected.attempt);
    let discarded = service
        .recover_graph(old_request)
        .await
        .expect("explicit safe cleanup");
    assert!(discarded.discarded && !discarded.selected && !discarded.eligible);
    assert!(!old_path.parent().expect("build directory").exists());
    assert!(
        storage_home
            .join("graph/builds")
            .join(second_head.graph_build_id)
            .exists()
    );
}
