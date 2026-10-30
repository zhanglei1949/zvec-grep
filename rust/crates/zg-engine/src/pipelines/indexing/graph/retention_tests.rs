use super::*;
use crate::{
    api::index::{
        IndexOptions,
        graph::{GraphRecoveryAction, GraphRecoveryOptions},
    },
    models::ModelRuntimeManager,
    pipelines::indexing::service::WorkspaceIndexService,
    workspace::manifest::read_workspace_manifest,
};

async fn fixture() -> (tempfile::TempDir, WorkspaceIndexService, WorkspaceManifest) {
    let dir = tempfile::tempdir().expect("workspace");
    let service = WorkspaceIndexService::with_test_registry();
    service
        .index(
            &ModelRuntimeManager::new(),
            IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(GraphOptions {
                    scope: GraphScope::Documents,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect("real dual-store build");
    let manifest = read_workspace_manifest(&dir.path().join(".zvec-grep"))
        .expect("manifest")
        .expect("selected");
    (dir, service, manifest)
}

#[tokio::test]
async fn graph_retention_at_capacity_reuses_current_but_refuses_a_new_candidate() {
    let (dir, service, manifest) = fixture().await;
    let builds = manifest.storage_home().join("graph/builds");
    let head =
        std::fs::read(manifest.storage_home().join("graph/HEAD.json")).expect("selected head");
    for _ in 0..7 {
        // Unknown/unconfirmed candidates also consume slots and are never pruned.
        std::fs::create_dir(builds.join(uuid::Uuid::new_v4().to_string()))
            .expect("retained candidate");
    }
    let zero = ModelRuntimeManager::new();
    let unchanged = service
        .index(
            &zero,
            IndexOptions {
                root: Some(dir.path().into()),
                ..Default::default()
            },
        )
        .await
        .expect("reuse needs no new space");
    assert_eq!(unchanged.graph.expect("graph").mode, "unchanged");
    let error = service
        .index(
            &zero,
            IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(GraphOptions {
                    scope: GraphScope::Both,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .expect_err("ninth candidate rejected");
    assert_eq!(error.code(), EngineError::INVALID_ARGUMENT);
    assert_eq!(std::fs::read_dir(&builds).expect("builds").count(), 8);
    assert_eq!(
        std::fs::read(manifest.storage_home().join("graph/HEAD.json")).expect("head"),
        head
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one native fixture compares known pre-root cleanup with unknown commit preservation"
)]
async fn graph_retention_discards_known_partial_schema_and_preserves_unknown() {
    let (dir, service, manifest) = fixture().await;
    let home = manifest.storage_home();
    let head = GraphHead::read(&home).expect("head").expect("selected");
    let input: BuildRecord = read_record(
        &home
            .join("graph/builds")
            .join(&head.graph_build_id)
            .join("build.json"),
    )
    .expect("input")
    .expect("record");
    let head_bytes = std::fs::read(home.join("graph/HEAD.json")).expect("head bytes");
    for stage in ["failed", "commit_pending"] {
        let id = uuid::Uuid::new_v4().to_string();
        let path = home.join("graph/builds").join(&id);
        let record = BuildRecord {
            graph_build_id: id.clone(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            ..input.clone()
        };
        write_record(&path.join("build.json"), &record).expect("registered input");
        // Actual native Schema/DDL, stopped before the first import progress row.
        let graph = GraphDbStore::create_batched_build(path.join("db"), GraphLimits::default())
            .expect("partial native schema");
        drop(graph);
        attempt(
            &path,
            &record,
            stage,
            if stage == "failed" {
                "not_applied"
            } else {
                "unknown"
            },
            "not_selected",
            false,
            None,
        )
        .expect("protocol state");
        let options = GraphRecoveryOptions {
            root: Some(dir.path().into()),
            graph_build_id: id.clone(),
            action: GraphRecoveryAction::Discard,
            expected_operation_id: Some(record.operation_id.clone()),
            expected_attempt: Some(record.attempt),
            ..Default::default()
        };
        if stage == "commit_pending" {
            assert!(
                service
                    .recover_graph(options)
                    .await
                    .expect_err("unknown native proof is not guessed")
                    .message()
                    .contains("recovery_required")
            );
            assert!(path.exists());
            continue;
        }
        let inspected = service
            .recover_graph(GraphRecoveryOptions {
                action: GraphRecoveryAction::Inspect,
                expected_operation_id: None,
                expected_attempt: None,
                ..options.clone()
            })
            .await
            .expect("inspect durable precommit identity without inventing native readiness");
        assert!(!inspected.facts_ready && !inspected.candidate_committed && !inspected.discarded);
        assert_eq!(inspected.operation_id, record.operation_id);
        assert!(
            inspected
                .diagnostics
                .iter()
                .any(|d| d == "native_candidate_not_inspected")
        );
        let done = service
            .recover_graph(options.clone())
            .await
            .expect("explicit known pre-root cleanup");
        assert!(done.discarded && !done.candidate_committed && !done.facts_ready);
        assert!(
            done.diagnostics
                .iter()
                .any(|d| d == "known_precommit_protocol_evidence")
        );
        assert!(!path.exists());
        assert!(
            service
                .recover_graph(options.clone())
                .await
                .expect("lost response retry")
                .discarded
        );
        let seen = service
            .recover_graph(GraphRecoveryOptions {
                action: GraphRecoveryAction::Inspect,
                expected_operation_id: None,
                expected_attempt: None,
                ..options
            })
            .await
            .expect("read-only tombstone inspection");
        assert!(seen.discarded);
    }
    assert_eq!(
        std::fs::read(home.join("graph/HEAD.json")).expect("selected unchanged"),
        head_bytes
    );
}
