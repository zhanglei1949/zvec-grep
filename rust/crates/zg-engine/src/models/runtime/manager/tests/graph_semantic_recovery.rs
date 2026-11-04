//! Fixed semantic input must survive a real interrupted native import.
use super::*;
use crate::{
    api::index::graph::{GraphRecoveryAction, GraphRecoveryOptions, SemanticBuildOptions},
    storage::graph_state::{BuildRecord, GraphHead, SemanticBuildInput, read_record, write_record},
};
use zg_knowledge::{
    graph::{GraphDbStore, GraphErrorKind, GraphLimits, ImportBudget},
    processing::{ProcessingContext, stable_digest},
    relations::bound::FactCatalog,
};

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 1_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn interrupted(options: &SemanticBuildOptions) -> FactCatalog {
    let manifest = active(options.root.as_deref().expect("root"));
    let home = manifest.storage_home();
    let base_path = home
        .join("graph/builds")
        .join(&options.expected_graph_build_id);
    let context = context();
    let base = GraphDbStore::open(base_path.join("db"), true, GraphLimits::default())
        .expect("base")
        .read_facts(&context)
        .expect("base facts");
    let mut record: BuildRecord = read_record(&base_path.join("build.json"))
        .expect("record")
        .expect("base record");
    record.graph_build_id.clone_from(&options.operation_id);
    record.operation_id.clone_from(&options.operation_id);
    record.attempt = 1;
    record.base_head = Some(options.expected_graph_build_id.clone());
    record.semantic = Some(SemanticBuildInput {
        base_facts_digest: stable_digest(&base).expect("base digest"),
        input: options.input.clone(),
    });
    record.rules_fingerprint = record.expected_rules_fingerprint().expect("rules");
    let mut expected = base.apply_sources(&options.input, &context).expect("input");
    expected.semantic.last_operation = record.semantic_operation().expect("operation");
    let budget = ImportBudget {
        max_batch_rows: 2,
        max_batch_bytes: 4096,
    };
    record.import_budget = Some(budget.clone());
    let path = home.join("graph/builds").join(&options.operation_id);
    write_record(&path.join("build.json"), &record).expect("fixed input");
    let graph = GraphDbStore::create_batched_build(path.join("db"), GraphLimits::default())
        .expect("candidate");
    let stop = context.cancellation.clone();
    assert_eq!(
        graph
            .write_facts_batched_observed(&expected, &budget, &context, |_| stop.cancel())
            .expect_err("stop after first batch")
            .kind,
        GraphErrorKind::Cancelled
    );
    graph.checkpoint().expect("durable prefix");
    expected
}

async fn initial() -> (tempfile::TempDir, crate::ZvecGrep) {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(dir.path().join("src/a.rs"), "fn entry() {}\n").expect("source");
    let calls = Arc::new(AtomicUsize::new(0));
    let service = WorkspaceIndexService::with_test_registry();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service
        .index(&models(&calls), initial)
        .await
        .expect("index");
    drop(service);
    let engine = crate::ZvecGrep::new();
    engine
        .apply_semantics(semantic_options(dir.path(), "initial"))
        .await
        .expect("F1");
    (dir, engine)
}

#[tokio::test]
async fn m3_fixed_semantic_input_resumes_without_reapplying_correction() {
    let (dir, engine) = initial().await;
    let options = semantic_options(dir.path(), "explicit-correction");
    let expected = interrupted(&options);
    let before = text_hashes(&active(dir.path()).storage_home().join("storage"));
    assert!(
        engine
            .apply_semantics(options.clone())
            .await
            .expect_err("explicit recovery required")
            .is_recovery_required()
    );
    let mut recovery = GraphRecoveryOptions {
        root: options.root.clone(),
        graph_build_id: options.operation_id.clone(),
        ..Default::default()
    };
    let seen = engine
        .recover_graph(recovery.clone())
        .await
        .expect("inspect");
    assert_eq!(seen.completed_batches, Some(1));
    assert!(!seen.candidate_committed && !seen.selected);
    recovery.expected_operation_id = Some(seen.operation_id.clone());
    recovery.expected_attempt = Some(seen.attempt);
    recovery.action = GraphRecoveryAction::Resume;
    let resumed = engine
        .recover_graph(recovery.clone())
        .await
        .expect("resume");
    assert!(resumed.candidate_committed && !resumed.selected);
    assert_eq!(resumed.operation_id, options.operation_id);
    assert_eq!(resumed.attempt, 1);
    let again = engine
        .recover_graph(recovery.clone())
        .await
        .expect("resume twice");
    assert!(
        again
            .diagnostics
            .iter()
            .any(|d| d == "resume_already_committed")
    );
    assert_eq!(
        fact_ids(
            &engine
                .explore(semantic_request(dir.path(), false))
                .await
                .expect("base still selected")
        ),
        ["f1"]
    );
    recovery.action = GraphRecoveryAction::Select;
    assert!(
        engine
            .recover_graph(recovery)
            .await
            .expect("select")
            .selected
    );
    engine.close();
    let reopened = crate::ZvecGrep::new();
    assert_eq!(
        fact_ids(
            &reopened
                .explore(semantic_request(dir.path(), false))
                .await
                .expect("F2 current")
        ),
        ["f2"]
    );
    assert_eq!(
        reopened
            .apply_semantics(options)
            .await
            .expect("same operation")
            .mode,
        "semantic_replayed"
    );
    let home = active(dir.path()).storage_home();
    let head = GraphHead::read(&home).expect("head").expect("selected");
    let graph = GraphDbStore::open(
        home.join("graph/builds")
            .join(head.graph_build_id)
            .join("db"),
        true,
        GraphLimits::default(),
    )
    .expect("reopen candidate");
    assert_eq!(graph.read_facts(&context()).expect("facts"), expected);
    assert_eq!(text_hashes(&home.join("storage")), before);
    assert_eq!(reopened.runtime_snapshot().loaded_models, 0);
    reopened.close();
}
