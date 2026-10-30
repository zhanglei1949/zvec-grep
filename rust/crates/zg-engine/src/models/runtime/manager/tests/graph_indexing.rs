//! Real service + real zvec/graphdb; only model computation is deterministic.
#[path = "graph_recovery_leases.rs"]
mod recovery_leases;
use super::*;
use crate::EngineError;
use crate::{
    api::index::{
        IndexOptions,
        graph::{GraphLanguage, GraphModuleRoot, GraphOptions, GraphScope, PackageLayout},
    },
    pipelines::indexing::service::WorkspaceIndexService,
    storage::graph_state::{GraphHead, TextPhase, TextState},
    workspace::{graph_policy::GraphPolicy, manifest::read_workspace_manifest},
};

struct FixtureModel {
    info: EmbeddingModelInfo,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl EmbeddingModel for FixtureModel {
    fn info(&self) -> &EmbeddingModelInfo {
        &self.info
    }
    async fn embed(
        &self,
        input: &[Vec<Content>],
        _: EmbeddingOptions,
    ) -> Result<EmbeddingResult, ModelError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Ok(EmbeddingResult {
            vectors: input.iter().map(|_| vec![1.0, 0.0]).collect(),
            truncated: Vec::new(),
        })
    }
}
pub(super) fn models(calls: &Arc<AtomicUsize>) -> ModelRuntimeManager {
    let calls = Arc::clone(calls);
    ModelRuntimeManager::with_factory(move |_, _, _| {
        Ok(Arc::new(FixtureModel {
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
            calls: Arc::clone(&calls),
        }))
    })
}
pub(super) fn options(root: &std::path::Path) -> IndexOptions {
    IndexOptions {
        root: Some(root.into()),
        allow_remote: true,
        api_key: Some("test-only-not-a-secret".into()),
        embedding: Some(crate::api::index::options::EmbeddingModelSpec {
            reference: "qwen/text-embedding-v4".into(),
            revision: None,
            cache_dir: None,
            endpoint: Some("https://example.invalid/v1".into()),
            device: crate::domain::model::Device::Auto,
        }),
        ..IndexOptions::default()
    }
}
pub(super) fn policy() -> GraphOptions {
    GraphOptions {
        scope: GraphScope::Both,
        module_roots: vec![GraphModuleRoot {
            root: "src".into(),
            language: GraphLanguage::Rust,
            namespace: "fixture".into(),
            module_prefix: vec![],
            layout: PackageLayout::Flat,
        }],
        rules: None,
    }
}
fn corpus(root: &std::path::Path) {
    std::fs::create_dir(root.join("src")).expect("src");
    std::fs::write(
        root.join("src/lib.rs"),
        "\u{feff}fn target() {}\r\nfn caller() { target(); }\r\n",
    )
    .expect("code");
    std::fs::write(
        root.join("readme.md"),
        "# 中文\r\n[more](other.md#target)\r\n",
    )
    .expect("doc");
    std::fs::write(root.join("other.md"), "# Target\n").expect("target");
}

fn recovery_options(
    root: &std::path::Path,
    id: &str,
) -> crate::api::index::graph::GraphRecoveryOptions {
    crate::api::index::graph::GraphRecoveryOptions {
        root: Some(root.into()),
        graph_build_id: id.into(),
        ..Default::default()
    }
}
fn mutate_recovery(
    mut options: crate::api::index::graph::GraphRecoveryOptions,
    result: &crate::api::index::graph::GraphRecoveryResult,
    action: crate::api::index::graph::GraphRecoveryAction,
) -> crate::api::index::graph::GraphRecoveryOptions {
    options.action = action;
    options.expected_operation_id = Some(result.operation_id.clone());
    options.expected_attempt = Some(result.attempt);
    options
}

#[tokio::test]
async fn graph_explicit_recovery_inspection_is_readonly_and_reconciles_selected_effect() {
    use crate::api::index::graph::GraphRecoveryAction;
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    let result = service.index(&manager, opts).await.expect("initial graph");
    let id = result.graph.expect("graph").graph_build_id.expect("id");
    let manifest = active(dir.path());
    let home = manifest.storage_home();
    let path = home.join("graph/builds").join(&id).join("attempt.json");
    let mut diagnostic: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("attempt")).expect("json");
    diagnostic["stage"] = "select_pending".into();
    diagnostic["selectionEffect"] = "unknown".into();
    crate::storage::graph_state::write_record(&path, &diagnostic)
        .expect("lost host response injection after real selected effect");
    let initial_calls = calls.load(Ordering::Acquire);
    let before = text_hashes(&home);
    let engine = crate::ZvecGrep::new();
    let inspect = recovery_options(dir.path(), &id);
    let seen = engine
        .recover_graph(inspect.clone())
        .await
        .expect("read-only inspect");
    assert!(seen.candidate_committed && seen.selected && seen.eligible && seen.facts_ready);
    assert_eq!(
        text_hashes(&home),
        before,
        "inspect must not rewrite any graph or text file"
    );
    let mut wrong = mutate_recovery(inspect.clone(), &seen, GraphRecoveryAction::Reconcile);
    wrong.expected_attempt = Some(seen.attempt + 1);
    assert_eq!(
        engine
            .recover_graph(wrong)
            .await
            .expect_err("old client attempt")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    let result = engine
        .recover_graph(mutate_recovery(
            inspect.clone(),
            &seen,
            GraphRecoveryAction::Reconcile,
        ))
        .await
        .expect("writable reconciliation");
    assert!(result.selected && result.eligible);
    let now: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("attempt")).expect("json");
    assert_eq!(now["stage"], "selected");
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    assert_eq!(calls.load(Ordering::Acquire), initial_calls);
    std::fs::write(dir.path().join("src/lib.rs"), "fn replacement() {}\n")
        .expect("change raw source without index");
    let stale = engine
        .recover_graph(inspect)
        .await
        .expect("stale is diagnosed");
    assert!(stale.selected && stale.candidate_committed);
    assert!(!stale.eligible);
}

/// Another immutable candidate bound to the same real text. Only host attempt
/// diagnostics are injected; all fact batches and root receipts are native.
fn unselected_candidate(
    manifest: &crate::workspace::manifest::WorkspaceManifest,
    incomplete: bool,
    commit_root: bool,
) -> String {
    use zg_knowledge::{
        graph::{
            GraphDbStore, GraphLimits, ImportBudget,
            build::{BuildScope, BuildSpec},
        },
        processing::ProcessingContext,
    };
    let home = manifest.storage_home();
    let old = GraphHead::read(&home).expect("head").expect("selected");
    let ctx = ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 1_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::default(),
    };
    let old_path = home.join("graph/builds").join(&old.graph_build_id);
    let graph =
        GraphDbStore::open(old_path.join("db"), true, GraphLimits::default()).expect("old graph");
    let catalog = graph.read_facts(&ctx).expect("facts");
    drop(graph);
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(old_path.join("build.json")).expect("record"))
            .expect("json");
    let id = uuid::Uuid::new_v4().to_string();
    let operation = uuid::Uuid::new_v4().to_string();
    record["graphBuildId"] = id.clone().into();
    record["operationId"] = operation.clone().into();
    record["baseHead"] = old.graph_build_id.into();
    let budget = ImportBudget {
        max_batch_rows: 2,
        max_batch_bytes: 4096,
    };
    record["importBudget"] = serde_json::to_value(&budget).expect("budget");
    let path = home.join("graph/builds").join(&id);
    crate::storage::graph_state::write_record(&path.join("build.json"), &record)
        .expect("ownership");
    let graph = GraphDbStore::create_batched_build(path.join("db"), GraphLimits::default())
        .expect("candidate");
    if incomplete {
        let cancel = ctx.cancellation.clone();
        assert_eq!(
            graph
                .write_facts_batched_observed(&catalog, &budget, &ctx, |_| cancel.cancel())
                .expect_err("cancel between batches")
                .kind,
            zg_knowledge::graph::GraphErrorKind::Cancelled
        );
    } else {
        graph
            .write_facts_batched(&catalog, &budget, &ctx)
            .expect("batches");
        let spec = BuildSpec {
            graph_build_id: id.clone(),
            operation_id: operation.clone(),
            attempt: record["attempt"].as_u64().expect("attempt"),
            base_text_revision: record["baseTextRevision"].as_u64().expect("revision"),
            base_catalog_digest: record["baseCatalogDigest"].as_str().expect("digest").into(),
            base_head: Some(record["baseHead"].as_str().expect("base").into()),
            scope: BuildScope::Both,
            configuration: record["configuration"].clone(),
            policy_fingerprint: record["policyFingerprint"].as_str().expect("policy").into(),
            rules_fingerprint: record["rulesFingerprint"].as_str().expect("rules").into(),
            module_roots_fingerprint: record["moduleRootsFingerprint"]
                .as_str()
                .expect("roots")
                .into(),
        };
        if commit_root {
            graph.commit_build(&spec, &ctx).expect("real root/receipt");
        }
    }
    graph.checkpoint().expect("durable");
    drop(graph);
    crate::storage::graph_state::write_record(&path.join("attempt.json"),&serde_json::json!({"formatVersion":1,"operationId":operation,"attempt":record["attempt"],"stage":"commit_pending","commitEffect":"unknown","selectionEffect":"not_selected","cancellationRequested":false,"errorCode":null})).expect("unknown host response");
    id
}

#[tokio::test]
async fn graph_recovery_selects_only_matching_committed_candidate_and_rejects_stale_base() {
    use crate::api::index::graph::GraphRecoveryAction;
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    service.index(&manager, opts).await.expect("initial");
    let manifest = active(dir.path());
    let first = unselected_candidate(&manifest, false, true);
    let competing = unselected_candidate(&manifest, false, true);
    let engine = crate::ZvecGrep::new();
    let inspect = recovery_options(dir.path(), &first);
    let seen = engine
        .recover_graph(inspect.clone())
        .await
        .expect("unselected proof");
    assert!(seen.candidate_committed && seen.eligible && !seen.selected);
    let selected = engine
        .recover_graph(mutate_recovery(inspect, &seen, GraphRecoveryAction::Select))
        .await
        .expect("select matching candidate");
    assert!(selected.selected && selected.eligible);
    let inspect = recovery_options(dir.path(), &competing);
    let seen = engine
        .recover_graph(inspect.clone())
        .await
        .expect("competitor proof");
    assert!(
        !seen.eligible,
        "a superseded selection base is not eligible"
    );
    assert_eq!(
        engine
            .recover_graph(mutate_recovery(inspect, &seen, GraphRecoveryAction::Select))
            .await
            .expect_err("cannot overwrite later head")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    assert_eq!(
        GraphHead::read(&manifest.storage_home())
            .expect("head")
            .expect("head")
            .graph_build_id,
        first
    );
    let conflicting = manifest
        .storage_home()
        .join("graph/builds")
        .join(&competing);
    let bytes = std::fs::read(conflicting.join("attempt.json")).expect("attempt");
    let mut input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(conflicting.join("build.json")).expect("record"))
            .expect("json");
    input["baseCatalogDigest"] = crate::utils::sha256_hex(b"contradictory-base").into();
    crate::storage::graph_state::write_record(&conflicting.join("build.json"), &input)
        .expect("corrupt immutable input");
    let error = engine
        .recover_graph(recovery_options(dir.path(), &competing))
        .await
        .expect_err("root and host input disagree");
    assert!(error.message().contains("recovery_required"));
    assert_eq!(
        std::fs::read(conflicting.join("attempt.json")).expect("retained evidence"),
        bytes
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
}

#[tokio::test]
async fn graph_recovery_classifies_partial_import_and_does_not_repair_text_fence() {
    use crate::api::index::graph::GraphRecoveryAction;
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    service.index(&manager, opts).await.expect("initial");
    let manifest = active(dir.path());
    let id = unselected_candidate(&manifest, true, false);
    let engine = crate::ZvecGrep::new();
    let inspect = recovery_options(dir.path(), &id);
    let seen = engine
        .recover_graph(inspect.clone())
        .await
        .expect("partial native proof");
    assert!(!seen.facts_ready && !seen.candidate_committed && !seen.selected && !seen.eligible);
    assert_eq!(seen.completed_batches, Some(1));
    engine
        .recover_graph(mutate_recovery(
            inspect.clone(),
            &seen,
            GraphRecoveryAction::Reconcile,
        ))
        .await
        .expect("no root/receipt confirmed");
    assert_eq!(
        engine
            .recover_graph(mutate_recovery(inspect, &seen, GraphRecoveryAction::Select))
            .await
            .expect_err("partial cannot publish")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    let imported = unselected_candidate(&manifest, false, false);
    let check = recovery_options(dir.path(), &imported);
    let seen = engine
        .recover_graph(check.clone())
        .await
        .expect("facts complete without publication receipt");
    assert!(seen.facts_ready && !seen.candidate_committed && !seen.selected);
    engine
        .recover_graph(mutate_recovery(
            check,
            &seen,
            GraphRecoveryAction::Reconcile,
        ))
        .await
        .expect("confirm absent native receipt");
    let home = manifest.storage_home();
    let mut state = TextState::read(&home).expect("state").expect("state");
    state.state = TextPhase::Mutating;
    crate::storage::graph_state::write_record(&home.join("graph/STATE.json"), &state)
        .expect("dirty");
    let selected = GraphHead::read(&home).expect("head").expect("head");
    let inspect = recovery_options(dir.path(), &selected.graph_build_id);
    let seen = engine
        .recover_graph(inspect.clone())
        .await
        .expect("dirty inspected");
    assert!(!seen.eligible);
    engine
        .recover_graph(mutate_recovery(
            inspect,
            &seen,
            GraphRecoveryAction::Reconcile,
        ))
        .await
        .expect("reconcile preserves dirty");
    assert_eq!(TextState::read(&home).expect("state"), Some(state));
}

#[tokio::test]
async fn graph_recovery_resumes_an_interrupted_import_with_fixed_identity() {
    use crate::api::index::graph::GraphRecoveryAction;
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    service.index(&manager, opts).await.expect("initial");
    let after_index = calls.load(Ordering::Acquire);
    let manifest = active(dir.path());
    // An import interrupted after the first batch, before finalize and commit.
    let id = unselected_candidate(&manifest, true, false);
    let engine = crate::ZvecGrep::new();
    let inspect = recovery_options(dir.path(), &id);
    let seen = engine
        .recover_graph(inspect.clone())
        .await
        .expect("partial inspect");
    assert!(!seen.facts_ready && !seen.candidate_committed && !seen.selected);
    assert_eq!(seen.completed_batches, Some(1));
    let total = seen.total_batches.expect("total batches");
    assert!(total > 1, "fixture must span multiple batches to interrupt");
    // A mismatched identity must not resume.
    let mut wrong = mutate_recovery(inspect.clone(), &seen, GraphRecoveryAction::Resume);
    wrong.expected_attempt = Some(seen.attempt + 1);
    assert_eq!(
        engine
            .recover_graph(wrong)
            .await
            .expect_err("identity mismatch")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    // Resume with the inspected fixed identity continues the persisted prefix,
    // commits and confirms, without re-embedding or minting a new operation.
    let resumed = engine
        .recover_graph(mutate_recovery(
            inspect.clone(),
            &seen,
            GraphRecoveryAction::Resume,
        ))
        .await
        .expect("resume interrupted import");
    assert!(resumed.facts_ready && resumed.candidate_committed && !resumed.selected);
    assert_eq!(resumed.completed_batches, Some(total));
    assert_eq!(resumed.operation_id, seen.operation_id);
    assert_eq!(resumed.attempt, seen.attempt);
    assert_eq!(
        calls.load(Ordering::Acquire),
        after_index,
        "resume must not embed again"
    );
    // Resume is idempotent: a second call reports the committed candidate.
    let again = engine
        .recover_graph(mutate_recovery(
            inspect.clone(),
            &resumed,
            GraphRecoveryAction::Resume,
        ))
        .await
        .expect("idempotent resume");
    assert!(again.candidate_committed);
    assert!(
        again
            .diagnostics
            .iter()
            .any(|d| d == "resume_already_committed")
    );
    // The resumed candidate is now selectable.
    let selected = engine
        .recover_graph(mutate_recovery(
            inspect,
            &again,
            GraphRecoveryAction::Select,
        ))
        .await
        .expect("select resumed candidate");
    assert!(selected.selected && selected.eligible);
}
#[tokio::test]
async fn graph_resume_late_stop_preserves_commit_through_host_recheck() {
    use crate::api::index::graph::GraphRecoveryAction;
    use zg_knowledge::graph::QueryStopReason;

    for reason in [QueryStopReason::Cancelled, QueryStopReason::Deadline] {
        let dir = tempfile::tempdir().expect("corpus");
        corpus(dir.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = models(&calls);
        let service = WorkspaceIndexService::with_test_registry();
        let mut opts = options(dir.path());
        opts.graph = Some(policy());
        service.index(&manager, opts).await.expect("initial");
        let after_index = calls.load(Ordering::Acquire);
        let manifest = active(dir.path());
        let original_head = GraphHead::read(&manifest.storage_home())
            .expect("head")
            .expect("selected");
        let id = unselected_candidate(&manifest, true, false);
        let engine = crate::ZvecGrep::new();
        let inspect = recovery_options(dir.path(), &id);
        let seen = engine
            .recover_graph(inspect.clone())
            .await
            .expect("partial inspect");
        assert!(!seen.candidate_committed);
        WorkspaceIndexService::stop_resume_after_commit(id.clone(), reason);
        let resumed = engine
            .recover_graph(mutate_recovery(
                inspect.clone(),
                &seen,
                GraphRecoveryAction::Resume,
            ))
            .await
            .expect("late stop must not erase the committed effect");
        WorkspaceIndexService::clear_resume_commit_hook();
        assert!(resumed.facts_ready && resumed.candidate_committed);
        assert!(!resumed.selected, "resume never selects automatically");
        assert_eq!(resumed.operation_id, seen.operation_id);
        assert_eq!(resumed.attempt, seen.attempt);
        let expected = match reason {
            QueryStopReason::Cancelled => "cancellation_arrived_after_commit",
            QueryStopReason::Deadline => "deadline_arrived_after_commit",
        };
        assert!(resumed.diagnostics.iter().any(|d| d == expected));
        assert_eq!(calls.load(Ordering::Acquire), after_index);
        assert_eq!(
            GraphHead::read(&manifest.storage_home())
                .expect("head")
                .expect("selected"),
            original_head
        );

        let path = manifest.storage_home().join("graph/builds").join(&id);
        let record: crate::storage::graph_state::BuildRecord =
            crate::storage::graph_state::read_record(&path.join("build.json"))
                .expect("record")
                .expect("present");
        let context = zg_knowledge::processing::ProcessingContext {
            max_input_bytes: 64 << 20,
            max_output_items: 1_000_000,
            deadline: None,
            cancellation: tokio_util::sync::CancellationToken::default(),
        };
        let graph = zg_knowledge::graph::GraphDbStore::open_import(
            path.join("db"),
            true,
            zg_knowledge::graph::GraphLimits::default(),
            &context,
        )
        .expect("read-only reopen with fresh native control");
        let proof = graph
            .inspect_build(&record.spec(), &context)
            .expect("root and receipt");
        assert!(proof.is_some());
        drop(graph);
        let attempt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path.join("attempt.json")).expect("attempt"))
                .expect("json");
        assert_eq!(attempt["commitEffect"], "committed");
        assert_eq!(attempt["stage"], "confirmed");

        let again = engine
            .recover_graph(mutate_recovery(
                inspect,
                &resumed,
                GraphRecoveryAction::Resume,
            ))
            .await
            .expect("idempotent committed resume");
        assert!(again.candidate_committed && !again.selected);
        assert!(
            again
                .diagnostics
                .iter()
                .any(|d| d == "resume_already_committed")
        );
        assert_eq!(calls.load(Ordering::Acquire), after_index);
    }
}

pub(super) fn active(root: &std::path::Path) -> crate::workspace::manifest::WorkspaceManifest {
    read_workspace_manifest(&root.join(".zvec-grep"))
        .expect("read")
        .expect("active manifest")
}
pub(super) fn text_hashes(
    home: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, String> {
    let mut out = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(home).expect("directory") {
        let entry = entry.expect("entry");
        if entry.file_type().expect("type").is_dir() {
            out.extend(text_hashes(&entry.path()));
        } else {
            out.insert(
                entry.path(),
                crate::utils::sha256_hex(&std::fs::read(entry.path()).expect("bytes")),
            );
        }
    }
    out
}

#[tokio::test]
async fn graph_publication_first_enable_on_existing_text_is_model_free() {
    let dir = tempfile::tempdir().expect("workspace");
    corpus(dir.path());
    let service = WorkspaceIndexService::with_test_registry();
    let calls = Arc::new(AtomicUsize::new(0));
    let model = models(&calls);
    service
        .index(&model, options(dir.path()))
        .await
        .expect("ordinary text index");
    let manifest = active(dir.path());
    assert!(!manifest.storage_home().join("graph").exists());
    let before = text_hashes(&manifest.storage_home().join("storage"));
    let embeddings = calls.load(Ordering::Acquire);
    let zero = ModelRuntimeManager::with_factory(|_, _, _| {
        panic!("first graph enable must not load a model")
    });
    let result = service
        .index(
            &zero,
            IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(policy()),
                ..IndexOptions::default()
            },
        )
        .await
        .expect("model-free graph enable")
        .graph
        .expect("graph");
    assert_eq!(result.mode, "graph_only");
    assert!(result.graph_selected && result.graph_candidate_committed);
    assert!(!result.text_published);
    assert_eq!(
        active(dir.path()).storage_generation,
        manifest.storage_generation
    );
    assert_eq!(calls.load(Ordering::Acquire), embeddings);
    assert_eq!(
        text_hashes(&manifest.storage_home().join("storage")),
        before
    );
}

#[tokio::test]
async fn graph_publication_legacy_manifest_is_rejected_before_policy_or_models() {
    let dir = tempfile::tempdir().expect("workspace");
    corpus(dir.path());
    let service = WorkspaceIndexService::with_test_registry();
    let calls = Arc::new(AtomicUsize::new(0));
    let model = models(&calls);
    service
        .index(&model, options(dir.path()))
        .await
        .expect("ordinary index");
    let manifest = active(dir.path());
    let before = text_hashes(&manifest.storage_home().join("storage"));
    let file = manifest.path.join("manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).expect("manifest")).expect("json");
    value["storageGeneration"] = serde_json::Value::Null;
    let invalid = serde_json::to_vec(&value).expect("json");
    std::fs::write(&file, &invalid).expect("legacy-layout metadata fixture");
    let zero =
        ModelRuntimeManager::with_factory(|_, _, _| panic!("legacy rejection must precede models"));
    let error = service
        .index(
            &zero,
            IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(policy()),
                ..Default::default()
            },
        )
        .await
        .expect_err("current format requires an explicit generation");
    assert!(error.message().contains("rebuild"));
    assert_eq!(std::fs::read(file).expect("no implicit migration"), invalid);
    assert!(!manifest.path.join("graph/POLICY.json").exists());
    assert_eq!(
        text_hashes(&manifest.storage_home().join("storage")),
        before
    );
}

#[tokio::test]
async fn graph_publication_exclusion_invalidates_target_without_embedding() {
    use zg_knowledge::{
        graph::{GraphDbStore, GraphLimits},
        processing::ProcessingContext,
        relations::ResolutionStatus,
    };
    let dir = tempfile::tempdir().expect("workspace");
    corpus(dir.path());
    let service = WorkspaceIndexService::with_test_registry();
    let calls = Arc::new(AtomicUsize::new(0));
    let model = models(&calls);
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service.index(&model, initial).await.expect("initial");
    let first = active(dir.path());
    let old = TextState::read(&first.storage_home())
        .expect("state")
        .expect("fence");
    let count = calls.load(Ordering::Acquire);
    let excluded = service
        .index(
            &model,
            IndexOptions {
                root: Some(dir.path().into()),
                allow_remote: true,
                scan: crate::api::index::options::ScanRulesUpdate {
                    globs: Some(vec!["!other.md".into()]),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .expect("exclude target");
    assert_eq!(excluded.files_deleted, 1);
    assert_eq!(calls.load(Ordering::Acquire), count);
    assert!(excluded.graph.expect("graph").graph_selected);
    assert_eq!(
        TextState::read(&first.storage_home())
            .expect("state")
            .expect("stable")
            .text_revision,
        old.text_revision + 1
    );
    let head = GraphHead::read(&first.storage_home())
        .expect("head")
        .expect("selected");
    let graph = GraphDbStore::open(
        first
            .storage_home()
            .join("graph/builds")
            .join(head.graph_build_id)
            .join("db"),
        true,
        GraphLimits::default(),
    )
    .expect("real graph readonly");
    let ctx = ProcessingContext {
        max_input_bytes: 64 * 1024 * 1024,
        max_output_items: 1_000_000,
        deadline: None,
        cancellation: CancellationToken::new(),
    };
    let facts = graph.read_facts(&ctx).expect("complete real readback");
    assert!(facts.files.iter().all(|f| f.record.path != "other.md"));
    assert!(
        facts
            .resolutions
            .iter()
            .filter(|r| r.file_path == "readme.md")
            .all(|r| r.status != ResolutionStatus::Resolved)
    );
}

#[tokio::test]
async fn graph_publication_service_joint_rebuild_model_free_reuse_and_policy_change() {
    let dir = tempfile::tempdir().expect("workspace");
    corpus(dir.path());
    let service = WorkspaceIndexService::with_test_registry();
    let calls = Arc::new(AtomicUsize::new(0));
    let model = models(&calls);
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    let result = service
        .index(&model, initial.clone())
        .await
        .expect("real joint publication");
    assert_eq!(result.files_failed, 0);
    assert_eq!(result.files_added, 3);
    let graph = result.graph.expect("graph result");
    assert!(
        graph.text_published && graph.graph_selected && graph.graph_candidate_committed,
        "{graph:?}"
    );
    assert_eq!(graph.mode, "text_and_graph");
    let first = active(dir.path());
    let state = TextState::read(&first.storage_home())
        .expect("state")
        .expect("fence");
    assert_eq!(state.state, TextPhase::Stable);
    assert_eq!(state.text_revision, 1);
    let head = GraphHead::read(&first.storage_home())
        .expect("head")
        .expect("selected");
    assert_eq!(Some(&head.graph_build_id), graph.graph_build_id.as_ref());
    let before = text_hashes(&first.storage_home().join("storage"));
    let graph_before = text_hashes(&first.storage_home().join("graph"));
    let embeddings = calls.load(Ordering::Acquire);
    // The saved descriptor is remote, but no grant/key/model is supplied here.
    // Any model runtime acquisition would panic before returning a result.
    let zero = ModelRuntimeManager::with_factory(|_, _, _| {
        panic!("graph-only must not acquire a model runtime")
    });
    let request = IndexOptions {
        root: Some(dir.path().into()),
        ..IndexOptions::default()
    };
    let reused = service
        .index(&zero, request.clone())
        .await
        .expect("zero-model unchanged")
        .graph
        .expect("graph");
    assert_eq!(reused.mode, "unchanged");
    assert!(!reused.text_published);
    assert_eq!(reused.graph_build_id, graph.graph_build_id);
    assert_eq!(
        text_hashes(&first.storage_home().join("graph")),
        graph_before,
        "unchanged must not rewrite graph metadata or native files"
    );
    let mut changed = policy();
    changed.module_roots[0].namespace = "changed".into();
    let rebuilt_graph = service
        .index(
            &zero,
            IndexOptions {
                graph: Some(changed),
                ..request
            },
        )
        .await
        .expect("zero-model graph-only")
        .graph
        .expect("graph");
    assert_eq!(rebuilt_graph.mode, "graph_only");
    assert!(rebuilt_graph.graph_selected);
    assert!(!rebuilt_graph.text_published);
    assert_ne!(rebuilt_graph.graph_build_id, graph.graph_build_id);
    assert_eq!(text_hashes(&first.storage_home().join("storage")), before);
    assert_eq!(
        TextState::read(&first.storage_home()).expect("unchanged text state"),
        Some(state)
    );
    assert_eq!(calls.load(Ordering::Acquire), embeddings);
    assert_eq!(zero.snapshot().cached_runtimes, 0);
    // Explicit rebuild must select a new text generation together with its HEAD.
    initial.rebuild = true;
    initial.graph = None;
    let rebuilt = service.index(&model, initial).await.expect("joint rebuild");
    assert!(rebuilt.graph.expect("graph").graph_selected);
    let next = active(dir.path());
    assert_ne!(first.storage_generation, next.storage_generation);
    assert!(
        GraphHead::read(&next.storage_home())
            .expect("head")
            .is_some()
    );
    assert_eq!(
        GraphPolicy::read(&next.path)
            .expect("policy")
            .expect("intent")
            .configuration
            .module_roots[0]
            .namespace,
        "changed"
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real service lifecycle checks authorization, same-mtime updates and policy revocation"
)]
async fn graph_publication_same_mtime_content_change_off_and_cancel_before_policy() {
    let dir = tempfile::tempdir().expect("workspace");
    corpus(dir.path());
    let service = WorkspaceIndexService::with_test_registry();
    let calls = Arc::new(AtomicUsize::new(0));
    let model = models(&calls);
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service
        .index(&model, initial.clone())
        .await
        .expect("initial");
    let first = active(dir.path());
    let old_state = TextState::read(&first.storage_home())
        .expect("state")
        .expect("fence");
    let old_head = GraphHead::read(&first.storage_home()).expect("head");
    let path = dir.path().join("src/lib.rs");
    let timestamp = std::fs::metadata(&path)
        .expect("metadata")
        .modified()
        .expect("mtime");
    std::fs::write(
        &path,
        "\u{feff}fn other_() {}\r\nfn caller() { other_(); }\r\n",
    )
    .expect("same length changed code");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("file")
        .set_times(std::fs::FileTimes::new().set_modified(timestamp))
        .expect("restore mtime");
    let no_grant = IndexOptions {
        root: Some(dir.path().into()),
        ..IndexOptions::default()
    };
    let zero = ModelRuntimeManager::with_factory(|_, _, _| {
        panic!("authorization must precede model acquisition")
    });
    assert!(
        service.index(&zero, no_grant.clone()).await.is_err(),
        "changed bytes must take normal authorized text path"
    );
    assert_eq!(
        TextState::read(&first.storage_home()).expect("no text write before authorization"),
        Some(old_state.clone())
    );
    assert_eq!(
        GraphHead::read(&first.storage_home()).expect("head"),
        old_head
    );
    initial.graph = None;
    initial.embedding = None;
    initial.endpoint = None;
    let updated = service
        .index(&model, initial)
        .await
        .expect("authorized incremental update");
    assert!(updated.files_modified > 0);
    assert!(updated.graph.expect("graph").graph_selected);
    assert!(
        TextState::read(&first.storage_home())
            .expect("state")
            .expect("stable")
            .text_revision
            > old_state.text_revision
    );
    let before = std::fs::read(first.path.join("graph/POLICY.json")).expect("policy");
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        service
            .index(
                &zero,
                IndexOptions {
                    graph: Some(GraphOptions::default()),
                    signal: Some(cancel),
                    ..no_grant.clone()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(first.path.join("graph/POLICY.json")).expect("policy"),
        before
    );
    // Off is a policy change independent of the subsequent ordinary text
    // authorization failure. It must revoke selection without opening graphdb.
    assert!(
        service
            .index(
                &zero,
                IndexOptions {
                    graph: Some(GraphOptions::default()),
                    ..no_grant
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        GraphPolicy::read(&first.path)
            .expect("policy")
            .expect("intent")
            .configuration
            .scope,
        GraphScope::Off
    );
    assert!(
        GraphHead::read(&first.storage_home())
            .expect("revoked")
            .is_none()
    );
}

#[tokio::test]
async fn graph_coverage_reports_each_gap_and_recovers_after_source_and_policy_fix() {
    use crate::api::index::graph::{GraphReady, GraphSourceState};
    let dir = tempfile::tempdir().expect("workspace");
    corpus(dir.path());
    std::fs::create_dir(dir.path().join("extra")).expect("extra");
    std::fs::write(dir.path().join("extra/lib.rs"), "fn extra() {}\n").expect("extra source");
    std::fs::write(dir.path().join("src/broken.rs"), "fn broken( {\n").expect("broken source");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    let result = service
        .index(&manager, opts)
        .await
        .expect("text and partial graph");
    assert_eq!(result.files_failed, 0);
    let graph = result.graph.expect("graph");
    let code = &graph.domains["code"];
    assert_eq!(code.expected_coverage, Some(3));
    assert_eq!(code.coverage, 1);
    assert_eq!(
        code.ready,
        GraphReady::NotReady("incomplete_coverage".into())
    );
    assert_eq!(graph.domains["documents"].ready, GraphReady::Ready);
    let states = code
        .sources
        .iter()
        .map(|s| (s.source.as_deref().expect("UTF-8 fixture source"), s.state))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(states["extra/lib.rs"], GraphSourceState::NoModuleRoot);
    assert_eq!(states["src/broken.rs"], GraphSourceState::IncompleteFacts);
    std::fs::write(dir.path().join("src/broken.rs"), "fn repaired() {}\n").expect("repair");
    let mut graph_options = policy();
    graph_options.module_roots.push(GraphModuleRoot {
        root: "extra".into(),
        language: GraphLanguage::Rust,
        namespace: "extra".into(),
        module_prefix: vec![],
        layout: PackageLayout::Flat,
    });
    let mut opts = options(dir.path());
    opts.graph = Some(graph_options);
    let fixed = service
        .index(&manager, opts)
        .await
        .expect("repaired graph")
        .graph
        .expect("graph");
    let code = &fixed.domains["code"];
    assert_eq!(code.ready, GraphReady::Ready);
    assert_eq!(code.expected_coverage, Some(3));
    assert_eq!(code.coverage, 3);
    assert!(
        code.sources
            .iter()
            .all(|s| s.state == GraphSourceState::Ready)
    );
    let head = GraphHead::read(&active(dir.path()).storage_home())
        .expect("head")
        .expect("selected");
    assert_eq!(head.domains["code"], *code);
}
