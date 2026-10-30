//! Real indexing and both native stores; explore uses a fresh model-free engine.
use super::graph_indexing::*;
use super::*;
use crate::EngineError;
use crate::api::explore::*;
use crate::pipelines::indexing::service::WorkspaceIndexService;
fn request(root: &std::path::Path, operation: ExploreOperation) -> ExploreOptions {
    let mut options = ExploreOptions::new(ExploreRequest {
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
async fn explore_real_published_view_preserves_occurrences_and_avoids_models() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(dir.path().join("src")).expect("src");
    std::fs::write(dir.path().join("src/lib.rs"),"fn target() {}\r\nfn caller() { target(); target(); caller(); }\r\nfn root() { caller(); }\r\n").expect("code");
    let calls = Arc::new(AtomicUsize::new(0));
    let manager = models(&calls);
    let service = WorkspaceIndexService::with_test_registry();
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
    let service = WorkspaceIndexService::with_test_registry();
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
    let limited = engine.explore(limited).await.expect("bounded list");
    assert_eq!(limited.execution.status, Completion::Partial);
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
    let service = WorkspaceIndexService::with_test_registry();
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
    let service = WorkspaceIndexService::with_test_registry();
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
    let service = WorkspaceIndexService::with_test_registry();
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
    let model = embeddings[0].model.reference();
    let storage = IndexStore::open(WorkspaceIndexStorageOptions::ReadWrite {
        storage_path: home,
        embeddings,
    })
    .expect("writer");
    let file = storage.list_files().expect("files").remove(0);
    let ids = storage.file_entity_ids(&file).expect("IDs");
    let mut entities = storage.graph_entities(&ids).expect("entities");
    let entity = &mut entities[0];
    let ordinal = u32::try_from(entity.fragments.len()).expect("ordinal");
    entity.fragments.push(EntityFragment {
        id: FragmentId::new(&entity.id, ordinal),
        range: Range::Byte(ByteRange::new(0, 2).expect("fragment")),
    });
    let entries = entities
        .iter()
        .flat_map(|entity| {
            entity.fragments.iter().map(|fragment| IndexedFragment {
                entity_id: entity.id.clone(),
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
    let service = WorkspaceIndexService::with_test_registry();
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
    let service = WorkspaceIndexService::with_test_registry();
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
        let service = WorkspaceIndexService::with_test_registry();
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
