//! Each language is admitted, persisted and read through the real dual-store host.
use super::*;
use crate::api::index::{
    IndexOptions,
    graph::{GraphOptions, GraphReady},
};
use crate::storage::graph_state::GraphHead;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    graph: GraphOptions,
    sources: Vec<Source>,
    paths: Vec<ExpectedPath>,
    calls: Vec<Call>,
}
#[derive(Deserialize)]
struct Source {
    path: String,
    text: String,
}
#[derive(Deserialize)]
struct ExpectedPath {
    anchor: String,
    target: String,
    marker: String,
}
#[derive(Deserialize)]
struct Call {
    source: String,
    span: ByteSpan,
    text: String,
    target: String,
    target_source: String,
}
struct Indexed {
    dir: tempfile::TempDir,
    service: WorkspaceIndexService,
    manager: ModelRuntimeManager,
    spec: Fixture,
}
impl Indexed {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("workspace");
        let spec: Fixture = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/compiled-languages-v1.json"
        )))
        .expect("frozen fixture");
        for source in &spec.sources {
            let path = dir.path().join(&source.path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("directories");
            std::fs::write(path, &source.text).expect("source");
        }
        let service = WorkspaceIndexService::with_test_registry();
        let manager = models(&Arc::new(AtomicUsize::new(0)));
        let mut initial = options(dir.path());
        initial.graph = Some(spec.graph.clone());
        initial.scan.globs = Some(vec!["!**/go.mod".into(), "!compile_commands.json".into()]);
        let result = service
            .index(&manager, initial)
            .await
            .expect("real publication");
        assert_eq!(
            result.graph.expect("graph").domains["code"].ready,
            GraphReady::Ready
        );
        Self {
            dir,
            service,
            manager,
            spec,
        }
    }
    fn root(&self) -> &std::path::Path {
        self.dir.path()
    }
    fn graph_only(&self) -> IndexOptions {
        IndexOptions {
            root: Some(self.root().into()),
            ..Default::default()
        }
    }
}
async fn paths(engine: &crate::ZvecGrep, indexed: &Indexed) {
    for case in &indexed.spec.paths {
        let result = engine
            .explore(request(
                indexed.root(),
                ExploreOperation::ShortestPath {
                    start: named(&case.anchor),
                    target: named(&case.target),
                    traversal: TraversalSpec {
                        min_hops: 2,
                        max_hops: 2,
                        ..Default::default()
                    },
                },
            ))
            .await
            .expect("published two-hop path");
        assert_eq!(result.execution.status, Completion::Complete);
        assert_eq!(result.paths.len(), 1, "{}", case.anchor);
        assert_eq!(result.paths[0].relations.len(), 2);
        assert!(
            result
                .evidence
                .iter()
                .all(|e| matches!(e, EvidenceRead::Verified { .. }))
        );
    }
}
async fn call_sites(engine: &crate::ZvecGrep, indexed: &Indexed) {
    for call in &indexed.spec.calls {
        let references = engine
            .explore(request(
                indexed.root(),
                ExploreOperation::References {
                    selector: ReferenceSelector::Source {
                        source_id: call.source.clone(),
                    },
                },
            ))
            .await
            .expect("references");
        let found = references
            .references
            .iter()
            .find(|r| r.kind == RelationKind::Calls && r.evidence.span == call.span)
            .expect("independently labelled call position");
        assert_eq!(
            found.status,
            zg_knowledge::relations::ResolutionStatus::Resolved
        );
        let target = engine
            .explore(request(
                indexed.root(),
                ExploreOperation::Locate {
                    target: Locator::Name {
                        name: call.target.clone(),
                        source_id: Some(call.target_source.clone()),
                        kind: None,
                    },
                },
            ))
            .await
            .expect("definition");
        assert_eq!(target.locations[0].candidates.len(), 1);
        assert_eq!(
            found.target.as_deref(),
            Some(target.locations[0].candidates[0].node.node_id.as_str())
        );
        let source = indexed
            .spec
            .sources
            .iter()
            .find(|s| s.path == call.source)
            .expect("source")
            .text
            .trim_start_matches('\u{feff}');
        assert_eq!(call.span.slice(source).expect("UTF-8 span"), call.text);
    }
}
#[tokio::test]
async fn m4_m6_four_languages_publish_reopen_and_preserve_position_evidence() {
    let indexed = Indexed::new().await;
    let before = text_hashes(&active(indexed.root()).storage_home().join("storage"));
    let zero =
        ModelRuntimeManager::with_factory(|_, _, _| panic!("graph-only must not acquire a model"));
    assert!(
        indexed
            .service
            .index(&zero, indexed.graph_only())
            .await
            .expect("graph-only")
            .graph
            .expect("graph")
            .graph_selected
    );
    let engine = crate::ZvecGrep::new();
    paths(&engine, &indexed).await;
    call_sites(&engine, &indexed).await;
    for (anchor, kinds, count) in [
        (
            "JavaEntry",
            vec![RelationKind::Extends, RelationKind::Implements],
            2,
        ),
        ("CppChild", vec![RelationKind::Extends], 2),
        ("GoCounter", vec![RelationKind::Implements], 0),
    ] {
        let result = engine
            .explore(request(
                indexed.root(),
                ExploreOperation::Neighbors {
                    anchor: named(anchor),
                    relations: kinds,
                    direction: Direction::Outgoing,
                },
            ))
            .await
            .expect("type relationships");
        assert_eq!(result.relations.len(), count);
    }
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
    let reopened = crate::ZvecGrep::new();
    paths(&reopened, &indexed).await;
    reopened.close();
    assert_eq!(
        text_hashes(&active(indexed.root()).storage_home().join("storage")),
        before
    );
}
#[tokio::test]
async fn m4_m6_configuration_drift_rebuilds_without_embedding_and_failure_keeps_head() {
    let indexed = Indexed::new().await;
    let before = text_hashes(&active(indexed.root()).storage_home().join("storage"));
    let path = indexed.root().join("compile_commands.json");
    let original = std::fs::read(&path).expect("commands");
    let mut commands: serde_json::Value = serde_json::from_slice(&original).expect("commands");
    commands[0]["arguments"]
        .as_array_mut()
        .expect("arguments")
        .push("-DADDED=1".into());
    let changed = serde_json::to_vec(&commands).expect("changed configuration");
    std::fs::write(&path, &changed).expect("configuration drift");
    let engine = crate::ZvecGrep::new();
    assert!(
        engine
            .explore(request(
                indexed.root(),
                ExploreOperation::Locate {
                    target: named("cEntry")
                }
            ))
            .await
            .is_err()
    );
    engine.close();
    let zero = ModelRuntimeManager::with_factory(|_, _, _| {
        panic!("configuration-only build must not embed")
    });
    let result = indexed
        .service
        .index(&zero, indexed.graph_only())
        .await
        .expect("graph rebuild")
        .graph
        .expect("graph");
    assert!(result.graph_selected);
    assert_ne!(result.mode, "unchanged");
    assert_eq!(result.domains["code"].ready, GraphReady::Ready);
    let home = active(indexed.root()).storage_home();
    let head = GraphHead::read(&home).expect("head");
    std::fs::write(&path, "{broken").expect("invalid configuration");
    assert!(
        indexed
            .service
            .index(&zero, indexed.graph_only())
            .await
            .is_err()
    );
    assert_eq!(GraphHead::read(&home).expect("head"), head);
    std::fs::write(&path, &changed).expect("restore same bytes");
    let engine = crate::ZvecGrep::new();
    paths(&engine, &indexed).await;
    engine.close();
    assert_eq!(text_hashes(&home.join("storage")), before);
}
#[tokio::test]
async fn m4_m6_off_augment_and_constrain_keep_the_same_retrieval_contract() {
    use crate::api::context::{
        ContextOptions,
        graph::{BodyRegion, GraphQueryMode, ScopeSpec},
        options::{ContextRoute, ContextRouteMode, RefreshPolicy},
    };
    let indexed = Indexed::new().await;
    let engine = crate::ZvecGrep::new();
    for case in &indexed.spec.paths {
        let mut query = ContextOptions {
            root: Some(indexed.root().into()),
            auto_update: false,
            refresh: Some(RefreshPolicy::Off),
            routes: vec![ContextRoute {
                mode: ContextRouteMode::Fts,
                query: case.marker.clone(),
            }],
            ..Default::default()
        };
        let ordinary = engine.context(query.clone()).await.expect("off");
        assert!(!ordinary.items.is_empty());
        query.graph = GraphQueryMode::Augment {
            seeds: 3,
            expansion: TraversalSpec::default(),
        };
        let augmented = engine.context(query.clone()).await.expect("augment");
        assert_eq!(ordinary.items, augmented.items);
        query.graph = GraphQueryMode::Constrain {
            scope: ScopeSpec {
                anchor: named(&case.anchor),
                traversal: TraversalSpec {
                    min_hops: 2,
                    max_hops: 2,
                    ..Default::default()
                },
                region: BodyRegion::Full,
            },
        };
        let constrained = engine.context(query).await.expect("exact scope");
        assert!(!constrained.items.is_empty());
        assert!(
            constrained
                .items
                .iter()
                .all(|item| item.content.contains(&case.target))
        );
    }
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}

fn leaf_sources() -> [(&'static str, &'static str, &'static str); 4] {
    [
        ("go/util/leaf.go", "GoLeaf", "GoGone"),
        ("java/b/JavaBase.java", "javaLeaf", "javaGone"),
        ("c/helpers.h", "cLeaf", "cGone"),
        ("cpp/types.hpp", "cppLeaf", "cppGone"),
    ]
}
async fn leaf_statuses(indexed: &Indexed, status: zg_knowledge::relations::ResolutionStatus) {
    let engine = crate::ZvecGrep::new();
    for (source, name, _) in leaf_sources() {
        let site = indexed
            .spec
            .calls
            .iter()
            .find(|call| call.target == name)
            .expect("frozen call");
        let current =
            std::fs::read_to_string(indexed.root().join(&site.source)).expect("current source");
        let decoded = current.trim_start_matches('\u{feff}');
        let start = decoded
            .find(&site.text)
            .expect("independent updated call location");
        let span = ByteSpan {
            start: start.try_into().expect("offset"),
            end: (start + site.text.len()).try_into().expect("offset"),
        };
        let result = engine
            .explore(request(
                indexed.root(),
                ExploreOperation::References {
                    selector: ReferenceSelector::Source {
                        source_id: site.source.clone(),
                    },
                },
            ))
            .await
            .expect("updated references");
        let reference = result
            .references
            .iter()
            .find(|r| r.evidence.span == span && r.kind == RelationKind::Calls)
            .expect("same call location");
        assert_eq!(reference.status, status, "{source}");
        assert!(reference.target.is_none());
    }
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}
#[tokio::test]
async fn m4_m6_target_removal_and_ambiguity_recompute_all_four_languages() {
    use zg_knowledge::relations::ResolutionStatus;
    let indexed = Indexed::new().await;
    let engine = crate::ZvecGrep::new();
    let original = engine
        .explore(request(
            indexed.root(),
            ExploreOperation::Locate {
                target: named("cLeaf"),
            },
        ))
        .await
        .expect("old node")
        .locations
        .remove(0)
        .candidates
        .remove(0)
        .node;
    engine.close();
    for (source, name, replacement) in leaf_sources() {
        let text = &indexed
            .spec
            .sources
            .iter()
            .find(|s| s.path == source)
            .expect("source")
            .text;
        let declaration = if std::path::Path::new(source)
            .extension()
            .and_then(|p| p.to_str())
            == Some("go")
        {
            format!("func {name}")
        } else {
            format!("void {name}")
        };
        assert!(text.contains(&declaration));
        std::fs::write(
            indexed.root().join(source),
            text.replacen(&declaration, &declaration.replace(name, replacement), 1),
        )
        .expect("remove definition");
    }
    assert!(
        indexed
            .service
            .index(&indexed.manager, options(indexed.root()))
            .await
            .expect("publish changed targets")
            .graph
            .expect("graph")
            .graph_selected
    );
    leaf_statuses(&indexed, ResolutionStatus::Unresolved).await;
    let engine = crate::ZvecGrep::new();
    assert!(
        engine
            .explore(request(
                indexed.root(),
                ExploreOperation::Neighbors {
                    anchor: Locator::Node { node: original },
                    relations: vec![RelationKind::Calls],
                    direction: Direction::Outgoing
                }
            ))
            .await
            .is_err()
    );
    engine.close();
    add_ambiguities(&indexed);
    assert!(
        indexed
            .service
            .index(&indexed.manager, options(indexed.root()))
            .await
            .expect("publish ambiguity")
            .graph
            .expect("graph")
            .graph_selected
    );
    leaf_statuses(&indexed, ResolutionStatus::Ambiguous).await;
}

#[tokio::test]
async fn m4_m6_code_configuration_drift_keeps_semantic_reads_and_blocks_stale_writes() {
    let indexed = Indexed::new().await;
    let engine = crate::ZvecGrep::new();
    engine
        .apply_semantics(semantic_options(indexed.root(), "initial"))
        .await
        .expect("semantic seed");
    let home = active(indexed.root()).storage_home();
    let before = text_hashes(&home.join("storage"));
    let head = GraphHead::read(&home).expect("head");
    let path = indexed.root().join("compile_commands.json");
    let mut bytes = std::fs::read(&path).expect("configuration");
    bytes.push(b'\n');
    std::fs::write(&path, bytes).expect("code configuration drift");
    assert_eq!(
        fact_ids(
            &engine
                .explore(semantic_request(indexed.root(), false))
                .await
                .expect("independent semantic read")
        ),
        ["f1"]
    );
    assert!(
        engine
            .apply_semantics(semantic_options(indexed.root(), "second-source"))
            .await
            .is_err()
    );
    assert_eq!(GraphHead::read(&home).expect("head"), head);
    engine.close();
    let zero = ModelRuntimeManager::with_factory(|_, _, _| {
        panic!("configuration-only graph build must not acquire a model")
    });
    assert!(
        indexed
            .service
            .index(&zero, indexed.graph_only())
            .await
            .expect("refresh graph")
            .graph
            .expect("graph")
            .graph_selected
    );
    let reopened = crate::ZvecGrep::new();
    paths(&reopened, &indexed).await;
    assert_eq!(
        fact_ids(
            &reopened
                .explore(semantic_request(indexed.root(), false))
                .await
                .expect("retained semantics")
        ),
        ["f1"]
    );
    reopened.close();
    assert_eq!(text_hashes(&home.join("storage")), before);
}

#[tokio::test]
async fn m6_unmapped_header_cannot_bypass_coverage_with_a_forged_ready_head() {
    let dir = tempfile::tempdir().expect("workspace");
    std::fs::write(
        dir.path().join("unmapped.h"),
        "static inline void header_function(){}\n",
    )
    .expect("header");
    let mut initial = options(dir.path());
    initial.graph = Some(GraphOptions {
        scope: crate::api::index::graph::GraphScope::Code,
        module_roots: vec![],
        rules: None,
    });
    let service = WorkspaceIndexService::with_test_registry();
    let result = service
        .index(&models(&Arc::new(AtomicUsize::new(0))), initial)
        .await
        .expect("indexed header");
    assert_ne!(
        result.graph.expect("graph").domains["code"].ready,
        GraphReady::Ready
    );
    drop(service);
    let home = active(dir.path()).storage_home();
    let mut head = GraphHead::read(&home).expect("head").expect("selected");
    head.domains.get_mut("code").expect("code").ready = GraphReady::Ready;
    std::fs::write(
        home.join("graph/HEAD.json"),
        serde_json::to_vec(&head).expect("head JSON"),
    )
    .expect("forge ready only");
    let engine = crate::ZvecGrep::new();
    assert!(
        engine
            .explore(request(
                dir.path(),
                ExploreOperation::Locate {
                    target: named("header_function")
                }
            ))
            .await
            .is_err()
    );
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
}

fn add_ambiguities(indexed: &Indexed) {
    for (source, name, _) in leaf_sources() {
        let original = &indexed
            .spec
            .sources
            .iter()
            .find(|s| s.path == source)
            .expect("source")
            .text;
        let changed = if std::path::Path::new(source)
            .extension()
            .and_then(|p| p.to_str())
            == Some("go")
        {
            format!("{original}\nfunc {name}(){{}}\n")
        } else if std::path::Path::new(source)
            .extension()
            .and_then(|p| p.to_str())
            == Some("java")
        {
            let at = original.rfind('}').expect("class body");
            format!(
                "{}public static void {name}(int x){{}}{}",
                &original[..at],
                &original[at..]
            )
        } else if std::path::Path::new(source)
            .extension()
            .and_then(|p| p.to_str())
            == Some("hpp")
        {
            original.replace(
                "static void cppLeaf()",
                "static void cppLeaf(int x){}static void cppLeaf()",
            )
        } else {
            format!("{original}\nstatic inline void {name}(int x){{}}\n")
        };
        std::fs::write(indexed.root().join(source), changed).expect("ambiguous definitions");
    }
}

#[tokio::test]
async fn m7_code_only_reads_keep_markdown_and_javascript_as_text_only_sources() {
    let indexed = Indexed::new().await;
    std::fs::write(
        indexed.root().join("notes.md"),
        "# Text source\nordinary documentation\n",
    )
    .expect("Markdown");
    std::fs::write(
        indexed.root().join("notes.js"),
        "function ordinaryText() {}\n",
    )
    .expect("JavaScript");
    let mut update = options(indexed.root());
    update.graph = Some(indexed.spec.graph.clone());
    update.scan.globs = Some(vec!["!**/go.mod".into(), "!compile_commands.json".into()]);
    let result = indexed
        .service
        .index(&indexed.manager, update)
        .await
        .expect("text and code graph publication");
    assert_eq!(
        result.graph.expect("graph").domains["code"].ready,
        GraphReady::Ready
    );
    let before = text_hashes(&active(indexed.root()).storage_home().join("storage"));
    let engine = crate::ZvecGrep::new();
    paths(&engine, &indexed).await;
    assert_eq!(engine.runtime_snapshot().loaded_models, 0);
    engine.close();
    let reopened = crate::ZvecGrep::new();
    paths(&reopened, &indexed).await;
    reopened.close();
    assert_eq!(
        text_hashes(&active(indexed.root()).storage_home().join("storage")),
        before
    );
}
