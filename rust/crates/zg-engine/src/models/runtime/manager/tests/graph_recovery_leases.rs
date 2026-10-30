//! Nonempty dual-store recovery uses the same code/document corpus and model
//! fixture as the publication regressions, without downloading a model.
use super::*;
use crate::{
    api::index::graph::TextStateRecoveryOptions,
    storage::{graph_state, read_session::ReadSessionCache},
    workspace::lock::{LockMode, acquire_home_lock},
};
use zg_knowledge::{graph::GraphLimits, processing::ProcessingContext};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "nonempty native snapshot, repair and model-free rebuild share one fixture"
)]
async fn graph_nonempty_dirty_repair_preserves_text_and_rebuilds_without_embedding() {
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service
        .index(&manager, initial)
        .await
        .expect("joint publication");
    let manifest = active(dir.path());
    let storage_home = manifest.storage_home();
    let old = GraphHead::read(&storage_home)
        .expect("head")
        .expect("selected");
    let before_text = text_hashes(&storage_home.join("storage"));
    let before_calls = calls.load(Ordering::Acquire);
    let cache = ReadSessionCache::new(Duration::from_secs(60)).expect("cache");
    let ctx = ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 1_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    };
    {
        let _reader =
            acquire_home_lock(&manifest.path, LockMode::Read, "nonempty.graph").expect("reader");
        let lease = cache.acquire_graph(&manifest).expect("graph lease");
        let facts = lease
            .graph_facts(&manifest, GraphLimits::default(), &ctx)
            .expect("real EntityId binding");
        assert_eq!(facts.files.len(), 3);
        assert!(facts.files.iter().all(|f| f.record.entity_count > 0));
        assert!(!facts.resolutions.is_empty());
        let mut limited = ctx.clone();
        limited.max_output_items = 0;
        assert_eq!(
            lease
                .graph_facts(&manifest, GraphLimits::default(), &limited)
                .expect_err("read budget rejection")
                .code(),
            EngineError::RESOURCE_BUSY
        );
    }
    {
        let _writer = acquire_home_lock(&manifest.path, LockMode::Write, "interrupted.checkpoint")
            .expect("writer drains graph lease");
        let mut dirty = TextState::read(&storage_home)
            .expect("state")
            .expect("present");
        dirty.state = TextPhase::Mutating;
        graph_state::write_record(&storage_home.join("graph/STATE.json"), &dirty)
            .expect("interruption after real text checkpoint");
    }
    let engine = crate::ZvecGrep::new();
    let mut request = TextStateRecoveryOptions {
        root: Some(dir.path().into()),
        generation: manifest.storage_generation.clone().expect("generation"),
        ..Default::default()
    };
    let before = text_hashes(&storage_home);
    let seen = engine
        .recover_text_state(request.clone())
        .await
        .expect("inspect nonempty corpus");
    assert!(seen.dirty && seen.sources_match && !seen.repaired);
    assert_eq!(
        text_hashes(&storage_home),
        before,
        "inspection never writes either store"
    );
    request.repair = true;
    request.expected_operation_id = Some(seen.operation_id);
    request.expected_attempt = Some(seen.attempt);
    request.expected_text_revision = Some(seen.text_revision);
    let original = std::fs::read(dir.path().join("other.md")).expect("original");
    std::fs::write(dir.path().join("other.md"), "# Changed\n").expect("source change");
    assert_eq!(
        engine
            .recover_text_state(request.clone())
            .await
            .expect_err("source mismatch refuses repair")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    std::fs::write(dir.path().join("other.md"), original).expect("restore bytes");
    engine
        .recover_text_state(request)
        .await
        .expect("confirmed text repair");
    assert!(
        GraphHead::read(&storage_home)
            .expect("revoked head")
            .is_none()
    );
    assert_eq!(text_hashes(&storage_home.join("storage")), before_text);
    let zero = ModelRuntimeManager::with_factory(|_, _, _| {
        panic!("repair graph build must not load models")
    });
    let result = service
        .index(
            &zero,
            IndexOptions {
                root: Some(dir.path().into()),
                ..Default::default()
            },
        )
        .await
        .expect("graph-only rebuild");
    let graph = result.graph.expect("graph result");
    assert!(graph.graph_selected);
    assert_ne!(
        graph.graph_build_id.as_deref(),
        Some(old.graph_build_id.as_str())
    );
    assert_eq!(calls.load(Ordering::Acquire), before_calls);
    assert_eq!(text_hashes(&storage_home.join("storage")), before_text);
    cache.close();
}
