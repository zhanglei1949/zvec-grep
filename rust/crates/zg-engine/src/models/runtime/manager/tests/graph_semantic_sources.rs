use super::*;
use zg_knowledge::{
    graph::{GraphDbStore, GraphLimits},
    processing::ProcessingContext,
    semantic::{
        Episode, EpisodeState, Identity, SemanticCatalog, SemanticEntity, SemanticSource,
        SemanticSourceState, SourceRef,
    },
};

fn context() -> ProcessingContext {
    ProcessingContext {
        max_input_bytes: 64 << 20,
        max_output_items: 1_000_000,
        deadline: None,
        cancellation: tokio_util::sync::CancellationToken::new(),
    }
}
fn semantic() -> SemanticCatalog {
    let source = SourceRef {
        source_id: "external:document".into(),
        source_version: "v1".into(),
    };
    SemanticCatalog {
        sources: vec![SemanticSource {
            source: source.clone(),
            state: SemanticSourceState::Complete,
            semantic_version: Some("v1".into()),
        }],
        entities: vec![SemanticEntity {
            identity: Identity {
                space_id: "workspace".into(),
                id: "project".into(),
            },
            kind: "project".into(),
            name: "项目 A".into(),
            aliases: vec![],
            attributes: serde_json::from_value(serde_json::json!({"active":true,"tags":[1,"a"]}))
                .expect("attributes"),
        }],
        episodes: vec![Episode {
            identity: Identity {
                space_id: "workspace".into(),
                id: "episode".into(),
            },
            kind: "meeting".into(),
            source,
            context: "project".into(),
            occurred_at_ms: None,
            ingested_at_ms: 100,
            content_ref: "host:episode".into(),
            state: EpisodeState::Active,
        }],
        ..Default::default()
    }
}
async fn seed(root: &std::path::Path, data: SemanticCatalog) {
    let manifest = active(root);
    let id = unselected_candidate_with_facts(&manifest, false, true, |catalog| {
        catalog
            .with_semantic(data, &context())
            .expect("semantic candidate")
    });
    let engine = crate::ZvecGrep::new();
    let opts = recovery_options(root, &id);
    let seen = engine
        .recover_graph(opts.clone())
        .await
        .expect("candidate proof");
    let selected = engine
        .recover_graph(mutate_recovery(
            opts,
            &seen,
            crate::api::index::graph::GraphRecoveryAction::Select,
        ))
        .await
        .expect("select candidate");
    assert!(selected.selected && selected.eligible);
}
fn read(root: &std::path::Path) -> zg_knowledge::relations::bound::FactCatalog {
    let home = active(root).storage_home();
    let head = GraphHead::read(&home).expect("head").expect("selected");
    GraphDbStore::open(
        home.join("graph/builds")
            .join(head.graph_build_id)
            .join("db"),
        true,
        GraphLimits::default(),
    )
    .expect("reopen")
    .read_facts(&context())
    .expect("facts")
}

#[tokio::test]
async fn graph_code_rebuild_inherits_outside_semantics_and_episodes() {
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service
        .index(&manager, initial)
        .await
        .expect("initial graph");
    let expected = semantic();
    seed(dir.path(), expected.clone()).await;
    assert_eq!(read(dir.path()).semantic, expected);
    let before = calls.load(Ordering::Acquire);
    let mut code = policy();
    code.scope = GraphScope::Code;
    let opts = IndexOptions {
        root: Some(dir.path().into()),
        graph: Some(code),
        ..Default::default()
    };
    let changed = service
        .index(&manager, opts)
        .await
        .expect("code-only full candidate");
    assert!(changed.graph.expect("graph").graph_selected);
    assert_eq!(
        calls.load(Ordering::Acquire),
        before,
        "policy rebuild needs no model"
    );
    assert_eq!(
        read(dir.path()).semantic,
        expected,
        "outside semantic records survive the scope change"
    );
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn target() {}\nfn caller() { target(); target(); }\n",
    )
    .expect("changed code");
    service
        .index(&manager, options(dir.path()))
        .await
        .expect("code refresh");
    assert_eq!(read(dir.path()).semantic, expected);
}

fn document_data(root: &std::path::Path) -> SemanticCatalog {
    use zg_knowledge::{
        query::{ByteSpan, EvidenceRef},
        semantic::{
            Fact, FactObject, FactState, Mention, ObjectId, ScalarValue, SourceAssociation,
            SourceEvidence, SourceRole,
        },
    };
    let catalog = read(root);
    let facts = catalog
        .files
        .iter()
        .find_map(|file| {
            file.facts
                .as_ref()
                .filter(|f| f.version.path == "readme.md")
        })
        .expect("document facts");
    let mut data = semantic();
    let source = SourceRef {
        source_id: "readme.md".into(),
        source_version: "document-v1".into(),
    };
    data.sources.push(SemanticSource {
        source: source.clone(),
        state: SemanticSourceState::Complete,
        semantic_version: Some(source.source_version.clone()),
    });
    let identity = data.entities[0].identity.clone();
    let fact_id = Identity {
        space_id: "workspace".into(),
        id: "document-fact".into(),
    };
    let evidence = EvidenceRef {
        source_id: "readme.md".into(),
        raw_digest: facts.version.raw_digest.clone(),
        decoded_digest: facts.version.decoded_digest.clone(),
        span: ByteSpan { start: 0, end: 1 },
        reference_id: Some("document-mention".into()),
    };
    data.mentions.push(Mention {
        ref_id: "document-mention".into(),
        owner_id: facts.nodes[0].id.clone(),
        target: identity.clone(),
        evidence: evidence.clone(),
    });
    data.facts.push(Fact {
        created_at_ms: None,
        identity: fact_id.clone(),
        subject: identity.clone(),
        relation_type: "description".into(),
        object: FactObject::Scalar {
            value: ScalarValue::String("项目说明".into()),
        },
        context: "project".into(),
        state: FactState::Active,
        supersedes: None,
        changed_at_ms: 100,
        attributes: serde_json::Map::new(),
    });
    data.associations.extend([
        SourceAssociation {
            target: ObjectId::Fact { identity: fact_id },
            evidence: SourceEvidence {
                episode: None,
                source: source.clone(),
                locator: None,
            },
            role: SourceRole::Support,
        },
        SourceAssociation {
            target: ObjectId::SemanticEntity { identity },
            evidence: SourceEvidence {
                episode: None,
                source,
                locator: Some(evidence),
            },
            role: SourceRole::Support,
        },
    ]);
    data.normalize().expect("canonical directory");
    data
}

#[tokio::test]
async fn graph_code_scope_keeps_document_positions_as_auxiliaries() {
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service.index(&manager, initial).await.expect("initial");
    let mut expected = document_data(dir.path());
    seed(dir.path(), expected.clone()).await;
    expected.deferred_mentions = std::mem::take(&mut expected.mentions);
    let before = calls.load(Ordering::Acquire);
    let mut code = policy();
    code.scope = GraphScope::Code;
    let result = service
        .index(
            &manager,
            IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(code),
                ..Default::default()
            },
        )
        .await
        .expect("code scope rebuild");
    assert!(result.graph.expect("graph").graph_selected);
    let catalog = read(dir.path());
    assert_eq!(catalog.semantic, expected);
    assert_eq!(catalog.semantic.current_facts().count(), 1);
    assert_eq!(calls.load(Ordering::Acquire), before);
    let replacement = "# Changed document\n";
    std::fs::write(dir.path().join("readme.md"), replacement).expect("changed document");
    service
        .index(&manager, options(dir.path()))
        .await
        .expect("host version update without semantic result");
    let source = expected
        .sources
        .iter_mut()
        .find(|s| s.source.source_id == "readme.md")
        .expect("document source");
    source.source.source_version = crate::utils::sha256_hex(replacement.as_bytes());
    source.state = SemanticSourceState::Unspecified;
    assert_eq!(
        read(dir.path()).semantic,
        expected,
        "new body version retains the last complete contribution and old positions"
    );
}

#[tokio::test]
async fn graph_confirmed_file_removal_withdraws_only_its_contribution() {
    use zg_knowledge::semantic::{FactState, SourceRole};
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service.index(&manager, initial).await.expect("initial");
    let expected = document_data(dir.path());
    seed(dir.path(), expected.clone()).await;
    let before = calls.load(Ordering::Acquire);
    std::fs::remove_file(dir.path().join("readme.md")).expect("host source removed");
    let result = service
        .index(&manager, options(dir.path()))
        .await
        .expect("confirmed scan and refresh");
    assert!(result.graph.expect("graph").graph_selected);
    let data = read(dir.path()).semantic;
    assert_eq!(data.entities, expected.entities);
    assert_eq!(data.episodes, expected.episodes);
    assert!(data.current_facts().next().is_none());
    assert_eq!(data.facts[0].state, FactState::Active);
    assert!(data.mentions.is_empty() && data.deferred_mentions.is_empty());
    assert!(
        data.sources
            .iter()
            .any(|s| s.source.source_id == "readme.md" && s.state == SemanticSourceState::Deleted)
    );
    assert!(
        data.sources
            .iter()
            .any(|s| s.source.source_id == "external:document"
                && s.state == SemanticSourceState::Complete)
    );
    assert_eq!(data.associations.len(), expected.associations.len());
    assert!(
        data.associations
            .iter()
            .all(|a| a.role == SourceRole::Withdrawn
                && a.evidence.source.source_version == "document-v1")
    );
    assert_eq!(calls.load(Ordering::Acquire), before);
    std::fs::write(dir.path().join("readme.md"), "# Restored document\n").expect("restored source");
    service
        .index(&manager, options(dir.path()))
        .await
        .expect("source observed again without semantics");
    let restored = read(dir.path()).semantic;
    assert!(restored.current_facts().next().is_none());
    assert_eq!(restored.facts, data.facts);
    assert_eq!(restored.associations, data.associations);
    assert!(
        restored
            .sources
            .iter()
            .any(|s| s.source.source_id == "readme.md"
                && s.state == SemanticSourceState::Unspecified
                && s.semantic_version.is_none())
    );
}

#[tokio::test]
async fn graph_text_rebuild_keeps_semantics_across_storage_generations() {
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service.index(&manager, initial).await.expect("initial");
    let mut expected = document_data(dir.path());
    seed(dir.path(), expected.clone()).await;
    expected.deferred_mentions = std::mem::take(&mut expected.mentions);
    let old = active(dir.path()).storage_generation;
    let mut rebuild = options(dir.path());
    rebuild.rebuild = true;
    let mut code = policy();
    code.scope = GraphScope::Code;
    rebuild.graph = Some(code);
    let result = service
        .index(&manager, rebuild)
        .await
        .expect("full text and code rebuild");
    assert!(result.graph.expect("graph").graph_selected);
    assert_ne!(active(dir.path()).storage_generation, old);
    assert_eq!(
        read(dir.path()).semantic,
        expected,
        "new storage generation keeps existing semantic identities and provenance"
    );
}

#[tokio::test]
async fn graph_unknown_format_blocks_rebuild_and_keeps_selected_history() {
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service.index(&manager, initial).await.expect("initial");
    seed(dir.path(), semantic()).await;
    let manifest = active(dir.path());
    let mut old = manifest
        .publication
        .as_ref()
        .expect("publication")
        .graph
        .clone()
        .expect("graph");
    old.schema_fingerprint = crate::utils::sha256_hex(b"unknown-old-format");
    crate::storage::graph_state::write_record(
        &manifest.storage_home().join("graph/HEAD.json"),
        &old,
    )
    .expect("old marker");
    crate::workspace::manifest::write_workspace_manifest_with_graph(
        &manifest.path,
        &manifest,
        Some(old.clone()),
    )
    .expect("published old format");
    let before = std::fs::read(manifest.path.join("manifest.json")).expect("valid fixture/result");
    let embeddings = calls.load(Ordering::Acquire);
    let mut rebuild = options(dir.path());
    rebuild.rebuild = true;
    let error = service
        .index(&manager, rebuild)
        .await
        .expect_err("old format requires separate migration");
    assert_eq!(error.code(), EngineError::RECOVERY_REQUIRED);
    assert_eq!(
        std::fs::read(manifest.path.join("manifest.json")).expect("valid fixture/result"),
        before
    );
    assert!(
        manifest
            .storage_home()
            .join("graph/builds")
            .join(old.graph_build_id)
            .join("db")
            .is_dir()
    );
    assert_eq!(calls.load(Ordering::Acquire), embeddings);
}

#[tokio::test]
async fn graph_protected_history_blocks_text_only_rebuild() {
    let dir = tempfile::tempdir().expect("corpus");
    corpus(dir.path());
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service.index(&manager, initial).await.expect("initial");
    seed(dir.path(), semantic()).await;
    let manifest = active(dir.path());
    let before = std::fs::read(manifest.path.join("manifest.json")).expect("valid fixture/result");
    let mut rebuild = options(dir.path());
    rebuild.rebuild = true;
    rebuild.graph = Some(crate::api::index::graph::GraphOptions::default());
    assert_eq!(
        service
            .index(&manager, rebuild)
            .await
            .expect_err("cannot discard semantic history")
            .code(),
        EngineError::RECOVERY_REQUIRED
    );
    assert_eq!(
        std::fs::read(manifest.path.join("manifest.json")).expect("valid fixture/result"),
        before
    );
    let selected = manifest
        .publication
        .as_ref()
        .expect("valid fixture/result")
        .graph
        .as_ref()
        .expect("valid fixture/result");
    assert!(
        manifest
            .storage_home()
            .join("graph/builds")
            .join(&selected.graph_build_id)
            .join("db")
            .is_dir()
    );
}
