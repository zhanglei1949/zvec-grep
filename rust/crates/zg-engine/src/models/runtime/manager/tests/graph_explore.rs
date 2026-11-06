//! Real indexing and both native stores; explore uses a fresh model-free engine.
#[path = "graph_languages.rs"]
mod languages;
#[path = "graph_semantic_recovery.rs"]
mod semantic_recovery;
use super::graph_indexing::*;
use super::*;
use crate::EngineError;
use crate::api::explore::*;
use crate::pipelines::indexing::service::WorkspaceIndexService;
fn request(root: &std::path::Path, operation: ExploreOperation) -> ExploreOptions {
    let mut options = ExploreOptions::new(ExploreRequest {
        page_size: 64,
        cursor: None,
        relation_filters: Vec::new(),
        domain: QueryDomain::Code,
        operation,
        boundary: SourceFilter::default(),
        result_filter: SourceFilter::default(),
    });
    options.root = Some(root.into());
    options
}
fn named(name: &str) -> Locator {
    Locator::Name {
        name: name.into(),
        source_id: None,
        kind: None,
    }
}

#[tokio::test]
async fn nested_bare_import_publishes_and_reopens_two_hop_path_without_models() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(dir.path().join("src/code")).expect("modules");
    for (path, text) in [
        ("src/lib.rs", "mod code;mod lookup;"),
        (
            "src/code.rs",
            "mod lookup;use lookup::{module_path as path};pub fn resolve(){path();}",
        ),
        (
            "src/code/lookup.rs",
            "pub(super) fn module_path(){leaf();}fn leaf(){}",
        ),
        ("src/lookup.rs", "pub fn module_path(){}"),
    ] {
        std::fs::write(dir.path().join(path), text).expect("source");
    }
    let manager = models(&Arc::new(AtomicUsize::new(0)));
    let service = WorkspaceIndexService::new();
    service
        .index(&manager, options(dir.path()))
        .await
        .expect("text");
    let home = active(dir.path()).storage_home();
    let before = text_hashes(&home.join("storage"));
    let zero =
        ModelRuntimeManager::with_factory(|_, _, _| panic!("graph-only must not acquire models"));
    service
        .index(
            &zero,
            crate::api::index::IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(policy()),
                ..Default::default()
            },
        )
        .await
        .expect("graph-only");
    assert_eq!(text_hashes(&home.join("storage")), before);
    drop(service);
    let engine = crate::ZvecGrep::new();
    let imports = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Neighbors {
                anchor: Locator::Position {
                    source_id: "src/code.rs".into(),
                    byte_offset: 0,
                    kind: Some(EntityKind::File),
                },
                relations: vec![RelationKind::Imports],
                direction: Direction::Outgoing,
            },
        ))
        .await
        .expect("persisted import");
    assert_eq!(imports.relations.len(), 1);
    assert!(
        imports
            .nodes
            .iter()
            .any(|n| n.source_id == "src/code/lookup.rs" && n.name == "module_path")
    );
    let path = engine
        .explore(request(
            dir.path(),
            ExploreOperation::ShortestPath {
                start: named("resolve"),
                target: named("leaf"),
                traversal: TraversalSpec::default(),
            },
        ))
        .await
        .expect("two-hop path after reopen");
    assert_eq!(path.paths.len(), 1);
    assert_eq!(path.paths[0].relations.len(), 2);
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    assert_eq!(text_hashes(&home.join("storage")), before);
    engine.close();
}
#[tokio::test]
async fn explore_real_published_view_preserves_occurrences_and_avoids_models() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(dir.path().join("src/lib.rs"),"fn target() {}\r\nfn caller() { target(); target(); caller(); }\r\nfn root() { caller(); }\r\n").expect("code");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    let built = service.index(&manager, opts).await.expect("publish");
    assert_eq!(
        built.graph.expect("graph").domains["code"].ready,
        crate::api::index::graph::GraphReady::Ready
    );
    let before = calls.load(Ordering::Acquire);
    drop(service);
    let engine = crate::ZvecGrep::new();
    let outgoing = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Neighbors {
                anchor: named("caller"),
                relations: vec![RelationKind::Calls],
                direction: Direction::Outgoing,
            },
        ))
        .await
        .expect("callees");
    assert_eq!(
        outgoing.relations.len(),
        3,
        "two occurrences and self-recursion"
    );
    let incoming = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Neighbors {
                anchor: named("target"),
                relations: vec![RelationKind::Calls],
                direction: Direction::Incoming,
            },
        ))
        .await
        .expect("callers");
    assert_eq!(incoming.relations.len(), 2);
    let path = engine
        .explore(request(
            dir.path(),
            ExploreOperation::ShortestPath {
                start: named("root"),
                target: named("target"),
                traversal: TraversalSpec::default(),
            },
        ))
        .await
        .expect("path");
    assert_eq!(path.paths[0].relations.len(), 2);
    let refs = engine
        .explore(request(
            dir.path(),
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("caller"),
                },
            },
        ))
        .await
        .expect("refs");
    assert_eq!(refs.references.len(), 3);
    assert!(
        refs.evidence
            .iter()
            .all(|e| matches!(e, EvidenceRead::Verified { .. }))
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    assert_eq!(calls.load(Ordering::Acquire), before);
    engine.close();
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real published fixture covers independent read-only contract failures without rebuilding per assertion"
)]
async fn explore_locators_boundaries_budgets_and_stale_ids_are_explicit() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(dir.path().join("src/a.rs"),"// 中文😀\r\nfn target() {}\r\nfn caller() { target(); target(); missing(); }\r\nfn root() { caller(); }\r\n").expect("a");
    std::fs::write(dir.path().join("src/b.rs"), "fn target() {}\n").expect("b");
    let calls = Arc::new(AtomicUsize::new(0));
    let service = WorkspaceIndexService::new();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    service.index(&models(&calls), opts).await.expect("index");
    drop(service);
    let engine = crate::ZvecGrep::new();
    engine.enable_read_session_cache().expect("cache");
    let ambiguous = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Locate {
                target: named("target"),
            },
        ))
        .await
        .expect("ambiguous");
    assert_eq!(ambiguous.locations[0].status, LocateStatus::Ambiguous);
    assert_eq!(ambiguous.locations[0].candidates.len(), 2);
    let neighbors = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Neighbors {
                anchor: named("target"),
                relations: vec![RelationKind::Calls],
                direction: Direction::Incoming,
            },
        ))
        .await
        .expect("do not choose a candidate");
    assert!(neighbors.relations.is_empty());
    assert_eq!(neighbors.locations[0].status, LocateStatus::Ambiguous);
    let unique = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Locate {
                target: Locator::Name {
                    name: "target".into(),
                    source_id: Some("src/a.rs".into()),
                    kind: Some(EntityKind::Function),
                },
            },
        ))
        .await
        .expect("unique");
    let located = &unique.locations[0].candidates[0];
    assert_eq!(located.kind, EntityKind::Function);
    let position = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Locate {
                target: Locator::Position {
                    source_id: "src/a.rs".into(),
                    byte_offset: located.span.start + 3,
                    kind: None,
                },
            },
        ))
        .await
        .expect("position");
    assert_eq!(position.locations[0].candidates[0].node, located.node);
    assert_eq!(
        engine
            .explore(request(
                dir.path(),
                ExploreOperation::Locate {
                    target: Locator::Position {
                        source_id: "src/a.rs".into(),
                        byte_offset: 4,
                        kind: None
                    }
                }
            ))
            .await
            .expect_err("middle of UTF-8 character")
            .code(),
        EngineError::INVALID_ARGUMENT
    );
    let mut old = located.node.clone();
    old.graph_build_id = "old-build".into();
    assert_eq!(
        engine
            .explore(request(
                dir.path(),
                ExploreOperation::Locate {
                    target: Locator::Node { node: old }
                }
            ))
            .await
            .expect_err("stale NodeRef")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    let mut empty = request(
        dir.path(),
        ExploreOperation::Neighbors {
            anchor: named("caller"),
            relations: vec![RelationKind::Calls],
            direction: Direction::Outgoing,
        },
    );
    empty.request.boundary.sources = Some(vec![]);
    let empty = engine.explore(empty).await.expect("empty boundary");
    assert_eq!(empty.locations[0].status, LocateStatus::NotFound);
    assert!(empty.relations.is_empty());
    let mut limited = request(
        dir.path(),
        ExploreOperation::Neighbors {
            anchor: named("caller"),
            relations: vec![RelationKind::Calls],
            direction: Direction::Outgoing,
        },
    );
    limited.execution.budget.nodes = 1;
    assert_eq!(
        engine
            .explore(limited)
            .await
            .expect_err("budget cannot make resumable progress")
            .code(),
        EngineError::RESOURCE_BUSY
    );
    let mut path = request(
        dir.path(),
        ExploreOperation::ShortestPath {
            start: named("root"),
            target: Locator::Name {
                name: "target".into(),
                source_id: Some("src/a.rs".into()),
                kind: None,
            },
            traversal: TraversalSpec::default(),
        },
    );
    path.execution.budget.nodes = 1;
    assert!(
        engine.explore(path).await.is_err(),
        "incomplete path search is not no-path"
    );
    let refs = engine
        .explore(request(
            dir.path(),
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("caller"),
                },
            },
        ))
        .await
        .expect("references");
    assert_eq!(refs.references.len(), 3);
    assert!(
        refs.references
            .iter()
            .any(|r| r.status == ResolutionStatus::Unresolved)
    );
    let reference = refs.references[0].clone();
    let mut stamp = refs.view;
    stamp.text_revision += 1;
    assert!(
        engine
            .explore(request(
                dir.path(),
                ExploreOperation::References {
                    selector: ReferenceSelector::Id {
                        reference_id: reference.id,
                        view: stamp
                    }
                }
            ))
            .await
            .is_err()
    );
    let mut bounded = request(
        dir.path(),
        ExploreOperation::References {
            selector: ReferenceSelector::Owner {
                owner: named("caller"),
            },
        },
    );
    bounded.execution.budget.output_bytes = refs.execution.usage.output_bytes * 2 / 3;
    let bounded = engine.explore(bounded).await.expect("output truncation");
    assert_eq!(bounded.execution.status, Completion::Partial);
    assert!(bounded.references.len() < 3);
    let injection = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Locate {
                target: named("target') MATCH (n) RETURN n"),
            },
        ))
        .await
        .expect("parameterized name");
    assert_eq!(injection.locations[0].status, LocateStatus::NotFound);
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let mut stopped = request(
        dir.path(),
        ExploreOperation::Locate {
            target: named("caller"),
        },
    );
    stopped.signal = Some(token);
    assert_eq!(
        engine.explore(stopped).await.expect_err("cancelled").code(),
        EngineError::CANCELLED
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}

#[tokio::test]
async fn explore_python_references_follow_target_removal_and_restore() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("py")).expect("py");
    std::fs::write(
        dir.path().join("py/a.py"),
        "from b import target\ndef caller():\n    target()\n    target()\n",
    )
    .expect("a");
    let target = dir.path().join("py/b.py");
    std::fs::write(&target, "def target(): pass\n").expect("b");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut opts = options(dir.path());
    opts.graph = Some(crate::api::index::graph::GraphOptions {
        scope: crate::api::index::graph::GraphScope::Code,
        module_roots: vec![crate::api::index::graph::GraphModuleRoot {
            root: "py".into(),
            language: crate::api::index::graph::GraphLanguage::Python,
            namespace: "python".into(),
            module_prefix: vec![],
            layout: crate::api::index::graph::PackageLayout::Flat,
        }],
        rules: None,
    });
    service
        .index(&manager, opts.clone())
        .await
        .expect("publish");
    let engine = crate::ZvecGrep::new();
    let before = engine
        .explore(request(
            dir.path(),
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("caller"),
                },
            },
        ))
        .await
        .expect("resolved refs");
    assert_eq!(before.references.len(), 2);
    assert!(
        before
            .references
            .iter()
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
    std::fs::write(&target, "# removed\n").expect("remove target");
    service.index(&manager, opts.clone()).await.expect("update");
    let missing = engine
        .explore(request(
            dir.path(),
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("caller"),
                },
            },
        ))
        .await
        .expect("unresolved refs");
    assert_eq!(missing.references.len(), 2);
    assert!(missing.references.iter().all(|r| r.target.is_none()));
    assert_eq!(
        before.references.iter().map(|r| &r.id).collect::<Vec<_>>(),
        missing.references.iter().map(|r| &r.id).collect::<Vec<_>>()
    );
    std::fs::write(&target, "def target(): pass\n").expect("restore target");
    service.index(&manager, opts).await.expect("restore");
    let restored = engine
        .explore(request(
            dir.path(),
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("caller"),
                },
            },
        ))
        .await
        .expect("resolved again");
    assert!(
        restored
            .references
            .iter()
            .all(|r| r.status == ResolutionStatus::Resolved)
    );
    assert_ne!(before.view, restored.view);
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}

#[tokio::test]
async fn explore_is_readonly_and_never_bypasses_incomplete_code_coverage() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    let source = dir.path().join("src/lib.rs");
    std::fs::write(&source, "fn target() {}\nfn caller() { target(); }\n").expect("source");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    service
        .index(&manager, opts.clone())
        .await
        .expect("publish");
    drop(service);
    let manifest = active(dir.path());
    let before = text_hashes(&manifest.storage_home());
    let count = calls.load(Ordering::Acquire);
    let engine = crate::ZvecGrep::new();
    engine
        .explore(request(
            dir.path(),
            ExploreOperation::Neighbors {
                anchor: named("caller"),
                relations: vec![RelationKind::Calls],
                direction: Direction::Outgoing,
            },
        ))
        .await
        .expect("read");
    engine.close();
    assert_eq!(
        before,
        text_hashes(&manifest.storage_home()),
        "query preserves graph/text data and publication files"
    );
    assert_eq!(calls.load(Ordering::Acquire), count);
    std::fs::write(
        &source,
        "macro_rules! bind { ($x:ident) => { $x }; }\nfn target() {}\nfn caller() { let bind!(target) = || {}; target(); }\n",
    )
    .expect("unsupported binding");
    let service = WorkspaceIndexService::new();
    let result = service
        .index(&manager, opts)
        .await
        .expect("build incomplete code");
    assert_ne!(
        result.graph.expect("graph").domains["code"].ready,
        crate::api::index::graph::GraphReady::Ready
    );
    drop(service);
    let engine = crate::ZvecGrep::new();
    let count = calls.load(Ordering::Acquire);
    let failure = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Locate {
                target: named("target"),
            },
        ))
        .await
        .expect_err("not_ready");
    assert_eq!(failure.code(), EngineError::RESOURCE_BUSY);
    assert!(failure.to_string().contains("not_ready"));
    assert_eq!(calls.load(Ordering::Acquire), count);
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}

fn replace_fragment_structure(root: &std::path::Path) {
    use crate::domain::{ByteRange, EntityFragment, FragmentId, Range};
    use crate::storage::{
        IndexStore,
        types::{IndexedFragment, WorkspaceIndexStorageOptions},
    };
    let home = active(root).storage_home();
    let schema: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("storage/schema.json")).expect("schema"))
            .expect("schema JSON");
    let embeddings: Vec<crate::domain::model::EmbeddingModelInfo> =
        serde_json::from_value(schema["embeddings"].clone()).expect("embeddings");
    let model = embeddings[0].model().reference();
    let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadWrite {
        storage_path: home,
        embeddings,
    })
    .expect("writer");
    let file = storage.list_files().expect("files").remove(0);
    let ids = storage.file_entity_ids(&file).expect("IDs");
    let mut entities = storage.graph_entities(&ids).expect("entities");
    let entity = &mut entities[0];
    let ordinal = u32::try_from(entity.fragments().len()).expect("ordinal");
    let mut fragments = entity.fragments().to_vec();
    fragments.push(EntityFragment {
        id: FragmentId::new(entity.id(), ordinal),
        range: Range::Byte(ByteRange::new(0, 2).expect("fragment")),
    });
    *entity = crate::domain::Entity::new(
        entity.id().clone(),
        entity.file_id(),
        entity.source_range(),
        entity.content().clone(),
        entity.metadata().cloned(),
        fragments,
    )
    .expect("entity");
    let entries = entities
        .iter()
        .flat_map(|entity| {
            entity.fragments().iter().map(|fragment| IndexedFragment {
                entity_id: entity.id().clone(),
                fragment_id: fragment.id.clone(),
                model: model.clone(),
                vector: vec![1.0, 0.0],
                fts_text: "fixture".into(),
            })
        })
        .collect::<Vec<_>>();
    storage
        .replace_file(&file, &entities, &entries)
        .expect("replace fragments without changing entity IDs");
    storage.checkpoint().expect("stable text");
    storage.close().expect("close");
}

#[tokio::test]
async fn graph_only_fragment_mismatch_preserves_the_explicit_rebuild_hint() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "fn target() {}\nfn caller() { target(); }\n",
    )
    .expect("code");
    let calls = Arc::new(AtomicUsize::new(0));
    let service = WorkspaceIndexService::new();
    let mut opts = options(dir.path());
    opts.graph = Some(policy());
    service.index(&models(&calls), opts).await.expect("publish");
    replace_fragment_structure(dir.path());
    let before = text_hashes(&active(dir.path()).storage_home().join("storage"));
    let zero =
        ModelRuntimeManager::with_factory(|_, _, _| panic!("graph-only must not acquire a model"));
    let mut changed = policy();
    changed.module_roots[0].namespace = "changed".into();
    let result = service
        .index(
            &zero,
            crate::api::index::IndexOptions {
                root: Some(dir.path().into()),
                graph: Some(changed),
                ..Default::default()
            },
        )
        .await
        .expect("classified graph failure")
        .graph
        .expect("graph result");
    assert!(!result.graph_selected);
    assert_eq!(result.mode, "failed");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.contains("zg index --rebuild")),
        "{result:?}"
    );
    assert_eq!(
        before,
        text_hashes(&active(dir.path()).storage_home().join("storage"))
    );
    assert_eq!(zero.snapshot().cached_runtimes, 0);
}

#[tokio::test]
async fn code_bindings_and_globs_publish_reopen_and_recompute_without_relaxing_ready() {
    use crate::api::index::graph::{GraphLanguage, GraphModuleRoot, GraphReady, PackageLayout};
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("Rust root");
    std::fs::create_dir(dir.path().join("py")).expect("Python root");
    let rust = "fn rust_target(){}\nmod bridge {use super::*;pub fn relay(){rust_target();}}\nfn rust_caller(callback:Option<fn()>){use crate::bridge::*;if let Some(relay)=callback{relay();}relay();relay();}\n";
    std::fs::write(dir.path().join("src/lib.rs"), rust).expect("Rust source");
    std::fs::write(dir.path().join("py/app.py"), "def py_target(): pass\ndef outer():\n    def py_target(): pass\n    def py_caller(xs):\n        global py_target\n        return [py_target() for x in xs]\n").expect("Python source");
    std::fs::create_dir(dir.path().join("py/pkg")).expect("namespace package");
    std::fs::write(
        dir.path().join("py/pkg/calc.py"),
        "def py_module_target(): pass\n",
    )
    .expect("module");
    std::fs::write(
        dir.path().join("py/imports.py"),
        "import pkg.calc\ndef py_import_caller(): pkg.calc.py_module_target()\n",
    )
    .expect("dotted import");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::new();
    let opts = options(dir.path());
    service
        .index(&manager, opts.clone())
        .await
        .expect("text index");
    let home = active(dir.path()).storage_home();
    let text_before = text_hashes(&home.join("storage"));
    let embedding_count = calls.load(Ordering::Acquire);
    let mut graph = policy();
    graph.module_roots.push(GraphModuleRoot {
        root: "py".into(),
        language: GraphLanguage::Python,
        namespace: "python-fixture".into(),
        module_prefix: vec![],
        layout: PackageLayout::Flat,
    });
    let graph_only = crate::api::index::IndexOptions {
        root: Some(dir.path().into()),
        graph: Some(graph.clone()),
        ..crate::api::index::IndexOptions::default()
    };
    let zero =
        ModelRuntimeManager::with_factory(|_, _, _| panic!("graph-only must not acquire models"));
    let published = service
        .index(&zero, graph_only)
        .await
        .expect("graph-only publish")
        .graph
        .expect("graph result");
    assert_eq!(published.mode, "graph_only");
    assert_eq!(published.domains["code"].ready, GraphReady::Ready);
    assert_eq!(calls.load(Ordering::Acquire), embedding_count);
    assert_eq!(text_hashes(&home.join("storage")), text_before);
    drop(service);
    assert_coverage_queries(dir.path()).await;
    let mut update = opts;
    update.graph = Some(graph);
    for (source, expected) in [
        (rust.replace("fn relay()", "fn removed()"), 0),
        (rust.into(), 2),
    ] {
        std::fs::write(dir.path().join("src/lib.rs"), source).expect("change export");
        let service = WorkspaceIndexService::new();
        let result = service
            .index(&manager, update.clone())
            .await
            .expect("recompute imported names");
        assert_eq!(
            result.graph.expect("graph").domains["code"].ready,
            GraphReady::Ready
        );
        drop(service);
        let reader = crate::ZvecGrep::new();
        let outgoing = reader
            .explore(request(
                dir.path(),
                ExploreOperation::Neighbors {
                    anchor: named("rust_caller"),
                    relations: vec![RelationKind::Calls],
                    direction: Direction::Outgoing,
                },
            ))
            .await
            .expect("updated calls");
        assert_eq!(outgoing.relations.len(), expected);
        reader.close();
    }
}

async fn assert_coverage_queries(root: &std::path::Path) {
    let engine = crate::ZvecGrep::new();
    for (name, count) in [
        ("rust_caller", 2),
        ("py_caller", 1),
        ("py_import_caller", 1),
    ] {
        let result = engine
            .explore(request(
                root,
                ExploreOperation::Neighbors {
                    anchor: named(name),
                    relations: vec![RelationKind::Calls],
                    direction: Direction::Outgoing,
                },
            ))
            .await
            .expect("reopened calls");
        assert_eq!(result.relations.len(), count);
    }
    let path = engine
        .explore(request(
            root,
            ExploreOperation::ShortestPath {
                start: named("rust_caller"),
                target: named("rust_target"),
                traversal: TraversalSpec::default(),
            },
        ))
        .await
        .expect("glob path");
    assert_eq!(path.paths[0].relations.len(), 2);
    let refs = engine
        .explore(request(
            root,
            ExploreOperation::References {
                selector: ReferenceSelector::Owner {
                    owner: named("rust_caller"),
                },
            },
        ))
        .await
        .expect("resolved and unresolved occurrences");
    assert_eq!(
        refs.references.len(),
        4,
        "one import, one callback and two static calls"
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
    drop(engine);
}

fn semantic_input(name: &str) -> zg_knowledge::semantic::SemanticInput {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../zg-knowledge/tests/fixtures/semantic-source-changes-v1.json"
    )))
    .expect("frozen inputs");
    serde_json::from_value(
        fixture["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|case| case["name"] == name)
            .expect("case")["input"]
            .clone(),
    )
    .expect("input")
}
fn semantic_options(
    root: &std::path::Path,
    name: &str,
) -> crate::api::index::graph::SemanticBuildOptions {
    let manifest = active(root);
    let head = crate::storage::graph_state::GraphHead::read(&manifest.storage_home())
        .expect("head")
        .expect("selected");
    crate::api::index::graph::SemanticBuildOptions {
        root: Some(root.into()),
        generation: manifest.storage_generation.expect("generation"),
        operation_id: uuid::Uuid::new_v4().to_string(),
        expected_graph_build_id: head.graph_build_id,
        input: semantic_input(name),
        lock_timeout_ms: None,
        signal: None,
    }
}
fn semantic_request(root: &std::path::Path, history: bool) -> ExploreOptions {
    let mut options = request(
        root,
        ExploreOperation::Semantic {
            query: SemanticQuery {
                space_id: "workspace".into(),
                anchor: None,
                context: Some("project-a".into()),
                name: None,
                kind: None,
                relation_type: Some("owner".into()),
                history,
                direction: Direction::Both,
            },
        },
    );
    options.request.domain = QueryDomain::Semantic;
    options
}
fn fact_ids(result: &ExploreResult) -> Vec<&str> {
    result
        .semantic
        .as_ref()
        .expect("semantic output")
        .objects
        .iter()
        .filter_map(|o| match &o.record {
            SemanticValue::Fact(f) => Some(f.identity.id.as_str()),
            _ => None,
        })
        .collect()
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "keep replay, source retirement, cached episode withdrawal and reopen in one admitted lifecycle"
)]
async fn m3_semantic_operations_publish_reopen_and_keep_predecessors_retired() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(dir.path().join("src/a.rs"), "fn entry() {}\n").expect("source");
    let calls = Arc::new(AtomicUsize::new(0));
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(policy());
    service
        .index(&models(&calls), initial)
        .await
        .expect("index");
    let before = text_hashes(&active(dir.path()).storage_home().join("storage"));
    drop(service);
    let engine = crate::ZvecGrep::new();
    engine.enable_read_session_cache().expect("cache");
    let mut operation = semantic_options(dir.path(), "initial");
    let source = operation.input.sources[0].source().clone();
    let episode_id = zg_knowledge::semantic::Identity {
        space_id: "workspace".into(),
        id: "episode".into(),
    };
    operation
        .input
        .episodes
        .push(zg_knowledge::semantic::Episode {
            identity: episode_id.clone(),
            kind: "meeting".into(),
            source: source.clone(),
            context: "project-a".into(),
            occurred_at_ms: None,
            ingested_at_ms: 1,
            content_ref: "host:episode".into(),
            state: zg_knowledge::semantic::EpisodeState::Active,
        });
    engine
        .apply_semantics(operation.clone())
        .await
        .expect("semantic write");
    assert_eq!(
        engine
            .apply_semantics(operation.clone())
            .await
            .expect("same attempt")
            .mode,
        "semantic_replayed"
    );
    let mut conflict = operation;
    conflict.input = semantic_input("second-source");
    assert!(engine.apply_semantics(conflict).await.is_err());
    let original = engine
        .explore(semantic_request(dir.path(), false))
        .await
        .expect("current F1");
    assert_eq!(fact_ids(&original), ["f1"]);
    engine
        .apply_semantics(semantic_options(dir.path(), "explicit-correction"))
        .await
        .expect("correction");
    assert_eq!(
        fact_ids(
            &engine
                .explore(semantic_request(dir.path(), false))
                .await
                .expect("current F2")
        ),
        ["f2"]
    );
    let mut removal = semantic_options(dir.path(), "explicit-correction");
    removal.input.sources = vec![zg_knowledge::semantic::SourceChange::Delete {
        source: removal.input.sources[0].source().clone(),
    }];
    removal.input.fact_changes.clear();
    engine
        .apply_semantics(removal)
        .await
        .expect("withdraw source");
    assert!(
        fact_ids(
            &engine
                .explore(semantic_request(dir.path(), false))
                .await
                .expect("unsupported F2")
        )
        .is_empty()
    );
    let mut stale = semantic_request(dir.path(), false);
    if let ExploreOperation::Semantic { query } = &mut stale.request.operation {
        query.anchor = original
            .semantic
            .expect("original")
            .objects
            .into_iter()
            .find(|o| matches!(o.record, SemanticValue::Fact(_)))
            .map(|o| o.reference);
    }
    assert!(engine.explore(stale).await.is_err());
    let mut all = semantic_request(dir.path(), false);
    if let ExploreOperation::Semantic { query } = &mut all.request.operation {
        query.relation_type = None;
    }
    assert!(
        engine
            .explore(all.clone())
            .await
            .expect("active episode")
            .semantic
            .expect("semantic")
            .objects
            .iter()
            .any(|o| matches!(o.record, SemanticValue::Episode(_)))
    );
    let mut withdrawal = semantic_options(dir.path(), "initial");
    withdrawal.input.sources.clear();
    withdrawal
        .input
        .episode_withdrawals
        .push(zg_knowledge::semantic::EpisodeWithdrawal {
            target: episode_id,
            expected_state: zg_knowledge::semantic::EpisodeState::Active,
            context: "project-a".into(),
            changed_at_ms: 2,
            evidence: zg_knowledge::semantic::SourceEvidence {
                episode: None,
                source,
                locator: None,
            },
        });
    engine
        .apply_semantics(withdrawal)
        .await
        .expect("withdraw episode");
    assert!(
        engine
            .explore(all)
            .await
            .expect("fresh cached view")
            .semantic
            .expect("semantic")
            .objects
            .iter()
            .all(|o| !matches!(o.record, SemanticValue::Episode(_)))
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
    let reopened = crate::ZvecGrep::new();
    assert!(
        fact_ids(
            &reopened
                .explore(semantic_request(dir.path(), false))
                .await
                .expect("reopen current")
        )
        .is_empty()
    );
    let history = reopened
        .explore(semantic_request(dir.path(), true))
        .await
        .expect("reopen history");
    assert_eq!(fact_ids(&history), ["f1", "f2"]);
    assert_eq!(history.semantic.expect("history").changes.len(), 1);
    assert_eq!(
        text_hashes(&active(dir.path()).storage_home().join("storage")),
        before
    );
    reopened.close();
}

#[tokio::test]
async fn m3_type_relations_publish_with_real_endpoints_and_declaration_positions() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "mod types;use crate::types::{Local,Child};impl Child for Local {}\n",
    )
    .expect("impl source");
    std::fs::write(
        dir.path().join("src/types.rs"),
        "pub trait Base {} pub trait Child: Base {} pub struct Local;\n",
    )
    .expect("types");
    let mut configuration = policy();
    configuration.module_roots = vec![crate::api::index::graph::GraphModuleRoot {
        root: "src".into(),
        language: crate::api::index::graph::GraphLanguage::Rust,
        namespace: "fixture".into(),
        module_prefix: vec![],
        layout: crate::api::index::graph::PackageLayout::Flat,
    }];
    let calls = Arc::new(AtomicUsize::new(0));
    let service = WorkspaceIndexService::new();
    let mut initial = options(dir.path());
    initial.graph = Some(configuration);
    service
        .index(&models(&calls), initial)
        .await
        .expect("index");
    drop(service);
    let engine = crate::ZvecGrep::new();
    let reply = engine
        .explore(request(
            dir.path(),
            ExploreOperation::Expand {
                anchor: named("Local"),
                traversal: TraversalSpec {
                    relations: vec![RelationKind::Implements, RelationKind::Extends],
                    min_hops: 1,
                    max_hops: 2,
                    ..Default::default()
                },
            },
        ))
        .await
        .expect("type expansion");
    assert_eq!(reply.relations.len(), 2);
    assert!(
        reply
            .nodes
            .iter()
            .chain(reply.locations.iter().flat_map(|l| &l.candidates))
            .any(|n| n.name == "Base")
    );
    let implementation = reply
        .relations
        .iter()
        .find(|r| r.kind == RelationKind::Implements)
        .expect("impl");
    assert_eq!(
        implementation
            .reference
            .as_ref()
            .expect("declaration")
            .evidence
            .source_id,
        "src/lib.rs"
    );
    assert!(
        reply
            .nodes
            .iter()
            .chain(reply.locations.iter().flat_map(|l| &l.candidates))
            .any(|n| n.node.node_id == implementation.source && n.name == "Local")
    );
    let refs = engine
        .explore(request(
            dir.path(),
            ExploreOperation::References {
                selector: ReferenceSelector::Source {
                    source_id: "src/lib.rs".into(),
                },
            },
        ))
        .await
        .expect("type references");
    assert!(
        refs.references
            .iter()
            .any(|r| r.kind == RelationKind::Implements)
    );
    engine.close();
}

#[tokio::test]
async fn m3_python_base_updates_publish_missing_and_ambiguous_references() {
    use zg_knowledge::relations::ResolutionStatus;
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    let path = dir.path().join("src/a.py");
    let mut configuration = policy();
    configuration.module_roots = vec![crate::api::index::graph::GraphModuleRoot {
        root: "src".into(),
        language: crate::api::index::graph::GraphLanguage::Python,
        namespace: "fixture".into(),
        module_prefix: vec![],
        layout: crate::api::index::graph::PackageLayout::Flat,
    }];
    let service = WorkspaceIndexService::new();
    let manager = models(&Arc::new(AtomicUsize::new(0)));
    for (source, expected) in [
        (
            "class Base: pass\nclass Child(Base): pass\n",
            ResolutionStatus::Resolved,
        ),
        ("class Child(Base): pass\n", ResolutionStatus::Unresolved),
        (
            "class Base: pass\nclass Base: pass\nclass Child(Base): pass\n",
            ResolutionStatus::Ambiguous,
        ),
    ] {
        std::fs::write(&path, source).expect("source version");
        let mut index = options(dir.path());
        index.graph = Some(configuration.clone());
        assert!(
            service
                .index(&manager, index)
                .await
                .expect("publish")
                .graph
                .expect("graph")
                .graph_selected
        );
        let engine = crate::ZvecGrep::new();
        let references = engine
            .explore(request(
                dir.path(),
                ExploreOperation::References {
                    selector: ReferenceSelector::Source {
                        source_id: "src/a.py".into(),
                    },
                },
            ))
            .await
            .expect("published references");
        let bases = references
            .references
            .iter()
            .filter(|r| r.kind == RelationKind::Extends)
            .collect::<Vec<_>>();
        assert_eq!(bases.len(), 1);
        assert_eq!(bases[0].status, expected);
        assert_eq!(
            bases[0].target.is_some(),
            expected == ResolutionStatus::Resolved
        );
        assert_eq!(
            bases[0].evidence.span.slice(source).expect("base location"),
            "Base"
        );
        assert_eq!(engine.runtime_snapshot().loaded_models, 0);
        engine.close();
    }
}
